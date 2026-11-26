// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 spectral baseline normalization and candidate discovery (`getcandidates4.f90`).

use num_complex::Complex32;

use crate::dsp::baseline::fit_ft4_baseline_linear;
use crate::dsp::fft::Ft4FftPlans;
use crate::dsp::window::nuttall_window;

use super::params::{
    DF_SPECTRUM, F_OFFSET_HZ, MAXCAND, MAX_FREQ_HZ, MIN_FREQ_HZ, NFFT1, NFQSO_PRIOR_HZ, NH1,
    NHSYM, NMAX, NSTEP, SYNCMIN,
};

/// Candidate signal peak discovered by `getcandidates4`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ft4Candidate {
    /// Estimated base (lowest-tone) audio frequency in Hz.
    pub freq_hz: f32,
    /// Smoothed power ratio relative to local fitted baseline (`savsm / sbase`).
    pub snr_ratio: f32,
}

/// Precomputed window weights for `getcandidates4`.
#[derive(Debug, Clone)]
pub struct Ft4CandidatePlan {
    nuttall_scaled: Vec<f32>,
}

impl Default for Ft4CandidatePlan {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4CandidatePlan {
    pub fn new() -> Self {
        let mut nuttall_scaled = nuttall_window(NFFT1);
        let fac = 1.0_f32 / 300.0;
        for w in &mut nuttall_scaled {
            *w *= fac;
        }
        Self { nuttall_scaled }
    }
}

/// Reusable scratch buffers for `find_candidates` (zero per-pass allocations).
pub struct Ft4CandidateScratch {
    savg: Vec<f32>,
    savsm: Vec<f32>,
    sbase: Vec<f32>,
    r2c_in: Vec<f32>,
    r2c_out: Vec<Complex32>,
    r2c_scratch: Vec<Complex32>,
}

impl Ft4CandidateScratch {
    pub fn new(plans: &Ft4FftPlans) -> Self {
        Self {
            savg: vec![0.0; NH1 + 1],
            savsm: vec![0.0; NH1 + 1],
            sbase: vec![0.0; NH1 + 1],
            r2c_in: plans.r2c_2304.make_input_vec(),
            r2c_out: plans.r2c_2304.make_output_vec(),
            r2c_scratch: plans.r2c_2304.make_scratch_vec(),
        }
    }

    /// Mean linear power spectrum (`1..=1152`) prior to baseline flattening.
    pub fn savg(&self) -> &[f32] {
        &self.savg
    }

    /// Normalized 15-bin smoothed power ratio (`savsm / sbase`, `1..=1152`).
    pub fn savsm_normalized(&self) -> &[f32] {
        &self.savsm
    }

    /// Linear fitted baseline (`1..=1152`).
    pub fn sbase(&self) -> &[f32] {
        &self.sbase
    }
}

/// Pure-Rust port of `wsjtx/lib/ft4/getcandidates4.f90`.
///
/// Computes the 122-frame spectrogram over `dd[0..72_576]`, normalizes by the 15-bin smoothed
/// ratio over the linear baseline, extracts up to `MAXCAND = 200` peaks above `SYNCMIN = 1.18`,
/// and prioritizes candidates within `±20 Hz` of `nfqso_hz` ahead of the remaining candidates
/// while preserving ascending frequency order within each partition.
pub fn find_candidates(
    dd: &[f32; NMAX],
    f_low_hz: f32,
    f_high_hz: f32,
    nfqso_hz: f32,
    plans: &Ft4FftPlans,
    cand_plan: &Ft4CandidatePlan,
    scratch: &mut Ft4CandidateScratch,
) -> Vec<Ft4Candidate> {
    scratch.savg.fill(0.0);
    scratch.savsm.fill(0.0);
    scratch.sbase.fill(0.0);

    let df = DF_SPECTRUM;
    let fa_safe = if f_low_hz.is_finite() {
        f_low_hz.max(0.0)
    } else {
        MIN_FREQ_HZ
    };
    let fb_safe = if f_high_hz.is_finite() {
        f_high_hz.max(0.0)
    } else {
        MAX_FREQ_HZ
    };
    if fb_safe < fa_safe {
        return Vec::new();
    }

    let min_bin = (MIN_FREQ_HZ / df).round() as usize; // 38
    let max_bin = (MAX_FREQ_HZ / df).round() as usize; // 943
    let nfa = ((fa_safe / df) as usize).max(min_bin).min(NH1);
    let nfb = ((fb_safe / df) as usize).min(max_bin);
    if nfb < nfa + 2 {
        return Vec::new();
    }

    // 1. Average 122 overlapping 2304-point Nuttall spectrograms.
    for j in 0..NHSYM {
        let ia = j * NSTEP;
        let ib = ia + NFFT1;
        debug_assert!(ib <= NMAX);
        for ((dst, &d), &w) in scratch
            .r2c_in
            .iter_mut()
            .zip(dd[ia..ib].iter())
            .zip(cand_plan.nuttall_scaled.iter())
        {
            *dst = d * w;
        }
        plans
            .r2c_2304
            .process_with_scratch(
                &mut scratch.r2c_in,
                &mut scratch.r2c_out,
                &mut scratch.r2c_scratch,
            )
            .expect("r2c_2304 scratch sized");

        for bin in 1..=NH1 {
            scratch.savg[bin] += scratch.r2c_out[bin].norm_sqr();
        }
    }
    let inv_nhsym = 1.0_f32 / (NHSYM as f32);
    for v in &mut scratch.savg[1..=NH1] {
        *v *= inv_nhsym;
    }

    // 2. 15-bin boxcar smooth before baseline fitting (matches getcandidates4.f90:39-41).
    for i in 8..=(NH1 - 7) {
        let sum: f32 = scratch.savg[(i - 7)..=(i + 7)].iter().sum();
        scratch.savsm[i] = sum / 15.0;
    }

    // 3. Linear quartic baseline fit and ratio normalization.
    fit_ft4_baseline_linear(&scratch.savg, nfa, nfb, &mut scratch.sbase);
    if scratch.sbase[nfa..=nfb]
        .iter()
        .any(|&v| v <= 0.0 || !v.is_finite())
    {
        return Vec::new();
    }
    for i in nfa..=nfb {
        scratch.savsm[i] /= scratch.sbase[i];
    }

    // 4. Local maxima search with parabolic frequency interpolation.
    let mut raw = Vec::with_capacity(32);
    for i in (nfa + 1)..=(nfb - 1) {
        let prev = scratch.savsm[i - 1];
        let curr = scratch.savsm[i];
        let next = scratch.savsm[i + 1];
        if curr >= prev && curr >= next && curr >= SYNCMIN {
            let den = prev - 2.0 * curr + next;
            let del = if den != 0.0 {
                0.5 * (prev - next) / den
            } else {
                0.0
            };
            let fpeak = ((i as f32) + del) * df + F_OFFSET_HZ;
            if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&fpeak) {
                continue;
            }
            let speak = curr - 0.25 * (prev - next) * del;
            raw.push(Ft4Candidate {
                freq_hz: fpeak,
                snr_ratio: speak,
            });
            if raw.len() >= MAXCAND {
                break;
            }
        }
    }

    // 5. Partition: candidates within ±20 Hz of nfqso_hz first (keeping ascending freq order).
    let mut prioritized = Vec::with_capacity(raw.len());
    for cand in &raw {
        if (cand.freq_hz - nfqso_hz).abs() <= NFQSO_PRIOR_HZ {
            prioritized.push(*cand);
        }
    }
    for cand in &raw {
        if (cand.freq_hz - nfqso_hz).abs() > NFQSO_PRIOR_HZ {
            prioritized.push(*cand);
        }
    }
    prioritized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulate::ft4::{encode_ft4_tones_from_77bits, gen_ft4_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn candidates_detects_clean_signal_and_partitions_near_nfqso() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let p1 = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let p2 = pack77("CQ W9XYZ EN52", None, &ctx, &ht).unwrap();
        let w1 = gen_ft4_wave_real(&encode_ft4_tones_from_77bits(&p1.bits), 900.0, 12_000);
        let w2 = gen_ft4_wave_real(&encode_ft4_tones_from_77bits(&p2.bits), 1850.0, 12_000);

        // Add subtle white-like deterministic background floor + both signals at t=0.5s.
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        let mut state = 0x1234_5678_9abc_def0_u64;
        for sample in dd.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((state >> 33) as f32) / (u32::MAX as f32) - 0.5;
            *sample = 0.05 * u;
        }
        for i in 0..w1.len() {
            dd[6000 + i] += w1[i] + w2[i];
        }

        let plans = Ft4FftPlans::new();
        let cand_plan = Ft4CandidatePlan::new();
        let mut scratch = Ft4CandidateScratch::new(&plans);
        let cands = find_candidates(&dd, 200.0, 4910.0, 1845.0, &plans, &cand_plan, &mut scratch);

        assert!(
            cands.iter().any(|c| (c.freq_hz - 900.0).abs() <= 5.0),
            "expected candidate near 900 Hz, got {cands:?}"
        );
        assert!(
            cands.iter().any(|c| (c.freq_hz - 1850.0).abs() <= 5.0),
            "expected candidate near 1850 Hz, got {cands:?}"
        );
        // Priority candidate near 1845 Hz must come before 900 Hz!
        let pos_1850 = cands
            .iter()
            .position(|c| (c.freq_hz - 1850.0).abs() <= 5.0)
            .unwrap();
        let pos_900 = cands
            .iter()
            .position(|c| (c.freq_hz - 900.0).abs() <= 5.0)
            .unwrap();
        assert!(pos_1850 < pos_900, "prioritization failed: {cands:?}");
    }

    #[test]
    fn candidates_silence_produces_empty_without_nans() {
        let plans = Ft4FftPlans::new();
        let cand_plan = Ft4CandidatePlan::new();
        let mut scratch = Ft4CandidateScratch::new(&plans);
        let dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        let cands = find_candidates(&dd, 200.0, 4910.0, 1500.0, &plans, &cand_plan, &mut scratch);
        assert!(cands.is_empty());
        assert!(scratch.savg().iter().all(|v| v.is_finite()));
        assert!(scratch.sbase().iter().all(|v| v.is_finite()));
        assert!(scratch.savsm_normalized().iter().all(|v| v.is_finite()));
    }
}
