// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Opt-in per-decode telemetry (`FecMethod`, Costas frequency drift, QSB symbol envelope).

use std::f32::consts::TAU;

use num_complex::Complex32;
use serde::{Deserialize, Serialize};

use crate::ft4::params as ft4p;
use crate::ft8::params as ft8p;

/// LDPC / OSD method that produced a valid 174-91 codeword decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FecMethod {
    /// Pure belief-propagation (sum-product) decode converging at iteration `iterations`.
    Bp { iterations: u8 },
    /// Ordered Statistics Decoding fallback at `order` on candidate BP output `bp_pass`.
    Osd { order: u8, bp_pass: u8 },
}

/// 8-bucket temporal envelope and QSB fading statistics across the active transmission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolEnvelope {
    /// Eight chronological bucket amplitudes in dB relative to the loudest active bucket (`<= 0.0`).
    /// Inactive buckets (e.g., muted leading/trailing segments on late-TX or partial passes) are `0.0`.
    pub bucket_db: [f32; 8],
    /// Bitmask of active buckets (`bit b` set iff bucket `b` lies within active non-truncated samples).
    pub active_buckets: u8,
    /// Mean linear symbol amplitude across active buckets.
    pub mean_amp: f32,
    /// Peak-to-mean amplitude ratio across active buckets in dB (`>= 0.0`).
    pub peak_to_mean_db: f32,
    /// Peak-to-trough fading depth across active buckets in dB (`max_db - min_db`, `0.0` if `< 2` active).
    pub qsb_depth_db: f32,
    /// Standard deviation across active buckets in dB (`0.0` if `< 2` active).
    pub qsb_std_db: f32,
}

/// Optional per-decode signal quality and fading telemetry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeTelemetry {
    pub fec_method: FecMethod,
    pub sync_score: f32,
    pub subtraction_pass: u8,
    pub hard_errors: u16,
    pub osd_distance: f32,
    pub quality: f32,
    /// Estimated total carrier frequency drift over the active transmission slot in Hz (`df/dt * T_slot`).
    pub freq_drift_hz: Option<f32>,
    /// 8-bucket QSB amplitude envelope across active channel symbols.
    pub qsb_envelope: Option<SymbolEnvelope>,
}

#[inline]
fn peakup(ym: f32, y0: f32, yp: f32) -> f32 {
    let denom = 2.0 * y0 - ym - yp;
    if denom.abs() < 1e-12 {
        0.0
    } else {
        (0.5 * (yp - ym) / denom).clamp(-0.5, 0.5)
    }
}

/// Estimates carrier frequency drift over a full FT8 transmission (12.64 s = 79 symbols) from the
/// three 7-symbol Costas blocks inside a demodulated downsampled baseband signal `cd0` (`fs = 200 Hz`,
/// `NSS = 32`, `df_tone = 6.25 Hz`, `ibest` = sample index of symbol 0 inside `cd0`).
///
/// Missing/muted Costas blocks (e.g. late TX or early partial-pass cutoff) are excluded automatically;
/// returns `None` when fewer than two valid Costas blocks have reliable lock.
pub fn estimate_ft8_costas_drift(
    cd0: &[Complex32],
    ibest: isize,
    active_samples_12k: usize,
) -> Option<f32> {
    const FS_DOWN: f32 = (ft8p::SAMPLE_RATE_HZ as f32) / (ft8p::NDOWN as f32); // 200.0 Hz
    const NSS: usize = ft8p::NSPS / ft8p::NDOWN; // 32
    const DF_TONE: f32 = (ft8p::SAMPLE_RATE_HZ as f32) / (ft8p::NSPS as f32); // 6.25 Hz
    const BLOCK_STARTS: [usize; 3] = [0, 36, 72];
    const SWEEP_HALF_HZ: f32 = 6.0;
    const SWEEP_STEP_HZ: f32 = 0.25;
    const SWEEP_N: usize = ((2.0 * SWEEP_HALF_HZ) / SWEEP_STEP_HZ) as usize + 1; // 49

    let active_down_end = (active_samples_12k / ft8p::NDOWN).min(cd0.len());
    let mut points: [(f32, f32, f32); 3] = [(0.0, 0.0, 0.0); 3];
    let mut n_valid = 0usize;

    let mut q = [Complex32::ZERO; 7 * NSS];
    let mut sweep = [0.0_f32; SWEEP_N];

    for &sym0 in &BLOCK_STARTS {
        let i_start = ibest + (sym0 as isize) * (NSS as isize);
        let i_end = i_start + (7 * NSS) as isize;
        if i_start < 0 || (i_end as usize) > active_down_end {
            continue;
        }
        let i_start_u = i_start as usize;

        // Check block energy is present (excludes leading muted frames on late TX).
        let mut block_energy = 0.0_f32;
        for (j, &tone) in ft8p::ICOS7.iter().enumerate() {
            let f_tone = f32::from(tone) * DF_TONE;
            for s in 0..NSS {
                let sample_idx = j * NSS + s;
                let x = cd0[i_start_u + sample_idx];
                block_energy += x.norm_sqr();
                let t = (s as f32) / FS_DOWN;
                let phase = -TAU * f_tone * t;
                let (sin_p, cos_p) = phase.sin_cos();
                q[sample_idx] = x * Complex32::new(cos_p, sin_p);
            }
        }
        if block_energy <= 1e-12 {
            continue;
        }

        // Sweep residual carrier frequency over [-6.0, +6.0] Hz coherently across the 7 Costas symbols.
        let mut max_idx = 0usize;
        let mut max_pow = -1.0_f32;
        for (step_idx, pow_slot) in sweep.iter_mut().enumerate() {
            let df = -SWEEP_HALF_HZ + (step_idx as f32) * SWEEP_STEP_HZ;
            let mut acc = Complex32::ZERO;
            let mut k = 0usize;
            for _j in 0..7 {
                // Use continuous time across the 7-symbol block; df_tone * T_sym = 1 cycle -> integer phase jump.
                for _s in 0..NSS {
                    let t = (k as f32) / FS_DOWN;
                    let (sin_p, cos_p) = (-TAU * df * t).sin_cos();
                    acc += q[k] * Complex32::new(cos_p, sin_p);
                    k += 1;
                }
            }
            let p = acc.norm_sqr();
            *pow_slot = p;
            if p > max_pow {
                max_pow = p;
                max_idx = step_idx;
            }
        }

        // Require clear peak prominence over median sweep power to exclude noise-only / blanked blocks.
        let mut sorted = sweep;
        sorted.sort_by(f32::total_cmp);
        let median = sorted[SWEEP_N / 2].max(1e-20);
        if max_pow < 1.8 * median {
            continue;
        }

        let frac = if max_idx > 0 && max_idx + 1 < SWEEP_N {
            peakup(sweep[max_idx - 1], sweep[max_idx], sweep[max_idx + 1])
        } else {
            0.0
        };
        let f_est = -SWEEP_HALF_HZ + ((max_idx as f32) + frac) * SWEEP_STEP_HZ;
        let t_center_sec = ((sym0 as f32) + 3.5) * (ft8p::NSPS as f32) / (ft8p::SAMPLE_RATE_HZ as f32);
        let weight = (max_pow - median).max(1e-12);
        points[n_valid] = (t_center_sec, f_est, weight);
        n_valid += 1;
    }

    weighted_slope_drift(&points[..n_valid], (ft8p::NN as f32) * 0.16)
}

/// Estimates carrier frequency drift over a full FT4 transmission (`103 * 0.048 s = 4.944 s`)
/// from the four 4-symbol Costas blocks inside demodulated aligned `cd` (`fs = 666.67 Hz`, `NSS = 32`,
/// `F_BAUD = 20.8333 Hz`, tones centered at `[-1.5, -0.5, +0.5, +1.5] * F_BAUD`).
pub fn estimate_ft4_costas_drift(
    cd: &[Complex32; ft4p::NFRAME_DOWN],
    ibest: isize,
    active_samples_12k: usize,
) -> Option<f32> {
    const FS_DOWN: f32 = ft4p::FS_DOWNSAMPLE;
    const NSS: usize = ft4p::NSS; // 32
    const F_BAUD: f32 = ft4p::F_BAUD;
    const ICOS_BLOCKS: [(usize, [u8; 4]); 4] = [
        (ft4p::SYNC_POSITIONS[0], ft4p::ICOS4_A),
        (ft4p::SYNC_POSITIONS[1], ft4p::ICOS4_B),
        (ft4p::SYNC_POSITIONS[2], ft4p::ICOS4_C),
        (ft4p::SYNC_POSITIONS[3], ft4p::ICOS4_D),
    ];
    const SWEEP_HALF_HZ: f32 = 8.0;
    const SWEEP_STEP_HZ: f32 = 0.25;
    const SWEEP_N: usize = ((2.0 * SWEEP_HALF_HZ) / SWEEP_STEP_HZ) as usize + 1; // 65

    let active_down_end = active_samples_12k / ft4p::NDOWN;
    let mut points: [(f32, f32, f32); 4] = [(0.0, 0.0, 0.0); 4];
    let mut n_valid = 0usize;

    let mut q = [Complex32::ZERO; 4 * NSS];
    let mut sweep = [0.0_f32; SWEEP_N];

    for &(sym0, pattern) in &ICOS_BLOCKS {
        let cb_start = ibest + (sym0 as isize) * (NSS as isize);
        let cb_end = cb_start + (4 * NSS) as isize;
        if cb_start < 0 || (cb_end as usize) > active_down_end {
            continue;
        }
        let cd_start = sym0 * NSS;

        let mut block_energy = 0.0_f32;
        let mut phi = 0.0_f32;
        for (j, &tone) in pattern.iter().enumerate() {
            let f_tone = (f32::from(tone) - 1.5) * F_BAUD;
            let dphi = TAU * f_tone / FS_DOWN;
            for s in 0..NSS {
                let sample_idx = j * NSS + s;
                let x = cd[cd_start + sample_idx];
                block_energy += x.norm_sqr();
                let (sin_p, cos_p) = (-phi).sin_cos();
                q[sample_idx] = x * Complex32::new(cos_p, sin_p);
                phi = (phi + dphi).rem_euclid(TAU);
            }
        }
        if block_energy <= 1e-12 {
            continue;
        }

        let mut max_idx = 0usize;
        let mut max_pow = -1.0_f32;
        for (step_idx, pow_slot) in sweep.iter_mut().enumerate() {
            let df = -SWEEP_HALF_HZ + (step_idx as f32) * SWEEP_STEP_HZ;
            let mut acc = Complex32::ZERO;
            let mut k = 0usize;
            for _j in 0..4 {
                for _s in 0..NSS {
                    let t = (k as f32) / FS_DOWN;
                    let (sin_p, cos_p) = (-TAU * df * t).sin_cos();
                    acc += q[k] * Complex32::new(cos_p, sin_p);
                    k += 1;
                }
            }
            let p = acc.norm_sqr();
            *pow_slot = p;
            if p > max_pow {
                max_pow = p;
                max_idx = step_idx;
            }
        }

        let mut sorted = sweep;
        sorted.sort_by(f32::total_cmp);
        let median = sorted[SWEEP_N / 2].max(1e-20);
        if max_pow < 1.5 * median {
            continue;
        }

        let frac = if max_idx > 0 && max_idx + 1 < SWEEP_N {
            peakup(sweep[max_idx - 1], sweep[max_idx], sweep[max_idx + 1])
        } else {
            0.0
        };
        let f_est = -SWEEP_HALF_HZ + ((max_idx as f32) + frac) * SWEEP_STEP_HZ;
        let t_center_sec = ((sym0 as f32) + 2.0) * (ft4p::NSPS as f32) / (ft4p::SAMPLE_RATE_HZ as f32);
        let weight = (max_pow - median).max(1e-12);
        points[n_valid] = (t_center_sec, f_est, weight);
        n_valid += 1;
    }

    let slot_tx_sec = (ft4p::NN as f32) * (ft4p::NSPS as f32) / (ft4p::SAMPLE_RATE_HZ as f32);
    weighted_slope_drift(&points[..n_valid], slot_tx_sec)
}

fn weighted_slope_drift(points: &[(f32, f32, f32)], slot_tx_sec: f32) -> Option<f32> {
    if points.len() < 2 {
        return None;
    }
    let mut sum_w = 0.0_f64;
    let mut sum_wt = 0.0_f64;
    let mut sum_wf = 0.0_f64;
    for &(t, f, w) in points {
        let w = f64::from(w);
        sum_w += w;
        sum_wt += w * f64::from(t);
        sum_wf += w * f64::from(f);
    }
    if sum_w <= 0.0 {
        return None;
    }
    let t_bar = sum_wt / sum_w;
    let f_bar = sum_wf / sum_w;
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for &(t, f, w) in points {
        let w = f64::from(w);
        let dt = f64::from(t) - t_bar;
        let df = f64::from(f) - f_bar;
        num += w * dt * df;
        den += w * dt * dt;
    }
    if den < 1e-6 {
        return None;
    }
    let slope_hz_per_sec = num / den;
    Some((slope_hz_per_sec * f64::from(slot_tx_sec)) as f32)
}

/// Computes the 8-bucket QSB `SymbolEnvelope` for a decoded FT8 signal given `cd0` (`fs = 200 Hz`),
/// `ibest`, decoded tones `[u8; 79]`, optional linear drift over the slot, and `active_samples_12k`.
pub fn compute_ft8_symbol_envelope(
    cd0: &[Complex32],
    ibest: isize,
    tones: &[u8; ft8p::NN],
    drift_hz: Option<f32>,
    active_samples_12k: usize,
) -> Option<SymbolEnvelope> {
    const FS_DOWN: f32 = (ft8p::SAMPLE_RATE_HZ as f32) / (ft8p::NDOWN as f32);
    const NSS: usize = ft8p::NSPS / ft8p::NDOWN;
    const DF_TONE: f32 = (ft8p::SAMPLE_RATE_HZ as f32) / (ft8p::NSPS as f32);

    let active_down_end = (active_samples_12k / ft8p::NDOWN).min(cd0.len());
    let slope = drift_hz.unwrap_or(0.0) / ((ft8p::NN as f32) * 0.16);
    let mut syms = [0.0_f32; ft8p::NN];
    let mut present = [false; ft8p::NN];

    for k in 0..ft8p::NN {
        let i0 = ibest + (k as isize) * (NSS as isize);
        let i1 = i0 + NSS as isize;
        if i0 < 0 || (i1 as usize) > active_down_end {
            continue;
        }
        let i0_u = i0 as usize;
        let f_drift = slope * (((k as f32) - (ft8p::NN as f32) * 0.5) * 0.16);
        let f_tone = f32::from(tones[k]) * DF_TONE + f_drift;
        let mut sym_energy = 0.0_f32;
        let mut acc = Complex32::ZERO;
        for s in 0..NSS {
            let x = cd0[i0_u + s];
            sym_energy += x.norm_sqr();
            let t = (s as f32) / FS_DOWN;
            let (sin_p, cos_p) = (-TAU * f_tone * t).sin_cos();
            acc += x * Complex32::new(cos_p, sin_p);
        }
        if sym_energy > 1e-12 {
            syms[k] = acc.norm() / (NSS as f32);
            present[k] = true;
        }
    }

    summarize_symbol_envelope(&syms, &present)
}

/// Computes the 8-bucket QSB `SymbolEnvelope` for a decoded FT4 signal given aligned `cd`,
/// `ibest`, decoded tones `[u8; 103]`, optional linear drift over the slot, and `active_samples_12k`.
pub fn compute_ft4_symbol_envelope(
    cd: &[Complex32; ft4p::NFRAME_DOWN],
    ibest: isize,
    tones: &[u8; ft4p::NN],
    drift_hz: Option<f32>,
    active_samples_12k: usize,
) -> Option<SymbolEnvelope> {
    const FS_DOWN: f32 = ft4p::FS_DOWNSAMPLE;
    const NSS: usize = ft4p::NSS;
    const F_BAUD: f32 = ft4p::F_BAUD;

    let active_down_end = active_samples_12k / ft4p::NDOWN;
    let slot_tx_sec = (ft4p::NN as f32) * (ft4p::NSPS as f32) / (ft4p::SAMPLE_RATE_HZ as f32);
    let slope = drift_hz.unwrap_or(0.0) / slot_tx_sec;
    let t_sym = (ft4p::NSPS as f32) / (ft4p::SAMPLE_RATE_HZ as f32);
    let mut syms = [0.0_f32; ft4p::NN];
    let mut present = [false; ft4p::NN];

    for k in 0..ft4p::NN {
        let cb0 = ibest + (k as isize) * (NSS as isize);
        let cb1 = cb0 + NSS as isize;
        if cb0 < 0 || (cb1 as usize) > active_down_end {
            continue;
        }
        let cd0_u = k * NSS;
        let f_drift = slope * (((k as f32) - (ft4p::NN as f32) * 0.5) * t_sym);
        let f_tone = (f32::from(tones[k]) - 1.5) * F_BAUD + f_drift;
        let mut sym_energy = 0.0_f32;
        let mut acc = Complex32::ZERO;
        for s in 0..NSS {
            let x = cd[cd0_u + s];
            sym_energy += x.norm_sqr();
            let t = (s as f32) / FS_DOWN;
            let (sin_p, cos_p) = (-TAU * f_tone * t).sin_cos();
            acc += x * Complex32::new(cos_p, sin_p);
        }
        if sym_energy > 1e-12 {
            syms[k] = acc.norm() / (NSS as f32);
            present[k] = true;
        }
    }

    summarize_symbol_envelope(&syms, &present)
}

fn summarize_symbol_envelope(syms: &[f32], present: &[bool]) -> Option<SymbolEnvelope> {
    let n = syms.len();
    let mut bucket_sum = [0.0_f32; 8];
    let mut bucket_cnt = [0usize; 8];
    for i in 0..n {
        if present[i] {
            let b = (i * 8) / n;
            bucket_sum[b] += syms[i];
            bucket_cnt[b] += 1;
        }
    }

    let mut bucket_amp = [0.0_f32; 8];
    let mut active_buckets = 0u8;
    let mut max_amp = 0.0_f32;
    let mut sum_active_amp = 0.0_f32;
    let mut n_active = 0usize;
    let total_present: usize = bucket_cnt.iter().sum();
    if total_present == 0 {
        return None;
    }

    let overall_mean: f32 = bucket_sum.iter().sum::<f32>() / (total_present as f32);
    if overall_mean <= 1e-12 {
        return None;
    }

    // First identify raw bucket means (requiring >= 50% of nominal symbols in a bucket and >5% of mean).
    let expected_per_bucket = (n / 8).max(1);
    for b in 0..8 {
        if bucket_cnt[b] * 2 >= expected_per_bucket {
            let avg = bucket_sum[b] / (bucket_cnt[b] as f32);
            if avg > overall_mean * 0.02 {
                bucket_amp[b] = avg;
                active_buckets |= 1u8 << b;
                max_amp = max_amp.max(avg);
                sum_active_amp += avg;
                n_active += 1;
            }
        }
    }
    if n_active == 0 || max_amp <= 1e-12 {
        return None;
    }

    let mean_amp = sum_active_amp / (n_active as f32);
    let peak_to_mean_db = (20.0 * (max_amp / mean_amp.max(1e-12)).log10()).max(0.0);

    let mut bucket_db = [0.0_f32; 8];
    let mut min_db = 0.0_f32;
    let mut sum_db = 0.0_f32;
    let mut sum_db2 = 0.0_f32;
    for b in 0..8 {
        if (active_buckets & (1u8 << b)) != 0 {
            let db = (20.0 * (bucket_amp[b] / max_amp).max(1e-3).log10()).clamp(-60.0, 0.0);
            bucket_db[b] = db;
            min_db = min_db.min(db);
            sum_db += db;
            sum_db2 += db * db;
        }
    }

    let (qsb_depth_db, qsb_std_db) = if n_active >= 2 {
        let nf = n_active as f32;
        let mean_db = sum_db / nf;
        let var = (sum_db2 / nf - mean_db * mean_db).max(0.0);
        ((-min_db).max(0.0), var.sqrt())
    } else {
        (0.0, 0.0)
    };

    Some(SymbolEnvelope {
        bucket_db,
        active_buckets,
        mean_amp,
        peak_to_mean_db,
        qsb_depth_db,
        qsb_std_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulate::ft4::encode_ft4_tones_from_77bits;
    use crate::modulate::ft8::encode_ft8_tones_from_77bits;
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    fn synth_ft8_cd0_with_drift_and_qsb(
        tones: &[u8; ft8p::NN],
        drift_hz_slot: f32,
        qsb_depth_db: f32,
        mute_first_syms: usize,
        truncate_last_syms: usize,
    ) -> (Vec<Complex32>, isize, usize) {
        const FS_DOWN: f32 = 200.0;
        const NSS: usize = 32;
        const DF_TONE: f32 = 6.25;
        let ibest: isize = 100;
        let total_down = 3000;
        let mut cd0 = vec![Complex32::ZERO; total_down];
        let slot_tx_sec = (ft8p::NN as f32) * 0.16;
        let slope = drift_hz_slot / slot_tx_sec;
        let mut phase = 0.0_f32;
        let min_lin = 10.0_f32.powf(-qsb_depth_db / 20.0);

        for (k, &tone) in tones.iter().enumerate().take(ft8p::NN) {
            let env_amp = if qsb_depth_db <= 0.01 {
                1.0
            } else {
                // Smooth sin^2 fade dip with plateau near slot ends down to `min_lin` at midpoint.
                let u = (k as f32) / ((ft8p::NN - 1) as f32);
                let s = (std::f32::consts::PI * u).sin();
                1.0 - (1.0 - min_lin) * (s * s)
            };
            let is_muted = k < mute_first_syms || k >= ft8p::NN - truncate_last_syms;
            let amp = if is_muted { 0.0 } else { env_amp };
            let f_drift = slope * (((k as f32) - (ft8p::NN as f32) * 0.5) * 0.16);
            let f_tone = f32::from(tone) * DF_TONE + f_drift;
            for s in 0..NSS {
                let idx = (ibest as usize) + k * NSS + s;
                let (sin_p, cos_p) = phase.sin_cos();
                cd0[idx] = Complex32::new(cos_p * amp, sin_p * amp);
                phase += TAU * f_tone / FS_DOWN;
            }
        }
        let active_12k = ((ibest as usize) + (ft8p::NN - truncate_last_syms) * NSS) * ft8p::NDOWN;
        (cd0, ibest, active_12k)
    }

    fn synth_ft4_cd_with_drift_and_qsb(
        tones: &[u8; ft4p::NN],
        drift_hz_slot: f32,
        qsb_depth_db: f32,
        mute_first_syms: usize,
        truncate_last_syms: usize,
    ) -> ([Complex32; ft4p::NFRAME_DOWN], isize, usize) {
        const FS_DOWN: f32 = ft4p::FS_DOWNSAMPLE;
        const NSS: usize = ft4p::NSS;
        const F_BAUD: f32 = ft4p::F_BAUD;
        let ibest: isize = 333;
        let mut cd = [Complex32::ZERO; ft4p::NFRAME_DOWN];
        let t_sym = (ft4p::NSPS as f32) / (ft4p::SAMPLE_RATE_HZ as f32);
        let slot_tx_sec = (ft4p::NN as f32) * t_sym;
        let slope = drift_hz_slot / slot_tx_sec;
        let mut phase = 0.0_f32;
        let min_lin = 10.0_f32.powf(-qsb_depth_db / 20.0);

        for (k, &tone) in tones.iter().enumerate().take(ft4p::NN) {
            let env_amp = if qsb_depth_db <= 0.01 {
                1.0
            } else {
                let u = (k as f32) / ((ft4p::NN - 1) as f32);
                let s = (std::f32::consts::PI * u).sin();
                1.0 - (1.0 - min_lin) * s
            };
            let is_muted = k < mute_first_syms || k >= ft4p::NN - truncate_last_syms;
            let amp = if is_muted { 0.0 } else { env_amp };
            let f_drift = slope * (((k as f32) - (ft4p::NN as f32) * 0.5) * t_sym);
            let f_tone = (f32::from(tone) - 1.5) * F_BAUD + f_drift;
            for s in 0..NSS {
                let idx = k * NSS + s;
                let (sin_p, cos_p) = phase.sin_cos();
                cd[idx] = Complex32::new(cos_p * amp, sin_p * amp);
                phase += TAU * f_tone / FS_DOWN;
            }
        }
        let active_12k = ((ibest as usize) + (ft4p::NN - truncate_last_syms) * NSS) * ft4p::NDOWN;
        (cd, ibest, active_12k)
    }

    #[test]
    fn ft8_costas_drift_and_qsb_accuracy_and_partial_slot_tolerance() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let tones = encode_ft8_tones_from_77bits(&packed.bits);

        // 1. Flat amplitude, +2.5 Hz/slot linear drift
        let (cd0, ibest, active) = synth_ft8_cd0_with_drift_and_qsb(&tones, 2.5, 0.0, 0, 0);
        let drift = estimate_ft8_costas_drift(&cd0, ibest, active).expect("drift");
        assert!((drift - 2.5).abs() <= 0.25, "drift={drift}");
        let env = compute_ft8_symbol_envelope(&cd0, ibest, &tones, Some(drift), active).unwrap();
        assert_eq!(env.active_buckets, 0xFF);
        assert!(env.qsb_depth_db <= 0.5, "flat qsb_depth={}", env.qsb_depth_db);

        // 2. 12 dB QSB fading + -1.8 Hz/slot drift
        let (cd0_qsb, ibest, active) = synth_ft8_cd0_with_drift_and_qsb(&tones, -1.8, 12.0, 0, 0);
        let drift_q = estimate_ft8_costas_drift(&cd0_qsb, ibest, active).expect("drift_q");
        assert!((drift_q - (-1.8)).abs() <= 0.25, "drift_q={drift_q}");
        let env_q =
            compute_ft8_symbol_envelope(&cd0_qsb, ibest, &tones, Some(drift_q), active).unwrap();
        assert!(
            (env_q.qsb_depth_db - 12.0).abs() <= 1.5,
            "qsb_depth={}",
            env_q.qsb_depth_db
        );
        assert!(env_q.qsb_std_db > 2.5);

        // 3. Late-TX (Costas block 0 muted, 2 blocks remain)
        let (cd0_late, ibest, active) = synth_ft8_cd0_with_drift_and_qsb(&tones, 2.0, 0.0, 20, 0);
        let drift_late = estimate_ft8_costas_drift(&cd0_late, ibest, active).expect("drift_late");
        assert!((drift_late - 2.0).abs() <= 0.25, "drift_late={drift_late}");
        let env_late =
            compute_ft8_symbol_envelope(&cd0_late, ibest, &tones, Some(drift_late), active).unwrap();
        assert!(env_late.qsb_depth_db <= 0.5, "late partial slot should not hallucinate QSB");

        // 4. Only 1 Costas block present -> None
        let (cd0_one, ibest, active) = synth_ft8_cd0_with_drift_and_qsb(&tones, 2.0, 0.0, 45, 0);
        assert!(estimate_ft8_costas_drift(&cd0_one, ibest, active).is_none());
    }

    #[test]
    fn ft4_costas_drift_and_qsb_accuracy() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let tones = encode_ft4_tones_from_77bits(&packed.bits);

        let (cd, ibest, active) = synth_ft4_cd_with_drift_and_qsb(&tones, -2.0, 10.0, 0, 0);
        let drift = estimate_ft4_costas_drift(&cd, ibest, active).expect("ft4 drift");
        assert!((drift - (-2.0)).abs() <= 0.25, "ft4 drift={drift}");
        let env = compute_ft4_symbol_envelope(&cd, ibest, &tones, Some(drift), active).unwrap();
        assert!((env.qsb_depth_db - 10.0).abs() <= 1.5, "ft4 qsb={}", env.qsb_depth_db);

        // Missing last block -> still recovers from blocks 0..3
        let (cd_part, ibest, active) = synth_ft4_cd_with_drift_and_qsb(&tones, 1.5, 0.0, 0, 10);
        let drift_part = estimate_ft4_costas_drift(&cd_part, ibest, active).expect("ft4 partial drift");
        assert!((drift_part - 1.5).abs() <= 0.25, "ft4 partial drift={drift_part}");
    }
}
