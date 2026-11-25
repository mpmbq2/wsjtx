// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FFT-domain FT8 baseband downsampler (`ft8_downsample.f90`) with immutable pass snapshot `cx`.

use std::f32::consts::PI;

use num_complex::Complex32;

use crate::ft8::params::{DOWN_NFFT1, DOWN_NFFT2, NMAX, NSPS};

use super::fft::Ft8FftPlans;

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
}
