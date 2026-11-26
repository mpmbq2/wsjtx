// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Coherent time-varying FT4 signal estimation and subtraction (`wsjtx/lib/ft4/subtractft4.f90`).

use num_complex::Complex32;

use crate::dsp::fft::Ft4FftPlans;
use crate::dsp::window::cos2_lpf_window;
use crate::modulate::ft4::gen_ft4_wave_complex;

use super::params::{NMAX, NN, NSPS, NZ2, SAMPLE_RATE_HZ, SUBTRACT_NFILT, SUBTRACT_SHIFT};

/// Precomputed 1400-tap `cos^2` low-pass filter spectrum for `subtractft4`.
#[derive(Debug, Clone)]
pub struct Ft4Subtractor {
    cw_fft: Vec<Complex32>,
}

/// Reusable scratch buffers for `Ft4Subtractor::subtract` (zero per-decode heap churn).
pub struct Ft4SubtractScratch {
    camp: Vec<Complex32>,
    c2c_scratch: Vec<Complex32>,
}

impl Ft4SubtractScratch {
    pub fn new(plans: &Ft4FftPlans) -> Self {
        let c2c_len = plans
            .c2c_fwd_72576
            .get_inplace_scratch_len()
            .max(plans.c2c_inv_72576.get_inplace_scratch_len());
        Self {
            camp: vec![Complex32::ZERO; NMAX],
            c2c_scratch: vec![Complex32::ZERO; c2c_len],
        }
    }
}

impl Ft4Subtractor {
    pub fn new(plans: &Ft4FftPlans) -> Self {
        let (taps, sumw) = cos2_lpf_window(SUBTRACT_NFILT);
        let mut cw_fft = vec![Complex32::ZERO; NMAX];
        let inv_scale = 1.0_f32 / (sumw * (NMAX as f32));
        for (dst, &w) in cw_fft[..taps.len()].iter_mut().zip(taps.iter()) {
            *dst = Complex32::new(w * inv_scale, 0.0);
        }
        // Matches Fortran `cw = cshift(cw, NFILT / 2 + 1)` (`701`).
        cw_fft.rotate_left(SUBTRACT_SHIFT);

        let mut c2c_scratch = vec![Complex32::ZERO; plans.c2c_fwd_72576.get_inplace_scratch_len()];
        plans
            .c2c_fwd_72576
            .process_with_scratch(&mut cw_fft, &mut c2c_scratch);

        Self { cw_fft }
    }

    /// Estimates complex amplitude envelope of the decoded FT4 signal and subtracts it
    /// from `dd[0..72_576]` (`wsjtx/lib/ft4/subtractft4.f90`).
    ///
    /// `dt_from_buf_start_sec` is the offset of Costas symbol `0` relative to `dd[0]` in
    /// seconds (`ibest / 666.67`, NOT user-displayed `xdt = ibest / 666.67 - 0.5`).
    pub fn subtract(
        &self,
        dd: &mut [f32; NMAX],
        tones: &[u8; NN],
        f0_hz: f32,
        dt_from_buf_start_sec: f32,
        plans: &Ft4FftPlans,
        scratch: &mut Ft4SubtractScratch,
    ) {
        // 1. Unit-amplitude complex reference waveform (105 * 576 = 60,480 samples).
        // `cref[576..]` aligns with Costas symbol 0 at `dt_from_buf_start_sec * 12000`.
        let cref = gen_ft4_wave_complex(tones, f64::from(f0_hz), SAMPLE_RATE_HZ);
        debug_assert_eq!(cref.len(), NZ2);

        let nstart0 = (dt_from_buf_start_sec * (SAMPLE_RATE_HZ as f32)) as i32 - (NSPS as i32);

        // 2. Complex envelope `camp(t) = dd(t) * conj(cref(t))` directly on real `dd`
        // matching `wsjtx/lib/ft4/subtractft4.f90:5-8`.
        scratch.camp.fill(Complex32::ZERO);
        for (k, &ref_val) in cref.iter().enumerate() {
            let idx = nstart0 + k as i32;
            if idx >= 0 && (idx as usize) < NMAX {
                let u = idx as usize;
                scratch.camp[u] = dd[u] * ref_val.conj();
            }
        }

        // 3. Smooth complex amplitude envelope via FFT multiplication with `cw_fft`.
        plans
            .c2c_fwd_72576
            .process_with_scratch(&mut scratch.camp, &mut scratch.c2c_scratch);
        for (c, &w) in scratch.camp.iter_mut().zip(self.cw_fft.iter()) {
            *c *= w;
        }
        plans
            .c2c_inv_72576
            .process_with_scratch(&mut scratch.camp, &mut scratch.c2c_scratch);

        // 4. Subtract reconstructed real waveform (`2 * Re{cfilt * cref}`) from `dd`.
        for (k, &ref_val) in cref.iter().enumerate() {
            let idx = nstart0 + k as i32;
            if idx >= 0 && (idx as usize) < NMAX {
                let u = idx as usize;
                dd[u] -= 2.0 * (scratch.camp[u] * ref_val).re;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulate::ft4::{encode_ft4_tones_from_77bits, gen_ft4_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn subtractft4_attenuates_strong_signal_and_unmasks_cochannel_weak() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let p_strong = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let p_weak = pack77("CQ W9XYZ EN52", None, &ctx, &ht).unwrap();

        let tones_strong = encode_ft4_tones_from_77bits(&p_strong.bits);
        let tones_weak = encode_ft4_tones_from_77bits(&p_weak.bits);

        let w_strong = gen_ft4_wave_real(&tones_strong, 1500.0, SAMPLE_RATE_HZ);

        // 1. Standalone strong cancellation attenuation (> 27 dB full frame, > 50 dB steady-state)
        let mut dd_strong_only: Box<[f32; NMAX]> =
            vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for i in 0..NZ2 {
            dd_strong_only[5424 + i] = 20.0 * w_strong[i];
        }
        let strong_power_before: f64 =
            w_strong.iter().map(|&v| f64::from(20.0 * v).powi(2)).sum();

        let plans = Ft4FftPlans::new();
        let sub = Ft4Subtractor::new(&plans);
        let mut scratch = Ft4SubtractScratch::new(&plans);
        sub.subtract(
            &mut dd_strong_only,
            &tones_strong,
            1500.0,
            0.5,
            &plans,
            &mut scratch,
        );
        let residual_power: f64 = dd_strong_only.iter().map(|&v| f64::from(v).powi(2)).sum();
        let atten_db = 10.0 * (strong_power_before / residual_power.max(1e-30)).log10();
        assert!(
            atten_db > 27.0,
            "expected > 27 dB full-frame suppression (edge LPF limited), got {atten_db:.2} dB"
        );
        let interior_sig: f64 = dd_strong_only[7000..63000]
            .iter()
            .map(|&v| f64::from(v).powi(2))
            .sum();
        let interior_orig: f64 = w_strong[1576..(1576 + 56000)]
            .iter()
            .map(|&v| f64::from(20.0 * v).powi(2))
            .sum();
        let interior_atten_db = 10.0 * (interior_orig / interior_sig.max(1e-30)).log10();
        assert!(
            interior_atten_db > 50.0,
            "expected > 50 dB interior steady-state suppression, got {interior_atten_db:.2} dB"
        );

        // 2. Co-channel overlapping unmasking (+20 dB strong at 1500 Hz + weak at 1525 Hz in band).
        let w_weak_25 = gen_ft4_wave_real(&tones_weak, 1525.0, SAMPLE_RATE_HZ);
        let mut dd_co: Box<[f32; NMAX]> =
            vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        let mut state = 0x5eed_cafe_beef_1234_u64;
        for sample in dd_co.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((state >> 33) as f32) / (u32::MAX as f32) - 0.5;
            *sample = 0.05 * u;
        }
        for i in 0..NZ2 {
            dd_co[5424 + i] += 10.0 * w_strong[i];
            dd_co[5700 + i] += w_weak_25[i];
        }
        let mut dec = crate::ft4::decoder::Ft4Decoder::new();
        let cfg = crate::DecodeConfig::for_mode(crate::Mode::Ft4);
        let decs = dec
            .decode_f32(
                &*dd_co,
                &cfg,
                &ht,
                None,
                crate::ft4::decoder::Ft4DecodeOptions::default(),
                |_| {},
            )
            .unwrap();
        assert!(
            decs.iter().any(|d| d.message == "CQ K1ABC FN42")
                && decs.iter().any(|d| d.message == "CQ W9XYZ EN52"),
            "expected both strong and masked co-channel FT4 signals, got {decs:?}"
        );
    }
}
