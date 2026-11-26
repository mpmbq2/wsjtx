// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Clean-room percentile, unweighted 5-term `f64` polynomial fit, and FT8 spectrum baseline.

use num_complex::Complex32;

use crate::ft4::params as ft4_params;
use crate::ft8::params::{NFFT1, NH1, NMAX};

use super::fft::Ft8FftPlans;

/// Exact equivalent of `wsjtx/lib/pctile.f90` using `f32::total_cmp` (no Numerical Recipes `shell`).
pub fn pctile(slice: &[f32], npct: u32) -> f32 {
    if slice.is_empty() || npct > 100 {
        return 1.0;
    }
    let mut tmp = slice.to_vec();
    pctile_in_place(&mut tmp, npct)
}

/// In-place percentile helper using a caller-owned scratch slice.
pub fn pctile_in_place(tmp: &mut [f32], npct: u32) -> f32 {
    let npts = tmp.len();
    if npts == 0 || npct > 100 {
        return 1.0;
    }
    tmp.sort_by(f32::total_cmp);
    let j = ((npts as f32) * 0.01 * (npct as f32)).round() as usize;
    let idx = j.clamp(1, npts) - 1;
    tmp[idx]
}

/// Unweighted (`mode = 0`) 5-term polynomial least-squares fit in `f64` matching
/// `wsjtx/lib/polyfit.f90` + `determ.f90`.
///
/// Fits `y(x) ≈ a[0] + a[1]*x + a[2]*x^2 + a[3]*x^3 + a[4]*x^4`.
/// Returns `[0.0; 5]` if underdetermined or singular.
pub fn polyfit_5(x: &[f64], y: &[f64]) -> [f64; 5] {
    const NTERMS: usize = 5;
    let npts = x.len().min(y.len());
    if npts < NTERMS {
        return [0.0; NTERMS];
    }

    let mut sumx = [0.0_f64; 2 * NTERMS - 1];
    let mut sumy = [0.0_f64; NTERMS];

    for i in 0..npts {
        let xi = x[i];
        let yi = y[i];
        let mut xterm = 1.0_f64;
        for sx in &mut sumx {
            *sx += xterm;
            xterm *= xi;
        }
        let mut yterm = yi;
        for sy in &mut sumy {
            *sy += yterm;
            yterm *= xi;
        }
    }

    let mut mat = [[0.0_f64; NTERMS]; NTERMS];
    for (j, row_mat) in mat.iter_mut().enumerate() {
        row_mat.copy_from_slice(&sumx[j..(NTERMS + j)]);
    }
    let mut rhs = sumy;

    // Gaussian elimination with partial pivoting (clean-room replacement for Cramer + determ).
    for col in 0..NTERMS {
        let mut pivot_row = col;
        let mut pivot_abs = mat[col][col].abs();
        for (row, row_mat) in mat.iter().enumerate().take(NTERMS).skip(col + 1) {
            let v = row_mat[col].abs();
            if v > pivot_abs {
                pivot_abs = v;
                pivot_row = row;
            }
        }
        if !pivot_abs.is_finite() || pivot_abs <= 1e-30 {
            return [0.0; NTERMS];
        }
        if pivot_row != col {
            mat.swap(col, pivot_row);
            rhs.swap(col, pivot_row);
        }

        let diag = mat[col][col];
        for row in (col + 1)..NTERMS {
            let factor = mat[row][col] / diag;
            if factor == 0.0 {
                continue;
            }
            mat[row][col] = 0.0;
            for c in (col + 1)..NTERMS {
                mat[row][c] -= factor * mat[col][c];
            }
            rhs[row] -= factor * rhs[col];
        }
    }

    let mut a = [0.0_f64; NTERMS];
    for i in (0..NTERMS).rev() {
        let mut acc = rhs[i];
        for c in (i + 1)..NTERMS {
            acc -= mat[i][c] * a[c];
        }
        let coeff = acc / mat[i][i];
        if !coeff.is_finite() {
            return [0.0; NTERMS];
        }
        a[i] = coeff;
    }
    a
}

/// Port of `wsjtx/lib/ft8/baseline.f90`: fits a quartic dB baseline (`+ 0.65 dB`) over
/// 1-based bins `[ia..=ib]`.
///
/// `s_linear` and `sbase_db` are 1-based-bin indexed (`len == NH1 + 1 == 1921`, index `0` unused).
pub fn fit_ft8_baseline_db(
    s_linear: &[f32],
    f_low_hz: f32,
    f_high_hz: f32,
    sbase_db: &mut [f32],
) {
    assert!(s_linear.len() > NH1);
    assert!(sbase_db.len() > NH1);

    const NSEG: usize = 10;
    const NPCT: u32 = 10;

    sbase_db[..=NH1].fill(0.0);

    let df = 12_000.0_f32 / (NFFT1 as f32);
    let ia = ((f_low_hz / df).round() as usize).clamp(1, NH1);
    let ib = ((f_high_hz / df).round() as usize).clamp(1, NH1);
    if ib < ia {
        return;
    }

    let span = ib - ia + 1;
    let nlen = span / NSEG;
    if nlen == 0 {
        return;
    }

    // Silence guard: avoid 10*log10(0) poisoning polyfit on all-zero buffers.
    if s_linear[ia..=ib].iter().all(|&v| v <= 0.0 || !v.is_finite()) {
        return;
    }

    let mut s_db = [0.0_f32; NH1 + 1];
    for i in ia..=ib {
        s_db[i] = 10.0 * s_linear[i].max(1e-30).log10();
    }

    // Matches `i0 = (ib - ia + 1) / 2` in `baseline.f90:23`.
    let i0 = (span / 2) as isize;
    let mut x = [0.0_f64; 1000];
    let mut y = [0.0_f64; 1000];
    let mut k = 0usize;
    let mut seg_tmp = vec![0.0_f32; nlen];

    for n in 0..NSEG {
        let ja = ia + n * nlen;
        let jb = ja + nlen - 1;
        seg_tmp.copy_from_slice(&s_db[ja..=jb]);
        let base = pctile_in_place(&mut seg_tmp, NPCT);
        for (i, &val_db) in s_db.iter().enumerate().take(jb + 1).skip(ja) {
            if val_db <= base {
                let slot = k.min(999);
                x[slot] = ((i as isize) - i0) as f64;
                y[slot] = f64::from(val_db);
                if k < 1000 {
                    k += 1;
                }
            }
        }
    }

    let a = polyfit_5(&x[..k], &y[..k]);
    for (i, out_db) in sbase_db.iter_mut().enumerate().take(ib + 1).skip(ia) {
        let t = ((i as isize) - i0) as f64;
        let val = a[0] + t * (a[1] + t * (a[2] + t * (a[3] + t * a[4]))) + 0.65;
        *out_db = val as f32;
    }
}

/// Port of `wsjtx/lib/ft4/ft4_baseline.f90`: fits a quartic dB baseline (`+ 0.65 dB`)
/// over 1-based bins `[ia..=ib]` and converts the fitted baseline back to **linear** power.
///
/// `s_linear` and `sbase_linear` are 1-based-bin indexed (`len == ft4::params::NH1 + 1 == 1153`,
/// index `0` unused). Unlike Fortran, `s_linear` is not mutated in-place.
pub fn fit_ft4_baseline_linear(
    s_linear: &[f32],
    nfa: usize,
    nfb: usize,
    sbase_linear: &mut [f32],
) {
    let nh1 = ft4_params::NH1;
    assert!(s_linear.len() > nh1);
    assert!(sbase_linear.len() > nh1);

    const NSEG: usize = 10;
    const NPCT: u32 = 10;

    sbase_linear[..=nh1].fill(0.0);

    let df = 12_000.0_f32 / (ft4_params::NFFT1 as f32);
    let ia_min = (200.0_f32 / df).round() as usize;
    let ia = ia_min.max(nfa).clamp(1, nh1);
    let ib = nh1.min(nfb);
    if ib < ia {
        return;
    }

    let span = ib - ia + 1;
    let nlen = span / NSEG;
    if nlen == 0 {
        return;
    }

    // Silence guard: avoid 10*log10(0) poisoning polyfit on all-zero / NaN spectra.
    if s_linear[ia..=ib].iter().all(|&v| v <= 0.0 || !v.is_finite()) {
        return;
    }

    let mut s_db = [0.0_f32; ft4_params::NH1 + 1];
    for i in ia..=ib {
        let v = if s_linear[i].is_finite() {
            s_linear[i].max(1e-30)
        } else {
            1e-30
        };
        s_db[i] = 10.0 * v.log10();
    }

    // Matches `i0 = (ib - ia + 1) / 2` in `ft4_baseline.f90`.
    let i0 = (span / 2) as isize;
    let mut x = [0.0_f64; 1000];
    let mut y = [0.0_f64; 1000];
    let mut k = 0usize;
    let mut seg_tmp = vec![0.0_f32; nlen];

    for n in 0..NSEG {
        let ja = ia + n * nlen;
        let jb = ja + nlen - 1;
        seg_tmp.copy_from_slice(&s_db[ja..=jb]);
        let base = pctile_in_place(&mut seg_tmp, NPCT);
        for (i, &val_db) in s_db.iter().enumerate().take(jb + 1).skip(ja) {
            if val_db <= base {
                let slot = k.min(999);
                x[slot] = ((i as isize) - i0) as f64;
                y[slot] = f64::from(val_db);
                if k < 1000 {
                    k += 1;
                }
            }
        }
    }

    let a = polyfit_5(&x[..k], &y[..k]);
    for (i, out_lin) in sbase_linear.iter_mut().enumerate().take(ib + 1).skip(ia) {
        let t = ((i as isize) - i0) as f64;
        let val_db = (a[0] + t * (a[1] + t * (a[2] + t * (a[3] + t * a[4]))) + 0.65) as f32;
        *out_lin = 10.0_f32.powf(0.1 * val_db);
    }
}

/// Scratch buffers for `compute_ft8_spectrum_baseline` (zero per-call heap allocations).
pub struct BaselineScratch {
    savg: Vec<f32>,
    r2c_in: Vec<f32>,
    r2c_out: Vec<Complex32>,
    r2c_scratch: Vec<Complex32>,
}

impl BaselineScratch {
    pub fn new(plans: &Ft8FftPlans) -> Self {
        Self {
            savg: vec![0.0; NH1 + 1],
            r2c_in: plans.r2c_3840.make_input_vec(),
            r2c_out: plans.r2c_3840.make_output_vec(),
            r2c_scratch: plans.r2c_3840.make_scratch_vec(),
        }
    }
}

/// Clamps `(f_low_hz, f_high_hz)` to `[100, 4910]` preserving narrow (`< 100 Hz`) spans
/// exactly as `wsjtx/lib/ft8/get_spectrum_baseline.f90:38-50` mutates `nfa`/`nfb`.
pub fn clamp_ft8_freq_bounds(f_low_hz: f32, f_high_hz: f32) -> (f32, f32) {
    let mut nfa = f_low_hz.round() as i32;
    let mut nfb = f_high_hz.round() as i32;
    let nwin = nfb - nfa;
    if nfa < 100 {
        nfa = 100;
        if nwin < 100 {
            nfb = nfa + nwin;
        }
    }
    if nfb > 4910 {
        nfb = 4910;
        if nwin < 100 {
            nfa = nfb - nwin;
        }
    }
    (nfa as f32, nfb as f32)
}

/// Builds the scaled periodic Nuttall window (`window / sum(window) * 3840.0 / 300.0`)
/// matching `get_spectrum_baseline.f90:21-22`.
pub fn build_scaled_baseline_window() -> Vec<f32> {
    let mut win = super::window::nuttall_window(NFFT1);
    let sum: f32 = win.iter().sum();
    let scale = (NFFT1 as f32) / (sum * 300.0);
    for w in &mut win {
        *w *= scale;
    }
    win
}

/// Port of `wsjtx/lib/ft8/get_spectrum_baseline.f90`.
///
/// Returns clamped `(f_low_hz, f_high_hz)` matching Fortran's in-place `nfa`/`nfb` clamping.
pub fn compute_ft8_spectrum_baseline(
    dd: &[f32; NMAX],
    f_low_hz: f32,
    f_high_hz: f32,
    plans: &Ft8FftPlans,
    nuttall_scaled: &[f32],
    scratch: &mut BaselineScratch,
    sbase_db: &mut [f32; NH1 + 1],
) -> (f32, f32) {
    assert_eq!(nuttall_scaled.len(), NFFT1);

    const NST: usize = NFFT1 / 2;
    const NF: usize = 93;

    scratch.savg.fill(0.0);
    for j in 0..NF {
        let ia = j * NST;
        let ib = ia + NFFT1;
        if ib > NMAX {
            break;
        }
        for ((dst, &d), &w) in scratch
            .r2c_in
            .iter_mut()
            .zip(dd[ia..ib].iter())
            .zip(nuttall_scaled.iter())
        {
            *dst = d * w;
        }
        plans
            .r2c_3840
            .process_with_scratch(
                &mut scratch.r2c_in,
                &mut scratch.r2c_out,
                &mut scratch.r2c_scratch,
            )
            .expect("r2c_3840 scratch sized");

        for bin in 1..=NH1 {
            scratch.savg[bin] += scratch.r2c_out[bin].norm_sqr();
        }
    }

    let (clamped_low, clamped_high) = clamp_ft8_freq_bounds(f_low_hz, f_high_hz);
    fit_ft8_baseline_db(&scratch.savg, clamped_low, clamped_high, sbase_db);
    (clamped_low, clamped_high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pctile_matches_fortran_edge_bounds() {
        let vals = [5.0_f32, 1.0, 4.0, 2.0, 3.0];
        assert_eq!(pctile(&vals, 0), 1.0);
        assert_eq!(pctile(&vals, 10), 1.0);
        // `5.0_f32 * 0.01_f32 * 50.0_f32` = `2.4999998`, matching Fortran `nint(5*0.01*50) = 2`.
        assert_eq!(pctile(&vals, 50), 2.0);
        assert_eq!(pctile(&vals, 100), 5.0);
        assert_eq!(pctile(&[], 50), 1.0);
        assert_eq!(pctile(&vals, 101), 1.0);

        let vals9 = [9.0_f32, 1.0, 8.0, 2.0, 7.0, 3.0, 6.0, 4.0, 5.0];
        assert_eq!(pctile(&vals9, 50), 5.0);
    }

    #[test]
    fn polyfit_5_recovers_exact_quartic() {
        let true_a = [12.5, -0.35, 0.02, -0.0004, 1.5e-6];
        let mut x = Vec::new();
        let mut y = Vec::new();
        for k in -20..=20 {
            let xv = k as f64;
            let yv = true_a[0]
                + xv * (true_a[1] + xv * (true_a[2] + xv * (true_a[3] + xv * true_a[4])));
            x.push(xv);
            y.push(yv);
        }
        let fit = polyfit_5(&x, &y);
        for i in 0..5 {
            assert!(
                (fit[i] - true_a[i]).abs() < 1e-10,
                "coeff {i}: got {}, expected {}",
                fit[i],
                true_a[i]
            );
        }
        // Singular / underdetermined input returns zeros cleanly.
        assert_eq!(polyfit_5(&[1.0, 2.0], &[3.0, 4.0]), [0.0; 5]);
        assert_eq!(polyfit_5(&[1.0; 10], &[5.0; 10]), [0.0; 5]);
    }

    #[test]
    fn silence_baseline_produces_no_nans() {
        let plans = Ft8FftPlans::new();
        let win = build_scaled_baseline_window();
        let mut scratch = BaselineScratch::new(&plans);
        let dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        let mut sbase = [0.0_f32; NH1 + 1];
        let (lo, hi) =
            compute_ft8_spectrum_baseline(&dd, 50.0, 5500.0, &plans, &win, &mut scratch, &mut sbase);
        assert_eq!((lo, hi), (100.0, 4910.0));
        assert!(sbase.iter().all(|&v| v == 0.0 && v.is_finite()));
    }

    #[test]
    fn ft4_baseline_tracks_sloped_floor_and_handles_edge_cases() {
        let nh1 = ft4_params::NH1;
        let nfa = 38usize;
        let nfb = 943usize;
        let mut savg = vec![0.0_f32; nh1 + 1];
        for (i, val) in savg.iter_mut().enumerate().take(nfb + 1).skip(nfa) {
            // Smooth tilted baseline in dB (~ +20 dB at nfa to +15 dB at nfb)
            let frac = (i - nfa) as f32 / (nfb - nfa) as f32;
            let db = 20.0 - 5.0 * frac + 1.5 * (frac * std::f32::consts::PI).sin();
            *val = 10.0_f32.powf(0.1 * db);
        }
        // Inject strong narrow spikes (FT4 signals) that must not pull up the 10%-ile baseline
        for &peak_bin in &[150usize, 320, 610, 820] {
            for v in &mut savg[(peak_bin - 2)..=(peak_bin + 2)] {
                *v *= 100.0;
            }
        }
        let mut sbase = vec![0.0_f32; nh1 + 1];
        fit_ft4_baseline_linear(&savg, nfa, nfb, &mut sbase);

        // Expect linear baseline near truth * 10^(+0.65/10) ≈ truth * 1.1614
        let expected_scale = 10.0_f32.powf(0.065);
        for &check_bin in &[150usize, 320, 610, 820] {
            let frac = (check_bin - nfa) as f32 / (nfb - nfa) as f32;
            let true_db = 20.0 - 5.0 * frac + 1.5 * (frac * std::f32::consts::PI).sin();
            let expected = 10.0_f32.powf(0.1 * true_db) * expected_scale;
            let rel_err = (sbase[check_bin] - expected).abs() / expected;
            assert!(
                rel_err < 0.05,
                "bin {check_bin}: got {}, expected {expected} (rel_err={rel_err})",
                sbase[check_bin]
            );
        }
        // Bins outside [nfa..=nfb] stay zero
        assert!(sbase[1..nfa].iter().all(|&v| v == 0.0));
        assert!(sbase[(nfb + 1)..=nh1].iter().all(|&v| v == 0.0));

        // All-zero / silent spectrum and invalid/degenerate range guards produce zeros without NaNs
        let zero_spec = vec![0.0_f32; nh1 + 1];
        fit_ft4_baseline_linear(&zero_spec, nfa, nfb, &mut sbase);
        assert!(sbase.iter().all(|&v| v == 0.0 && v.is_finite()));

        fit_ft4_baseline_linear(&savg, 500, 100, &mut sbase);
        assert!(sbase.iter().all(|&v| v == 0.0 && v.is_finite()));

        fit_ft4_baseline_linear(&savg, 0, 0, &mut sbase);
        assert!(sbase.iter().all(|&v| v == 0.0 && v.is_finite()));
    }
}
