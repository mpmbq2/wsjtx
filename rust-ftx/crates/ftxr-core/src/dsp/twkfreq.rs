// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Complex Legendre-polynomial carrier frequency/phase mixing (`twkfreq1`).

use std::f32::consts::TAU;

use num_complex::Complex32;

/// Port of `wsjtx/lib/ft8/twkfreq1.f90`: applies a Legendre-polynomial frequency/phase
/// shift specified by `a[0..5]` (Hz and polynomial drift terms) to a complex baseband buffer.
pub fn twkfreq1(input: &[Complex32], fsample: f32, a: &[f32; 5], output: &mut [Complex32]) {
    let npts = input.len();
    assert_eq!(output.len(), npts);
    if npts == 0 || fsample <= 0.0 {
        return;
    }

    let mut w = Complex32::new(1.0, 0.0);
    let x0 = 0.5 * ((npts + 1) as f32);
    let s = 2.0 / (npts as f32);
    let scale = TAU / fsample;

    for (idx, (&src, dst)) in input.iter().zip(output.iter_mut()).enumerate() {
        let i_1based = (idx + 1) as f32;
        let x = s * (i_1based - x0);
        let x2 = x * x;
        let p2 = 1.5 * x2 - 0.5;
        let p3 = 2.5 * x2 * x - 1.5 * x;
        let p4 = 4.375 * x2 * x2 - 3.75 * x2 + 0.375;
        let dphi = (a[0] + x * a[1] + p2 * a[2] + p3 * a[3] + p4 * a[4]) * scale;
        let wstep = Complex32::new(dphi.cos(), dphi.sin());
        w *= wstep;
        *dst = w * src;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twkfreq1_shifts_tone_accurately() {
        let fs = 200.0_f32;
        let npts = 2812usize;
        let f_in = 12.5_f32;
        let shift = -2.5_f32;
        let input: Vec<Complex32> = (0..npts)
            .map(|k| {
                let phi = TAU * f_in * (k as f32) / fs;
                Complex32::new(phi.cos(), phi.sin())
            })
            .collect();
        let mut output = vec![Complex32::ZERO; npts];
        twkfreq1(&input, fs, &[shift, 0.0, 0.0, 0.0, 0.0], &mut output);

        // Compare against ideal exp(j * 2*pi * (f_in + shift) * k / fs) up to constant start phase.
        let base_phase = TAU * shift / fs;
        let mut max_err = 0.0_f32;
        for (k, &val) in output.iter().enumerate() {
            let expected_phi = TAU * (f_in + shift) * (k as f32) / fs + base_phase;
            let exp_c = Complex32::new(expected_phi.cos(), expected_phi.sin());
            max_err = max_err.max((val - exp_c).norm());
        }
        assert!(max_err < 1e-3, "max phase-shifted error {max_err}");
    }
}
