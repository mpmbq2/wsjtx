// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Reusable FFT plan cache and scratch helpers (`rustfft` / `realfft`).

use std::fmt;
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};
use rustfft::{Fft, FftPlanner};

use crate::ft8::params::{DOWN_NFFT1, DOWN_NFFT2, NFFT1, SUB_NFFT};

/// Immutable, thread-safe collection of pre-planned FT8 FFTs.
///
/// Held inside `Ft8Decoder` / `DecoderEngine` so plans stay warm across slots with
/// zero cold-start replanning and zero per-slot allocations.
#[derive(Clone)]
pub struct Ft8FftPlans {
    pub r2c_3840: Arc<dyn RealToComplex<f32>>,
    pub r2c_192000: Arc<dyn RealToComplex<f32>>,
    pub c2c_inv_3200: Arc<dyn Fft<f32>>,
    pub c2c_fwd_32: Arc<dyn Fft<f32>>,
    pub c2c_fwd_180000: Arc<dyn Fft<f32>>,
    pub c2c_inv_180000: Arc<dyn Fft<f32>>,
    pub r2c_180000: Arc<dyn RealToComplex<f32>>,
}

impl fmt::Debug for Ft8FftPlans {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ft8FftPlans")
            .field("r2c_3840_scratch", &self.r2c_3840.get_scratch_len())
            .field("r2c_192000_scratch", &self.r2c_192000.get_scratch_len())
            .field("c2c_inv_3200_scratch", &self.c2c_inv_3200.get_inplace_scratch_len())
            .field("c2c_fwd_32_scratch", &self.c2c_fwd_32.get_inplace_scratch_len())
            .field("c2c_fwd_180000_scratch", &self.c2c_fwd_180000.get_inplace_scratch_len())
            .field("c2c_inv_180000_scratch", &self.c2c_inv_180000.get_inplace_scratch_len())
            .field("r2c_180000_scratch", &self.r2c_180000.get_scratch_len())
            .finish()
    }
}

impl Default for Ft8FftPlans {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft8FftPlans {
    /// Plans all 7 smooth FT8 FFT transforms once.
    pub fn new() -> Self {
        let mut r2c = RealFftPlanner::<f32>::new();
        let mut c2c = FftPlanner::<f32>::new();

        Self {
            r2c_3840: r2c.plan_fft_forward(NFFT1),
            r2c_192000: r2c.plan_fft_forward(DOWN_NFFT1),
            c2c_inv_3200: c2c.plan_fft_inverse(DOWN_NFFT2),
            c2c_fwd_32: c2c.plan_fft_forward(32),
            c2c_fwd_180000: c2c.plan_fft_forward(SUB_NFFT),
            c2c_inv_180000: c2c.plan_fft_inverse(SUB_NFFT),
            r2c_180000: r2c.plan_fft_forward(SUB_NFFT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex32;
    use std::f32::consts::TAU;

    #[test]
    fn fft_sign_convention_matches_wsjtx_four2a() {
        let plans = Ft8FftPlans::new();

        // Forward c2c: exp(+j * 2*pi * k0 * n / N) should peak at bin +k0.
        let n = 32usize;
        let k0 = 5usize;
        let mut buf: Vec<Complex32> = (0..n)
            .map(|idx| {
                let phi = TAU * (k0 as f32) * (idx as f32) / (n as f32);
                Complex32::new(phi.cos(), phi.sin())
            })
            .collect();
        let mut scratch = vec![Complex32::ZERO; plans.c2c_fwd_32.get_inplace_scratch_len()];
        plans.c2c_fwd_32.process_with_scratch(&mut buf, &mut scratch);

        assert!((buf[k0].re - (n as f32)).abs() < 1e-4);
        assert!(buf[k0].im.abs() < 1e-4);
        for (k, &val) in buf.iter().enumerate() {
            if k != k0 {
                assert!(val.norm() < 1e-4, "unexpected energy at bin {k}: {val:?}");
            }
        }

        // Forward r2c 3840 + unnormalized convention check.
        let mut r_in: Vec<f32> = (0..NFFT1)
            .map(|idx| {
                (std::f64::consts::TAU * 100.0 * (idx as f64) / (NFFT1 as f64)).cos() as f32
            })
            .collect();
        let mut r_out = plans.r2c_3840.make_output_vec();
        let mut r_scratch = plans.r2c_3840.make_scratch_vec();
        plans
            .r2c_3840
            .process_with_scratch(&mut r_in, &mut r_out, &mut r_scratch)
            .unwrap();
        assert!((r_out[100].re - (NFFT1 as f32) * 0.5).abs() < 1e-2);
        assert!(r_out[100].im.abs() < 1e-2);
    }
}
