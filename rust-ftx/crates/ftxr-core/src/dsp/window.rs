// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Analysis and filter windows for spectrograms and signal subtraction.

use std::f32::consts::PI;

/// Periodic Nuttall window matching `wsjtx/lib/nuttal_window.f90` (denominator `n`, not `n - 1`).
pub fn nuttall_window(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    const A0: f32 = 0.3635819;
    const A1: f32 = -0.4891775;
    const A2: f32 = 0.1365995;
    const A3: f32 = -0.0106411;

    let n_f = n as f32;
    let mut win = Vec::with_capacity(n);
    for i in 0..n {
        let phase = 2.0 * PI * (i as f32) / n_f;
        let w = A0 + A1 * phase.cos() + A2 * (2.0 * phase).cos() + A3 * (3.0 * phase).cos();
        win.push(w);
    }
    win
}

/// `cos(pi * j / nfilt)^2` low-pass filter taps for `j = -nfilt/2..=+nfilt/2`
/// (length `nfilt + 1`), returning `(taps, sumw)` matching `lib/ft8/subtractft8.f90:29-32`.
pub fn cos2_lpf_window(nfilt: usize) -> (Vec<f32>, f32) {
    let half = (nfilt / 2) as isize;
    let nfilt_f = nfilt as f32;
    let mut taps = Vec::with_capacity(nfilt + 1);
    let mut sumw = 0.0_f32;
    for j in -half..=half {
        let c = (PI * (j as f32) / nfilt_f).cos();
        let w = c * c;
        sumw += w;
        taps.push(w);
    }
    (taps, sumw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuttall_is_periodic_and_symmetric() {
        let w = nuttall_window(3840);
        assert_eq!(w.len(), 3840);
        assert!(w[0].abs() < 1e-3);
        assert!((w[1920] - 1.0).abs() < 1e-4);
        assert!((w[1] - w[3839]).abs() < 1e-6);
        assert!((w[500] - w[3840 - 500]).abs() < 1e-6);
    }

    #[test]
    fn cos2_window_matches_subtractft8_sumw() {
        let (w, sumw) = cos2_lpf_window(4000);
        assert_eq!(w.len(), 4001);
        assert!(w[0].abs() < 1e-6);
        assert!((w[2000] - 1.0).abs() < 1e-6);
        assert!(w[4000].abs() < 1e-6);
        assert!((sumw - 2000.0).abs() < 1e-3);
    }
}
