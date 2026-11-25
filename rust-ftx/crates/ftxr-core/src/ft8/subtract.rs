// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 multi-pass signal subtraction (`wsjtx/lib/ft8/subtractft8.f90`, `lib/peakup.f90`).

use num_complex::Complex32;

use crate::dsp::fft::Ft8FftPlans;
use crate::dsp::window::cos2_lpf_window;
use crate::modulate::ft8::gen_ft8_wave_complex;

use super::params::{NMAX, NN, NZ, SAMPLE_RATE_HZ, SUB_NFFT, SUB_NFILT};

/// Parabolic 3-point extremum interpolation matching `wsjtx/lib/peakup.f90`,
/// returning `f32::INFINITY` when the quadratic curvature denominator is zero.
#[inline]
pub fn peakup(ym: f32, y0: f32, yp: f32) -> f32 {
    let b = yp - ym;
    let c = yp + ym - 2.0 * y0;
    if c.abs() < 1e-30 {
        f32::INFINITY
    } else {
        -b / (2.0 * c)
    }
}

/// Reusable scratch buffers for `Ft8Subtractor::subtract` (zero per-call heap allocations).
pub struct SubtractScratch {
    camp: Vec<Complex32>,
    cfilt: Vec<Complex32>,
    c2c_scratch: Vec<Complex32>,
    dd_work: Box<[f32; NMAX]>,
    x_buf: Vec<f32>,
    r2c_out: Vec<Complex32>,
    r2c_scratch: Vec<Complex32>,
}

impl SubtractScratch {
    pub fn new(plans: &Ft8FftPlans) -> Self {
        let c2c_len = plans
            .c2c_fwd_180000
            .get_inplace_scratch_len()
            .max(plans.c2c_inv_180000.get_inplace_scratch_len());
        Self {
            camp: vec![Complex32::ZERO; SUB_NFFT],
            cfilt: vec![Complex32::ZERO; SUB_NFFT],
            c2c_scratch: vec![Complex32::ZERO; c2c_len],
            dd_work: vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap(),
            x_buf: plans.r2c_180000.make_input_vec(),
            r2c_out: plans.r2c_180000.make_output_vec(),
            r2c_scratch: plans.r2c_180000.make_scratch_vec(),
        }
    }
}

/// Pure-Rust port of `wsjtx/lib/ft8/subtractft8.f90`.
pub struct Ft8Subtractor {
    cw_fft: Vec<Complex32>,
    endcorr: [f32; SUB_NFILT / 2 + 1],
}

impl Ft8Subtractor {
    pub fn new(plans: &Ft8FftPlans) -> Self {
        let (window, sumw) = cos2_lpf_window(SUB_NFILT);
        let mut cw_fft = vec![Complex32::ZERO; SUB_NFFT];
        for (dst, &w) in cw_fft[..=SUB_NFILT].iter_mut().zip(window.iter()) {
            dst.re = w / sumw;
        }
        // Fortran `cshift(cw, NFILT/2 + 1)` rotates left by 2001 (NOT 2000!).
        cw_fft.rotate_left(SUB_NFILT / 2 + 1);
        let mut scratch = vec![Complex32::ZERO; plans.c2c_fwd_180000.get_inplace_scratch_len()];
        plans
            .c2c_fwd_180000
            .process_with_scratch(&mut cw_fft, &mut scratch);
        let fac = 1.0_f32 / (SUB_NFFT as f32);
        for c in &mut cw_fft {
            *c *= fac;
        }

        let half = SUB_NFILT / 2;
        let mut endcorr = [1.0_f32; SUB_NFILT / 2 + 1];
        for (idx, dst) in endcorr.iter_mut().enumerate() {
            // Fortran `j = idx + 1` sums `window(j - 1 : NFILT/2)`, i.e. indices `half + idx ..= SUB_NFILT`.
            let tail_sum: f32 = window[half + idx..=SUB_NFILT].iter().sum();
            let denom = (1.0 - tail_sum / sumw).max(1e-12);
            *dst = 1.0 / denom;
        }

        Self { cw_fft, endcorr }
    }

    /// Subtracts a decoded FT8 signal from `dd0` in-place using complex amplitude estimation.
    #[allow(clippy::too_many_arguments)]
    pub fn subtract(
        &self,
        dd0: &mut [f32; NMAX],
        itone: &[u8; NN],
        f0_hz: f32,
        dt_raw_sec: f32,
        refine_dt: bool,
        plans: &Ft8FftPlans,
        scratch: &mut SubtractScratch,
    ) {
        let cref = gen_ft8_wave_complex(itone, f64::from(f0_hz), SAMPLE_RATE_HZ);
        let nmax_i = NMAX as isize;
        let half = SUB_NFILT / 2;

        let mut run_sqf = |idt: isize, compute_energy: bool| -> f32 {
            let start0 = ((dt_raw_sec * (SAMPLE_RATE_HZ as f32)) as isize) + idt;
            scratch.camp.fill(Complex32::ZERO);
            for (i0, &cr) in cref.iter().enumerate() {
                let j0 = start0 + (i0 as isize);
                if j0 >= 0 && j0 < nmax_i {
                    scratch.camp[i0] = cr.conj() * dd0[j0 as usize];
                }
            }

            scratch.cfilt[..NZ].copy_from_slice(&scratch.camp[..NZ]);
            scratch.cfilt[NZ..SUB_NFFT].fill(Complex32::ZERO);

            plans
                .c2c_fwd_180000
                .process_with_scratch(&mut scratch.cfilt, &mut scratch.c2c_scratch);
            for (dst, &w) in scratch.cfilt.iter_mut().zip(self.cw_fft.iter()) {
                *dst *= w;
            }
            plans
                .c2c_inv_180000
                .process_with_scratch(&mut scratch.cfilt, &mut scratch.c2c_scratch);

            for (idx, &ec) in self.endcorr[..=half].iter().enumerate() {
                scratch.cfilt[idx] *= ec;
                scratch.cfilt[NZ - 1 - idx] *= ec;
            }

            scratch.dd_work.copy_from_slice(dd0);
            if compute_energy {
                scratch.x_buf.fill(0.0);
            }

            for (i0, &cr) in cref.iter().enumerate() {
                let j0 = start0 + (i0 as isize);
                if j0 >= 0 && j0 < nmax_i {
                    let ju = j0 as usize;
                    let z = scratch.cfilt[i0] * cr;
                    let val = scratch.dd_work[ju] - 2.0 * z.re;
                    scratch.dd_work[ju] = val;
                    if compute_energy {
                        scratch.x_buf[i0] = val;
                    }
                }
            }

            if !compute_energy {
                return 0.0;
            }

            plans
                .r2c_180000
                .process_with_scratch(
                    &mut scratch.x_buf,
                    &mut scratch.r2c_out,
                    &mut scratch.r2c_scratch,
                )
                .expect("r2c_180000 scratch sized");

            let df = (SAMPLE_RATE_HZ as f32) / (SUB_NFFT as f32);
            let ia = (((f0_hz - 1.5 * 6.25) / df) as isize).max(0) as usize;
            let ib = (((f0_hz + 8.5 * 6.25) / df) as isize)
                .max(0)
                .min((SUB_NFFT / 2) as isize) as usize;
            let mut sqq = 0.0_f32;
            if ia <= ib {
                for bin in &scratch.r2c_out[ia..=ib] {
                    sqq += bin.norm_sqr();
                }
            }
            sqq
        };

        if refine_dt {
            let sqa = run_sqf(-90, true);
            let sqb = run_sqf(90, true);
            let sq0 = run_sqf(0, true);
            let dx = peakup(sqa, sq0, sqb);
            if !dx.is_finite() || dx.abs() > 1.0 {
                return;
            }
            let i2 = (90.0 * dx).round() as isize;
            run_sqf(i2, false);
        } else {
            run_sqf(0, false);
        }

        dd0.copy_from_slice(&*scratch.dd_work);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
    use crate::ft8::ft8b::{ft8b_decode_candidate, Ft8bScratch};
    use crate::ft8::params::NH1;
    use crate::ft8::sync8::{sync8, BitMetricMode, Sync8Scratch};
    use crate::ft8::sync8d::Sync8dTables;
    use crate::modulate::ft8::{encode_ft8_tones_from_77bits, gen_ft8_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn subtraction_attenuates_strong_signal_and_unmasks_weak_signal() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();

        let p_strong = pack77("CQ N0AAA EM10", None, &ctx, &ht).unwrap();
        let tones_strong = encode_ft8_tones_from_77bits(&p_strong.bits);
        let wave_strong = gen_ft8_wave_real(&tones_strong, 1500.0, 12_000);

        let p_weak = pack77("CQ K9BBB DM79", None, &ctx, &ht).unwrap();
        let tones_weak = encode_ft8_tones_from_77bits(&p_weak.bits);
        let wave_weak = gen_ft8_wave_real(&tones_weak, 1510.0, 12_000);

        let amp_strong = 1000.0_f32;
        let amp_weak = amp_strong * 10_f32.powf(-14.0 / 20.0);

        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for i in 0..NZ {
            dd[6000 + i] = amp_strong * wave_strong[i] + amp_weak * wave_weak[i];
        }

        let mut dd_strong_only: Box<[f32; NMAX]> =
            vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for i in 0..NZ {
            dd_strong_only[6000 + i] = amp_strong * wave_strong[i];
        }
        let energy_before: f32 = dd_strong_only.iter().map(|&x| x * x).sum();

        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let sync8d_tables = Sync8dTables::new();
        let subtractor = Ft8Subtractor::new(&plans);
        let mut sub_scratch = SubtractScratch::new(&plans);

        subtractor.subtract(
            &mut dd_strong_only,
            &tones_strong,
            1500.0,
            0.5,
            false,
            &plans,
            &mut sub_scratch,
        );
        let energy_after: f32 = dd_strong_only.iter().map(|&x| x * x).sum();
        let atten_db = 10.0 * (energy_before / energy_after.max(1e-20)).log10();
        assert!(atten_db > 25.0, "expected >25 dB attenuation, got {atten_db:.2} dB");

        // Now subtract strong signal from the 0 dB + -14 dB 10 Hz mixture and decode the weak one.
        let mut sync_scratch = Sync8Scratch::new(&plans);
        let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let mut snapshot = Ft8SpectrumSnapshot::new();
        let mut ft8b_scratch = Ft8bScratch::new(&plans);

        // Pass 1 decodes the strong signal and subtracts it.
        let cands1 = sync8(&dd, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut sync_scratch);
        downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);
        let mut decoded_msgs = Vec::new();
        for cand in cands1 {
            let bin = ((cand.freq_hz / 3.125).round() as usize).clamp(1, NH1);
            let xbase = 10.0_f32.powf(0.1 * (sync_scratch.sbase_db()[bin] - 40.0));
            if let Some(out) = ft8b_decode_candidate(
                &snapshot,
                &cand,
                xbase,
                1,
                BitMetricMode::Amplitude,
                false,
                0,
                &plans,
                &downsampler,
                &sync8d_tables,
                &ctx,
                &ht,
                &mut ft8b_scratch,
            ) {
                if !decoded_msgs.contains(&out.message) {
                    subtractor.subtract(
                        &mut dd,
                        &out.itone,
                        out.freq_hz,
                        out.dt_raw_sec,
                        false,
                        &plans,
                        &mut sub_scratch,
                    );
                    decoded_msgs.push(out.message);
                }
            }
        }
        assert!(decoded_msgs.contains(&"CQ N0AAA EM10".to_string()));

        // Pass 2 finds and decodes the masked -14 dB signal 10 Hz away.
        let cands2 = sync8(&dd, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut sync_scratch);
        downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);
        for cand in cands2 {
            let bin = ((cand.freq_hz / 3.125).round() as usize).clamp(1, NH1);
            let xbase = 10.0_f32.powf(0.1 * (sync_scratch.sbase_db()[bin] - 40.0));
            if let Some(out) = ft8b_decode_candidate(
                &snapshot,
                &cand,
                xbase,
                1,
                BitMetricMode::Power,
                false,
                0,
                &plans,
                &downsampler,
                &sync8d_tables,
                &ctx,
                &ht,
                &mut ft8b_scratch,
            ) {
                if !decoded_msgs.contains(&out.message) {
                    decoded_msgs.push(out.message);
                }
            }
        }
        assert!(
            decoded_msgs.contains(&"CQ K9BBB DM79".to_string()),
            "expected weak masked signal after subtraction, got {decoded_msgs:?}"
        );
    }
}
