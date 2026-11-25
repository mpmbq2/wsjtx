// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 spectrogram and 7×7 Costas synchronization candidate search (`sync8.f90`).

use num_complex::Complex32;

use crate::dsp::baseline::{
    build_scaled_baseline_window, compute_ft8_spectrum_baseline, BaselineScratch,
};
use crate::dsp::fft::Ft8FftPlans;

use super::params::{ICOS7, NFFT1, NH1, NHSYM, NMAX, NSPS, NSTEP};

/// Maximum pre-deduplication candidates extracted from `red`/`red2`.
pub const MAX_PRECAND: usize = 1000;
/// Maximum lag steps (±2.5 s at 0.04 s/step).
pub const JZ: isize = 62;
const LAG_COUNT: usize = (2 * JZ + 1) as usize; // 125

/// Soft-bit metric mode used in `ft8b` (`imetric = 1` vs `2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BitMetricMode {
    Amplitude = 1,
    Power = 2,
}

/// Candidate signal peak discovered by `sync8`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ft8Candidate {
    pub freq_hz: f32,
    /// Relative to nominal 0.5 s TX start (`(jpeak - 0.5) * 0.04`).
    pub dt_sec: f32,
    pub sync: f32,
}

/// Reusable scratch buffers for `sync8` (zero per-pass allocations).
pub struct Sync8Scratch {
    s: Vec<f32>,      // NHSYM * (NH1 + 1)
    sync2d: Vec<f32>, // (NH1 + 1) * LAG_COUNT
    red: Vec<f32>,    // NH1 + 1
    red2: Vec<f32>,   // NH1 + 1
    jpeak: Vec<isize>,
    jpeak2: Vec<isize>,
    r2c_in: Vec<f32>,
    r2c_out: Vec<Complex32>,
    r2c_scratch: Vec<Complex32>,
    baseline_scratch: BaselineScratch,
    nuttall_scaled: Vec<f32>,
    sbase_db: [f32; NH1 + 1],
}

impl Sync8Scratch {
    pub fn new(plans: &Ft8FftPlans) -> Self {
        Self {
            s: vec![0.0; NHSYM * (NH1 + 1)],
            sync2d: vec![0.0; (NH1 + 1) * LAG_COUNT],
            red: vec![0.0; NH1 + 1],
            red2: vec![0.0; NH1 + 1],
            jpeak: vec![0; NH1 + 1],
            jpeak2: vec![0; NH1 + 1],
            r2c_in: plans.r2c_3840.make_input_vec(),
            r2c_out: plans.r2c_3840.make_output_vec(),
            r2c_scratch: plans.r2c_3840.make_scratch_vec(),
            baseline_scratch: BaselineScratch::new(plans),
            nuttall_scaled: build_scaled_baseline_window(),
            sbase_db: [0.0; NH1 + 1],
        }
    }

    pub fn sbase_db(&self) -> &[f32; NH1 + 1] {
        &self.sbase_db
    }
}

/// Port of `wsjtx/lib/ft8/sync8.f90`.
///
/// Populates `scratch.sbase_db()` and returns prioritized `Ft8Candidate`s (with
/// `|freq_hz - nfqso_hz| <= 10 Hz` placed first, followed by descending `sync`).
#[allow(clippy::too_many_arguments)]
pub fn sync8(
    dd: &[f32; NMAX],
    f_low_hz: f32,
    f_high_hz: f32,
    sync_min: f32,
    nfqso_hz: f32,
    max_cand: usize,
    plans: &Ft8FftPlans,
    scratch: &mut Sync8Scratch,
) -> Vec<Ft8Candidate> {
    if max_cand == 0 {
        return Vec::new();
    }

    let stride = NH1 + 1;
    let tstep = (NSTEP as f32) / 12_000.0; // 0.040 s
    let df = 12_000.0_f32 / (NFFT1 as f32); // 3.125 Hz
    let fac = 1.0_f32 / 300.0;

    for j0 in 0..NHSYM {
        let ia = j0 * NSTEP;
        for (dst, &src) in scratch.r2c_in[..NSPS].iter_mut().zip(dd[ia..ia + NSPS].iter()) {
            *dst = fac * src;
        }
        scratch.r2c_in[NSPS..NFFT1].fill(0.0);

        plans
            .r2c_3840
            .process_with_scratch(
                &mut scratch.r2c_in,
                &mut scratch.r2c_out,
                &mut scratch.r2c_scratch,
            )
            .expect("r2c_3840 scratch sized");

        let row = &mut scratch.s[j0 * stride..(j0 + 1) * stride];
        for bin in 1..=NH1 {
            row[bin] = scratch.r2c_out[bin].norm_sqr();
        }
    }

    let (clamped_low, clamped_high) = compute_ft8_spectrum_baseline(
        dd,
        f_low_hz,
        f_high_hz,
        plans,
        &scratch.nuttall_scaled,
        &mut scratch.baseline_scratch,
        &mut scratch.sbase_db,
    );

    let ia = ((clamped_low / df).round() as usize).clamp(1, NH1);
    let ib = ((clamped_high / df).round() as usize).clamp(1, NH1 - 12);
    if ib < ia {
        return Vec::new();
    }

    let nssy = (NSPS / NSTEP) as isize; // 4 steps/sym
    let nfos = NFFT1 / NSPS; // 2 bins/tone
    let jstrt = (0.5_f32 / tstep) as isize; // truncates 12.5 -> 12 (matches sync8.f90:50)
    let nhsym_i = NHSYM as isize;

    for bin in ia..=ib {
        let sync_row = &mut scratch.sync2d[bin * LAG_COUNT..(bin + 1) * LAG_COUNT];
        for j in -JZ..=JZ {
            let mut ta = 0.0_f32;
            let mut tb = 0.0_f32;
            let mut tc = 0.0_f32;
            let mut t0a = 0.0_f32;
            let mut t0b = 0.0_f32;
            let mut t0c = 0.0_f32;

            for (n, &icos) in ICOS7.iter().enumerate() {
                let m = j + jstrt + nssy * (n as isize);
                let tone_offset = nfos * (icos as usize);

                if (1..=nhsym_i).contains(&m) {
                    let row = &scratch.s[((m - 1) as usize) * stride..];
                    ta += row[bin + tone_offset];
                    let mut sum7 = 0.0_f32;
                    for t in 0..=6 {
                        sum7 += row[bin + nfos * t];
                    }
                    t0a += sum7;
                }

                let m_b = m + nssy * 36;
                if (1..=nhsym_i).contains(&m_b) {
                    let row = &scratch.s[((m_b - 1) as usize) * stride..];
                    tb += row[bin + tone_offset];
                    let mut sum7 = 0.0_f32;
                    for t in 0..=6 {
                        sum7 += row[bin + nfos * t];
                    }
                    t0b += sum7;
                }

                let m_c = m + nssy * 72;
                if (1..=nhsym_i).contains(&m_c) {
                    let row = &scratch.s[((m_c - 1) as usize) * stride..];
                    tc += row[bin + tone_offset];
                    let mut sum7 = 0.0_f32;
                    for t in 0..=6 {
                        sum7 += row[bin + nfos * t];
                    }
                    t0c += sum7;
                }
            }

            let t_abc = ta + tb + tc;
            let t0_abc = (t0a + t0b + t0c - t_abc) / 6.0;
            let sync_abc = if t0_abc > 0.0 { t_abc / t0_abc } else { 0.0 };

            let t_bc = tb + tc;
            let t0_bc = (t0b + t0c - t_bc) / 6.0;
            let sync_bc = if t0_bc > 0.0 { t_bc / t0_bc } else { 0.0 };

            let lag_idx = (j + JZ) as usize;
            sync_row[lag_idx] = sync_abc.max(sync_bc);
        }
    }

    const MLAG: isize = 13;
    const MLAG2: isize = JZ;
    scratch.red.fill(0.0);
    scratch.red2.fill(0.0);

    for bin in ia..=ib {
        let sync_row = &scratch.sync2d[bin * LAG_COUNT..(bin + 1) * LAG_COUNT];

        let mut best_lag1 = -MLAG;
        let mut best_val1 = f32::NEG_INFINITY;
        for lag in -MLAG..=MLAG {
            let v = sync_row[(lag + JZ) as usize];
            if v > best_val1 {
                best_val1 = v;
                best_lag1 = lag;
            }
        }
        scratch.jpeak[bin] = best_lag1;
        scratch.red[bin] = best_val1;

        let mut best_lag2 = -MLAG2;
        let mut best_val2 = f32::NEG_INFINITY;
        for lag in -MLAG2..=MLAG2 {
            let v = sync_row[(lag + JZ) as usize];
            if v > best_val2 {
                best_val2 = v;
                best_lag2 = lag;
            }
        }
        scratch.jpeak2[bin] = best_lag2;
        scratch.red2[bin] = best_val2;
    }

    let iz = ib - ia + 1;
    let npctile = (0.40_f32 * (iz as f32)).round() as usize;
    if npctile < 1 {
        return Vec::new();
    }

    let mut indx1: Vec<usize> = (ia..=ib).collect();
    indx1.sort_by(|&a, &b| scratch.red[a].total_cmp(&scratch.red[b]));
    let base1 = scratch.red[indx1[npctile - 1]];
    if !base1.is_finite() || base1 <= 0.0 {
        return Vec::new();
    }
    for v in &mut scratch.red[ia..=ib] {
        *v /= base1;
    }

    let mut indx2: Vec<usize> = (ia..=ib).collect();
    indx2.sort_by(|&a, &b| scratch.red2[a].total_cmp(&scratch.red2[b]));
    let base2 = scratch.red2[indx2[npctile - 1]];
    if base2.is_finite() && base2 > 0.0 {
        for v in &mut scratch.red2[ia..=ib] {
            *v /= base2;
        }
    } else {
        scratch.red2[ia..=ib].fill(0.0);
    }

    let mut candidate0 = Vec::with_capacity(MAX_PRECAND.min(iz * 2));
    for i in 0..MAX_PRECAND.min(iz) {
        let n = indx1[iz - 1 - i];
        if candidate0.len() >= MAX_PRECAND {
            break;
        }
        if scratch.red[n] >= sync_min && scratch.red[n].is_finite() {
            candidate0.push(Ft8Candidate {
                freq_hz: (n as f32) * df,
                dt_sec: ((scratch.jpeak[n] as f32) - 0.5) * tstep,
                sync: scratch.red[n],
            });
        }
        if scratch.jpeak2[n] == scratch.jpeak[n] {
            continue;
        }
        if candidate0.len() >= MAX_PRECAND {
            break;
        }
        if scratch.red2[n] >= sync_min && scratch.red2[n].is_finite() {
            candidate0.push(Ft8Candidate {
                freq_hz: (n as f32) * df,
                dt_sec: ((scratch.jpeak2[n] as f32) - 0.5) * tstep,
                sync: scratch.red2[n],
            });
        }
    }

    let ncand = candidate0.len();
    for i in 1..ncand {
        for j in 0..i {
            let fdiff = candidate0[i].freq_hz.abs() - candidate0[j].freq_hz.abs();
            let tdiff = (candidate0[i].dt_sec - candidate0[j].dt_sec).abs();
            if fdiff.abs() < 4.0 && tdiff < 0.04 {
                if candidate0[i].sync >= candidate0[j].sync {
                    candidate0[j].sync = 0.0;
                } else {
                    candidate0[i].sync = 0.0;
                }
            }
        }
    }

    // Place candidates within 10 Hz of nfqso at the top of the list, then remaining sorted by sync desc.
    let mut result = Vec::with_capacity(ncand.min(max_cand));
    for cand in &mut candidate0 {
        if (cand.freq_hz - nfqso_hz).abs() <= 10.0 && cand.sync >= sync_min {
            result.push(*cand);
            cand.sync = 0.0;
            if result.len() >= max_cand {
                return result;
            }
        }
    }

    let mut order: Vec<usize> = (0..ncand).collect();
    order.sort_by(|&a, &b| candidate0[a].sync.total_cmp(&candidate0[b].sync));
    for &idx in order.iter().rev() {
        let cand = candidate0[idx];
        if cand.sync >= sync_min {
            result.push(Ft8Candidate {
                freq_hz: cand.freq_hz.abs(),
                dt_sec: cand.dt_sec,
                sync: cand.sync,
            });
            if result.len() >= max_cand {
                break;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ft8::params::NMAX;
    use crate::modulate::ft8::{encode_ft8_tones_from_77bits, gen_ft8_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    fn place_signal(wave: &[f32], start_sample: isize) -> Box<[f32; NMAX]> {
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for (i, &w) in wave.iter().enumerate() {
            let dst = start_sample + i as isize;
            if dst >= 0 && (dst as usize) < NMAX {
                dd[dst as usize] = w * 1000.0;
            }
        }
        dd
    }

    #[test]
    fn sync8_finds_nominal_dt_partial_and_late_tx_and_handles_silence() {
        let plans = Ft8FftPlans::new();
        let mut scratch = Sync8Scratch::new(&plans);

        // Silent slot returns 0 candidates and zero NaNs.
        let zero_dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        let cands = sync8(&zero_dd, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut scratch);
        assert!(cands.is_empty());

        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ N0AAA EM10", None, &ctx, &ht).unwrap();
        let tones = encode_ft8_tones_from_77bits(&packed.bits);
        let wave = gen_ft8_wave_real(&tones, 1500.0, 12_000);

        // Nominal DT = 0.0 s (start at 6000)
        let dd_nom = place_signal(&wave, 6000);
        let cands_nom = sync8(&dd_nom, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut scratch);
        assert!(!cands_nom.is_empty());
        assert!((cands_nom[0].freq_hz - 1500.0).abs() <= 3.125);
        assert!(cands_nom[0].dt_sec.abs() <= 0.065, "dt={}", cands_nom[0].dt_sec);

        // +2.0 s DT (outside MLAG=13 => red2 picks true peak while noiseless red sidelobe may also pass syncmin)
        let dd_p2 = place_signal(&wave, 6000 + 24_000);
        let cands_p2 = sync8(&dd_p2, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut scratch);
        assert!(!cands_p2.is_empty());
        let best_p2 = cands_p2
            .iter()
            .max_by(|a, b| a.sync.total_cmp(&b.sync))
            .unwrap();
        assert!((best_p2.dt_sec - 2.0).abs() <= 0.065, "dt={}", best_p2.dt_sec);

        // -2.0 s DT
        let dd_m2 = place_signal(&wave, 6000 - 24_000);
        let cands_m2 = sync8(&dd_m2, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut scratch);
        assert!(!cands_m2.is_empty());
        let best_m2 = cands_m2
            .iter()
            .max_by(|a, b| a.sync.total_cmp(&b.sync))
            .unwrap();
        assert!((best_m2.dt_sec + 2.0).abs() <= 0.065, "dt={}", best_m2.dt_sec);

        // 11.8 s partial slot
        let mut dd_part = dd_nom.clone();
        dd_part[141_600..].fill(0.0);
        let cands_part = sync8(&dd_part, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut scratch);
        assert!(!cands_part.is_empty());
        assert!((cands_part[0].freq_hz - 1500.0).abs() <= 3.125);

        // 5.0 s late-start TX (0..60,000 zeroed) -> sync_bc locks onto B+C!
        let mut dd_late = dd_nom;
        dd_late[..60_000].fill(0.0);
        let cands_late = sync8(&dd_late, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut scratch);
        assert!(!cands_late.is_empty());
        assert!((cands_late[0].freq_hz - 1500.0).abs() <= 3.125);
        assert!(cands_late[0].dt_sec.abs() <= 0.065, "dt={}", cands_late[0].dt_sec);
    }
}
