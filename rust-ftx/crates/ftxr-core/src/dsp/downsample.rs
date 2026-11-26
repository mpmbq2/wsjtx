// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FFT-domain FT8 baseband downsampler (`ft8_downsample.f90`) with immutable pass snapshot `cx`.

use std::f32::consts::PI;

use num_complex::Complex32;

use crate::ft4::params as ft4_params;
use crate::ft8::params::{DOWN_NFFT1, DOWN_NFFT2, NMAX, NSPS};

use super::fft::{Ft4FftPlans, Ft8FftPlans};

/// Immutable 192,000-pt r2c spectrum snapshot (`cx[0..=96000]`) captured once per subtraction pass.
///
/// Shared read-only across all candidates in a pass (enables lock-free `rayon` in Stage 5).
#[derive(Debug, Clone)]
pub struct Ft8SpectrumSnapshot {
    pub cx: Vec<Complex32>,
}

impl Default for Ft8SpectrumSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft8SpectrumSnapshot {
    pub fn new() -> Self {
        Self {
            cx: vec![Complex32::ZERO; DOWN_NFFT1 / 2 + 1],
        }
    }
}

/// Scratch buffers for `Ft8Downsampler::compute_snapshot` (zero per-pass heap churn).
pub struct DownsampleSnapshotScratch {
    r2c_in: Vec<f32>,
    r2c_scratch: Vec<Complex32>,
}

impl DownsampleSnapshotScratch {
    pub fn new(plans: &Ft8FftPlans) -> Self {
        Self {
            r2c_in: plans.r2c_192000.make_input_vec(),
            r2c_scratch: plans.r2c_192000.make_scratch_vec(),
        }
    }
}

/// Pure-Rust port of `wsjtx/lib/ft8/ft8_downsample.f90`.
#[derive(Debug, Clone)]
pub struct Ft8Downsampler {
    taper: [f32; 101],
    scale: f32,
}

impl Default for Ft8Downsampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft8Downsampler {
    pub fn new() -> Self {
        let mut taper = [0.0_f32; 101];
        for (i, t) in taper.iter_mut().enumerate() {
            *t = 0.5 * (1.0 + ((i as f32) * PI / 100.0).cos());
        }
        let scale = 1.0 / ((DOWN_NFFT1 as f32) * (DOWN_NFFT2 as f32)).sqrt();
        Self { taper, scale }
    }

    /// Computes zero-padded 192,000-pt r2c FFT of `dd[0..180000]` into `snapshot.cx`.
    pub fn compute_snapshot(
        &self,
        dd: &[f32; NMAX],
        plans: &Ft8FftPlans,
        scratch: &mut DownsampleSnapshotScratch,
        snapshot: &mut Ft8SpectrumSnapshot,
    ) {
        scratch.r2c_in[..NMAX].copy_from_slice(dd);
        scratch.r2c_in[NMAX..DOWN_NFFT1].fill(0.0);
        plans
            .r2c_192000
            .process_with_scratch(
                &mut scratch.r2c_in,
                &mut snapshot.cx,
                &mut scratch.r2c_scratch,
            )
            .expect("r2c_192000 scratch sized");
    }

    /// Downconverts `f0_hz` to 200 Hz baseband `cd0[0..3200]` from an immutable `snapshot`.
    pub fn downsample_from_snapshot(
        &self,
        snapshot: &Ft8SpectrumSnapshot,
        f0_hz: f32,
        plans: &Ft8FftPlans,
        ifft_scratch: &mut [Complex32],
        cd0: &mut [Complex32; DOWN_NFFT2],
    ) {
        let df = 12_000.0_f32 / (DOWN_NFFT1 as f32); // 0.0625 Hz
        let baud = 12_000.0_f32 / (NSPS as f32); // 6.25 Hz
        let i0 = (f0_hz / df).round() as isize;
        let ft = f0_hz + 8.5 * baud;
        let it = ((ft / df).round() as isize).min((DOWN_NFFT1 / 2) as isize);
        let fb = f0_hz - 1.5 * baud;
        let ib = ((fb / df).round() as isize).max(1);

        cd0.fill(Complex32::ZERO);
        if ib > it {
            return;
        }

        let k = ((it - ib + 1) as usize).min(DOWN_NFFT2);
        let ib_u = ib as usize;
        cd0[..k].copy_from_slice(&snapshot.cx[ib_u..ib_u + k]);

        if k > 100 {
            for idx in 0..=100 {
                cd0[idx] *= self.taper[100 - idx];
            }
            for idx in 0..=100 {
                cd0[k - 1 - 100 + idx] *= self.taper[idx];
            }
        }

        let shift = (i0 - ib).rem_euclid(DOWN_NFFT2 as isize) as usize;
        cd0.rotate_left(shift);

        plans.c2c_inv_3200.process_with_scratch(cd0, ifft_scratch);
        for c in cd0.iter_mut() {
            *c *= self.scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn downsampled_tone_lands_on_exact_symbol_fft_bin() {
        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let mut snapshot = Ft8SpectrumSnapshot::new();

        // Tone at f0 + 3 * 6.25 Hz = 1518.75 Hz.
        let f0 = 1500.0_f32;
        let f_tone = f0 + 3.0 * 6.25;
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for (n, s) in dd.iter_mut().enumerate() {
            *s = (TAU * f_tone * (n as f32) / 12_000.0).cos();
        }

        downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);
        let mut ifft_scratch = vec![Complex32::ZERO; plans.c2c_inv_3200.get_inplace_scratch_len()];
        let mut cd0 = [Complex32::ZERO; DOWN_NFFT2];
        downsampler.downsample_from_snapshot(&snapshot, f0, &plans, &mut ifft_scratch, &mut cd0);

        // Take a 32-pt forward FFT of cd0[100..132] (well inside valid window) -> peaks at bin 3.
        let mut sym = [Complex32::ZERO; 32];
        sym.copy_from_slice(&cd0[100..132]);
        let mut sym_scratch = vec![Complex32::ZERO; plans.c2c_fwd_32.get_inplace_scratch_len()];
        plans
            .c2c_fwd_32
            .process_with_scratch(&mut sym, &mut sym_scratch);

        let peak_bin = sym
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.norm().total_cmp(&b.norm()))
            .map(|(idx, _)| idx)
            .unwrap();
        assert_eq!(peak_bin, 3);
    }

    #[test]
    fn ft4_downsampled_tone_lands_in_bin_and_attenuates_out_of_band() {
        let plans = Ft4FftPlans::new();
        let downsampler = Ft4Downsampler::new();
        let mut snap_scratch = Ft4DownsampleScratch::new(&plans);
        let mut spectrum = Ft4PassSpectrum::new();
        let baud = ft4_params::F_BAUD;
        let f0 = 1500.0_f32;

        // Passband test: tone at f0 + 2 * baud must land cleanly in bin 2 of a 32-pt symbol FFT.
        let f_tone = f0 + 2.0 * baud;
        let mut dd: Box<[f32; ft4_params::NMAX]> = vec![0.0_f32; ft4_params::NMAX]
            .into_boxed_slice()
            .try_into()
            .unwrap();
        for (n, s) in dd.iter_mut().enumerate() {
            *s = (TAU * f_tone * (n as f32) / 12_000.0).cos();
        }
        downsampler.compute_pass_spectrum(&dd, &plans, &mut snap_scratch, &mut spectrum);

        let mut cd0 = [Complex32::ZERO; ft4_params::NDMAX];
        downsampler.downsample_from_spectrum(&spectrum, f0, &plans, &mut snap_scratch, &mut cd0);

        let mut sym = [Complex32::ZERO; ft4_params::NSS];
        sym.copy_from_slice(&cd0[200..200 + ft4_params::NSS]);
        let mut sym_scratch = vec![Complex32::ZERO; plans.c2c_fwd_32.get_inplace_scratch_len()];
        plans
            .c2c_fwd_32
            .process_with_scratch(&mut sym, &mut sym_scratch);
        let peak_bin = sym
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.norm().total_cmp(&b.norm()))
            .map(|(idx, _)| idx)
            .unwrap();
        assert_eq!(peak_bin, 2);

        // Stopband test: out-of-band tone at f0 + 10 * baud must be strongly attenuated.
        let f_oob = f0 + 10.0 * baud;
        for (n, s) in dd.iter_mut().enumerate() {
            *s = (TAU * f_oob * (n as f32) / 12_000.0).cos();
        }
        downsampler.compute_pass_spectrum(&dd, &plans, &mut snap_scratch, &mut spectrum);
        downsampler.downsample_from_spectrum(&spectrum, f0, &plans, &mut snap_scratch, &mut cd0);
        let rms: f32 = (cd0.iter().map(|c| c.norm_sqr()).sum::<f32>() / (cd0.len() as f32)).sqrt();
        // Unwindowed 72,576-sample pure tone has spectral leakage ~ -45 dB outside the passband.
        assert!(rms < 0.005, "expected > 40 dB stopband attenuation, got rms={rms}");
    }
}

/// Immutable 72,576-pt r2c spectrum snapshot (`cx[0..=36_288]`) captured once per FT4
/// subtraction pass (`ft4_downsample.f90` pass snapshot).
///
/// Shared read-only across candidates in Stage 5 parallel decode.
#[derive(Debug, Clone)]
pub struct Ft4PassSpectrum {
    pub cx: Vec<Complex32>,
}

impl Default for Ft4PassSpectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4PassSpectrum {
    pub fn new() -> Self {
        Self {
            cx: vec![Complex32::ZERO; ft4_params::NMAX / 2 + 1],
        }
    }
}

/// Scratch buffers for `Ft4Downsampler` (zero per-pass or per-candidate heap churn).
pub struct Ft4DownsampleScratch {
    r2c_in: Vec<f32>,
    r2c_scratch: Vec<Complex32>,
    ifft_scratch: Vec<Complex32>,
}

impl Ft4DownsampleScratch {
    pub fn new(plans: &Ft4FftPlans) -> Self {
        Self {
            r2c_in: plans.r2c_72576.make_input_vec(),
            r2c_scratch: plans.r2c_72576.make_scratch_vec(),
            ifft_scratch: vec![Complex32::ZERO; plans.c2c_inv_4032.get_inplace_scratch_len()],
        }
    }
}

/// Pure-Rust port of `wsjtx/lib/ft4/ft4_downsample.f90`.
#[derive(Debug, Clone)]
pub struct Ft4Downsampler {
    window: Vec<f32>,
    inv_ndmax: f32,
}

impl Default for Ft4Downsampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4Downsampler {
    pub fn new() -> Self {
        let df = ft4_params::DF_DOWNSAMPLE;
        let baud = ft4_params::F_BAUD;
        let iwt = (0.5 * baud / df) as usize;
        let iwf = (4.0 * baud / df) as usize;
        let iws = (baud / df) as usize;
        debug_assert_eq!(iwt, 63);
        debug_assert_eq!(iwf, 504);
        debug_assert_eq!(iws, 126);

        let mut window = vec![0.0_f32; ft4_params::NDMAX];
        for (i, slot) in window[..iwt].iter_mut().enumerate() {
            let taper_arg = PI * ((iwt - 1 - i) as f32) / (iwt as f32);
            *slot = 0.5 * (1.0 + taper_arg.cos());
        }
        window[iwt..iwt + iwf].fill(1.0);
        for i in 0..iwt {
            let taper_arg = PI * (i as f32) / (iwt as f32);
            window[iwt + iwf + i] = 0.5 * (1.0 + taper_arg.cos());
        }
        window.rotate_left(iws);

        Self {
            window,
            inv_ndmax: 1.0_f32 / (ft4_params::NDMAX as f32),
        }
    }

    /// Computes full 72,576-pt r2c FFT of `dd` into `spectrum.cx` once per subtraction pass.
    pub fn compute_pass_spectrum(
        &self,
        dd: &[f32; ft4_params::NMAX],
        plans: &Ft4FftPlans,
        scratch: &mut Ft4DownsampleScratch,
        spectrum: &mut Ft4PassSpectrum,
    ) {
        scratch.r2c_in.copy_from_slice(dd);
        plans
            .r2c_72576
            .process_with_scratch(
                &mut scratch.r2c_in,
                &mut spectrum.cx,
                &mut scratch.r2c_scratch,
            )
            .expect("r2c_72576 scratch sized");
    }

    /// Downconverts `f0_hz` to 666.67 Hz complex baseband `cd0[0..4032]` from immutable `spectrum`.
    pub fn downsample_from_spectrum(
        &self,
        spectrum: &Ft4PassSpectrum,
        f0_hz: f32,
        plans: &Ft4FftPlans,
        scratch: &mut Ft4DownsampleScratch,
        cd0: &mut [Complex32; ft4_params::NDMAX],
    ) {
        self.downsample_with_ifft_scratch(
            spectrum,
            f0_hz,
            plans,
            &mut scratch.ifft_scratch,
            cd0,
        );
    }

    /// Stage-5 friendly variant accepting an arbitrary caller-owned `ifft_scratch` buffer.
    pub fn downsample_with_ifft_scratch(
        &self,
        spectrum: &Ft4PassSpectrum,
        f0_hz: f32,
        plans: &Ft4FftPlans,
        ifft_scratch: &mut [Complex32],
        cd0: &mut [Complex32; ft4_params::NDMAX],
    ) {
        let half_max = (ft4_params::NMAX / 2) as isize;
        let i0 = (f0_hz / ft4_params::DF_DOWNSAMPLE).round() as isize;

        cd0.fill(Complex32::ZERO);
        if i0 >= 0 && i0 <= half_max {
            cd0[0] = spectrum.cx[i0 as usize];
        }
        for i in 1..=(ft4_params::NDMAX / 2) {
            let i_isize = i as isize;
            let pos = i0 + i_isize;
            if pos >= 0 && pos <= half_max {
                cd0[i] = spectrum.cx[pos as usize];
            }
            let neg = i0 - i_isize;
            if neg >= 0 && neg <= half_max {
                cd0[ft4_params::NDMAX - i] = spectrum.cx[neg as usize];
            }
        }
        for (dst, &w) in cd0.iter_mut().zip(self.window.iter()) {
            *dst *= w;
        }

        plans
            .c2c_inv_4032
            .process_with_scratch(cd0, ifft_scratch);
        for c in cd0.iter_mut() {
            *c *= self.inv_ndmax;
        }
    }
}
