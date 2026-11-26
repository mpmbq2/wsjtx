// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Reusable FFT plan cache and scratch helpers (`rustfft` / `realfft`).

use std::fmt;
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};
use rustfft::{Fft, FftPlanner};

use crate::ft4::params as ft4_params;
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

/// Immutable, thread-safe collection of pre-planned 7-smooth FT4 FFTs.
///
/// Shared inside `Ft4Decoder` / `DecoderEngine` for zero per-slot replanning.
#[derive(Clone)]
pub struct Ft4FftPlans {
    pub r2c_2304: Arc<dyn RealToComplex<f32>>,
    pub r2c_72576: Arc<dyn RealToComplex<f32>>,
    pub c2c_inv_4032: Arc<dyn Fft<f32>>,
    pub c2c_fwd_32: Arc<dyn Fft<f32>>,
    pub c2c_fwd_72576: Arc<dyn Fft<f32>>,
    pub c2c_inv_72576: Arc<dyn Fft<f32>>,
}

impl fmt::Debug for Ft4FftPlans {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ft4FftPlans")
            .field("r2c_2304_scratch", &self.r2c_2304.get_scratch_len())
            .field("r2c_72576_scratch", &self.r2c_72576.get_scratch_len())
            .field(
                "c2c_inv_4032_scratch",
                &self.c2c_inv_4032.get_inplace_scratch_len(),
            )
            .field(
                "c2c_fwd_32_scratch",
                &self.c2c_fwd_32.get_inplace_scratch_len(),
            )
            .field(
                "c2c_fwd_72576_scratch",
                &self.c2c_fwd_72576.get_inplace_scratch_len(),
            )
            .field(
                "c2c_inv_72576_scratch",
                &self.c2c_inv_72576.get_inplace_scratch_len(),
            )
            .finish()
    }
}

impl Default for Ft4FftPlans {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4FftPlans {
    /// Plans all 7-smooth FT4 FFT transforms once.
    pub fn new() -> Self {
        let mut r2c = RealFftPlanner::<f32>::new();
        let mut c2c = FftPlanner::<f32>::new();

        Self {
            r2c_2304: r2c.plan_fft_forward(ft4_params::NFFT1),
            r2c_72576: r2c.plan_fft_forward(ft4_params::NMAX),
            c2c_inv_4032: c2c.plan_fft_inverse(ft4_params::NDMAX),
            c2c_fwd_32: c2c.plan_fft_forward(ft4_params::NSS),
            c2c_fwd_72576: c2c.plan_fft_forward(ft4_params::NMAX),
            c2c_inv_72576: c2c.plan_fft_inverse(ft4_params::NMAX),
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

    #[test]
    fn ft4_fft_plans_roundtrip_accuracy() {
        let plans = Ft4FftPlans::new();

        // 72,576-point C2C forward + inverse roundtrip
        let n72 = ft4_params::NMAX;
        let orig72: Vec<Complex32> = (0..n72)
            .map(|i| {
                let p = (i as f32) * 0.0317;
                Complex32::new(p.cos() * 0.7, (p * 1.3).sin() * 0.4)
            })
            .collect();
        let mut work72 = orig72.clone();
        let mut scratch72 = vec![
            Complex32::ZERO;
            plans
                .c2c_fwd_72576
                .get_inplace_scratch_len()
                .max(plans.c2c_inv_72576.get_inplace_scratch_len())
        ];
        plans.c2c_fwd_72576.process_with_scratch(&mut work72, &mut scratch72);
        plans.c2c_inv_72576.process_with_scratch(&mut work72, &mut scratch72);
        let mut num_err = 0.0_f64;
        let mut denom = 0.0_f64;
        for (a, b) in orig72.iter().zip(work72.iter()) {
            let rec = *b / (n72 as f32);
            let d = (a - rec).norm() as f64;
            num_err += d * d;
            let n = a.norm() as f64;
            denom += n * n;
        }
        let rel_l2_72 = (num_err / denom).sqrt();
        assert!(rel_l2_72 < 1e-6, "72576 c2c rel_l2={rel_l2_72}");

        // 72,576-point R2C bin check for real cosine
        let mut r72: Vec<f32> = (0..n72)
            .map(|i| (std::f64::consts::TAU * 313.0 * (i as f64) / (n72 as f64)).cos() as f32)
            .collect();
        let mut r72_out = plans.r2c_72576.make_output_vec();
        let mut r72_scratch = plans.r2c_72576.make_scratch_vec();
        plans
            .r2c_72576
            .process_with_scratch(&mut r72, &mut r72_out, &mut r72_scratch)
            .unwrap();
        assert!((r72_out[313].re - (n72 as f32) * 0.5).abs() < 1e-1);
        assert!(r72_out[313].im.abs() < 1e-1);

        // 4,032-point C2C inverse transforms impulse at bin k0 -> complex sinusoid exp(+j*2*pi*k0*n/N)
        let n4 = ft4_params::NDMAX;
        let k0 = 37usize;
        let mut buf4 = vec![Complex32::ZERO; n4];
        buf4[k0] = Complex32::new(n4 as f32, 0.0);
        let mut s4 = vec![Complex32::ZERO; plans.c2c_inv_4032.get_inplace_scratch_len()];
        plans.c2c_inv_4032.process_with_scratch(&mut buf4, &mut s4);
        let mut num4 = 0.0_f64;
        for (idx, &val) in buf4.iter().enumerate() {
            let phi = std::f64::consts::TAU * (k0 as f64) * (idx as f64) / (n4 as f64);
            let exp_val = Complex32::new(phi.cos() as f32, phi.sin() as f32);
            let d = (val / (n4 as f32) - exp_val).norm() as f64;
            num4 += d * d;
        }
        let rel_l2_4 = (num4 / (n4 as f64)).sqrt();
        assert!(rel_l2_4 < 1e-6, "4032 c2c_inv rel_l2={rel_l2_4}");
    }
}
