// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Deterministic `ChaCha8Rng`-seeded AWGN, linear-shifted superposition, late-TX/partial-slot,
//! and exact `SNR_2500` synthetic slot generator calibrated identically to `ft8sim`/`ft4sim`.

use std::f64::consts::PI;

use ftxr_core::Mode;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sim::{active_non_ramp_bounds, active_start_sample, SimError, SimRunner};

/// WSJT-X reference bandwidth ratio (`2500 Hz / 6000 Hz = 5/12`), matching `ft8sim.f90:92`.
pub const BANDWIDTH_RATIO: f64 = 2500.0 / 6000.0;

/// Nominal PCM gain matching `ft8sim.f90:154` and `ft4sim.f90:105`.
pub const NOMINAL_GAIN: f64 = 100.0;

/// Derives a deterministic 256-bit seed from `(seed_version, suite, mode, case_id, trial_idx)`.
///
/// Format: `SHA256("ftxr-synth-<seed_version>|<suite>|<mode>|<case_id>|<trial_idx>")`.
pub fn derive_seed(
    seed_version: &str,
    suite: &str,
    mode: Mode,
    case_id: &str,
    trial_idx: usize,
) -> [u8; 32] {
    let mode_str = match mode {
        Mode::Ft8 => "ft8",
        Mode::Ft4 => "ft4",
    };
    let label = format!("ftxr-synth-{seed_version}|{suite}|{mode_str}|{case_id}|{trial_idx}");
    let mut hasher = Sha256::new();
    hasher.update(label.as_bytes());
    hasher.finalize().into()
}

/// Deterministic unit-variance zero-mean Gaussian generator implementing the exact Marsaglia
/// polar Box-Muller algorithm from `wsjtx/lib/gran.c:19-27` backed by `ChaCha8Rng`.
pub struct GaussianRng {
    rng: ChaCha8Rng,
    cached: Option<f64>,
}

impl GaussianRng {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            rng: ChaCha8Rng::from_seed(seed),
            cached: None,
        }
    }

    #[inline]
    fn next_unit_f64(&mut self) -> f64 {
        // Standard 53-bit uniform float in [0.0, 1.0), platform-independent across x86_64/aarch64.
        const INV_2_53: f64 = 1.0 / ((1u64 << 53) as f64);
        ((self.rng.next_u64() >> 11) as f64) * INV_2_53
    }

    /// Generates one `N(0, 1)` sample.
    #[inline]
    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(g) = self.cached.take() {
            return g;
        }
        loop {
            let v1 = 2.0 * self.next_unit_f64() - 1.0;
            let v2 = 2.0 * self.next_unit_f64() - 1.0;
            let rsq = v1 * v1 + v2 * v2;
            if rsq > 0.0 && rsq < 1.0 {
                let fac = (-2.0 * rsq.ln() / rsq).sqrt();
                self.cached = Some(v1 * fac);
                return v2 * fac;
            }
        }
    }
}

/// Specification of one signal injected into a synthetic slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSpec {
    pub message: String,
    pub af_hz: f64,
    pub dt_sec: f64,
    pub snr_db: f64,
    /// Optional UTC-aligned late-TX mute duration (ms) from active start, followed by 10 ms fade-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_start_ms: Option<u32>,
}

/// Complete specification of a synthetic baseband slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotSpec {
    pub mode: Mode,
    pub signals: Vec<SignalSpec>,
    /// Optional partial-slot RX cutoff (ms): zeroes all samples (signal + noise) at `t >= truncate_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncate_ms: Option<u32>,
}

/// Result of synthesizing one deterministic slot.
#[derive(Debug, Clone)]
pub struct SynthesizedSlot {
    pub samples: Vec<i16>,
    pub gain: f64,
    pub rms_scaled: f64,
    #[allow(dead_code)]
    pub noise_stddev: f64,
    /// Empirical `SNR_2500` in dB measured over the exact active non-ramp interval (populated for
    /// single-signal non-truncated, non-late-start slots).
    #[allow(dead_code)]
    pub empirical_snr_2500_db: Option<f64>,
}

/// Converts target `SNR_2500` (dB) to linear signal amplitude multiplier for unit-power (`P=0.5`) waveforms.
#[inline]
pub fn snr_db_to_sig_scale(snr_db: f64) -> f64 {
    (2.0 * BANDWIDTH_RATIO).sqrt() * 10_f64.powf(0.05 * snr_db)
}

/// Applies optional UTC-aligned late-TX muting + 10 ms raised-cosine fade-in, followed by
/// linear zero-padded time shifting (`delta_samples = round(dt_sec * 12000)`), with zero wraparound.
pub fn transform_template(
    template: &[f64],
    mode: Mode,
    dt_sec: f64,
    late_start_ms: Option<u32>,
) -> Vec<f64> {
    let nmax = template.len();
    let mut work = template.to_vec();

    if let Some(ms) = late_start_ms {
        if ms > 0 {
            let mute_end = (active_start_sample(mode) + (ms as usize * 12)).min(nmax);
            for s in &mut work[..mute_end] {
                *s = 0.0;
            }
            // 10 ms click-free raised-cosine fade-in (120 samples at 12 kHz).
            let fade_len = 120usize.min(nmax.saturating_sub(mute_end));
            for i in 0..fade_len {
                let w = 0.5 * (1.0 - (PI * i as f64 / fade_len as f64).cos());
                work[mute_end + i] *= w;
            }
        }
    }

    let delta = (dt_sec * 12_000.0).round() as isize;
    if delta == 0 {
        return work;
    }

    let mut shifted = vec![0.0f64; nmax];
    for (src_idx, &val) in work.iter().enumerate() {
        let dst = src_idx as isize + delta;
        if dst >= 0 && (dst as usize) < nmax {
            shifted[dst as usize] = val;
        }
    }
    shifted
}

/// Synthesizes a deterministic slot directly from pre-extracted unit-power (`P=0.5`) templates.
pub fn synthesize_slot_from_templates(
    seed: [u8; 32],
    slot: &SlotSpec,
    templates: &[Vec<f64>],
) -> SynthesizedSlot {
    assert_eq!(slot.signals.len(), templates.len());
    let nmax = slot.mode.rx_window_samples();
    let mut sig_buf = vec![0.0f64; nmax];

    for (spec, tpl) in slot.signals.iter().zip(templates.iter()) {
        assert_eq!(tpl.len(), nmax);
        let transformed = transform_template(tpl, slot.mode, spec.dt_sec, spec.late_start_ms);
        let scale = snr_db_to_sig_scale(spec.snr_db);
        for (dst, &s) in sig_buf.iter_mut().zip(transformed.iter()) {
            *dst += scale * s;
        }
    }

    let mut rng = GaussianRng::from_seed(seed);
    let mut noise = vec![0.0f64; nmax];
    let mut sum_n = 0.0f64;
    let mut sum_n2 = 0.0f64;
    for x in &mut noise {
        let g = rng.next_gaussian();
        *x = g;
        sum_n += g;
        sum_n2 += g * g;
    }
    let mean_n = sum_n / nmax as f64;
    let noise_stddev = ((sum_n2 / nmax as f64) - mean_n * mean_n).max(0.0).sqrt();

    let empirical_snr_2500_db = if slot.signals.len() == 1
        && slot.truncate_ms.is_none()
        && slot.signals[0].late_start_ms.is_none()
        && slot.signals[0].dt_sec.abs() < 1e-6
    {
        let (a0, a1) = active_non_ramp_bounds(slot.mode);
        let psig: f64 = sig_buf[a0..a1].iter().map(|&x| x * x).sum();
        let pnoise_raw: f64 = noise[a0..a1].iter().map(|&x| x * x).sum();
        let pnoise_2500 = BANDWIDTH_RATIO * pnoise_raw;
        if psig > 0.0 && pnoise_2500 > 0.0 {
            Some(10.0 * (psig / pnoise_2500).log10())
        } else {
            None
        }
    } else {
        None
    };

    let mut mixed = vec![0.0f64; nmax];
    for i in 0..nmax {
        mixed[i] = sig_buf[i] + noise[i];
    }

    if let Some(trunc_ms) = slot.truncate_ms {
        let cut = (trunc_ms as usize * 12).min(nmax);
        for s in &mut mixed[cut..] {
            *s = 0.0;
        }
    }

    let peak = mixed.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    let gain = if peak > 0.0 {
        NOMINAL_GAIN.min(30_000.0 / peak)
    } else {
        NOMINAL_GAIN
    };

    let mut samples = Vec::with_capacity(nmax);
    let mut sum_sq_scaled = 0.0f64;
    for &v in &mixed {
        let scaled = (gain * v).round().clamp(-32_767.0, 32_767.0);
        let q = scaled as i16;
        samples.push(q);
        let qf = f64::from(q);
        sum_sq_scaled += qf * qf;
    }
    let rms_scaled = (sum_sq_scaled / nmax as f64).sqrt();

    SynthesizedSlot {
        samples,
        gain,
        rms_scaled,
        noise_stddev,
        empirical_snr_2500_db,
    }
}

/// High-level helper that extracts cached clean templates via `SimRunner` and synthesizes a slot.
pub fn synthesize_slot(
    sim: &SimRunner,
    seed: [u8; 32],
    slot: &SlotSpec,
) -> Result<SynthesizedSlot, SimError> {
    let mut templates = Vec::with_capacity(slot.signals.len());
    for sig in &slot.signals {
        let tpl = sim.clean_template(slot.mode, &sig.message, sig.af_hz)?;
        templates.push(tpl);
    }
    Ok(synthesize_slot_from_templates(seed, slot, &templates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ftxr_core::ft8;
    use std::f64::consts::TAU;

    fn synthetic_unit_sine_template(mode: Mode, af_hz: f64) -> Vec<f64> {
        let nmax = mode.rx_window_samples();
        let mut v = vec![0.0f64; nmax];
        let (a0, a1) = active_non_ramp_bounds(mode);
        for (idx, dst) in v.iter_mut().enumerate().take(a1).skip(a0) {
            *dst = (TAU * af_hz * (idx as f64) / 12_000.0).sin();
        }
        v
    }

    #[test]
    fn bit_exact_determinism_for_same_seed() {
        let seed = derive_seed("v1", "ft8_awgn", Mode::Ft8, "snr_-10", 0);
        let tpl = synthetic_unit_sine_template(Mode::Ft8, 1500.0);
        let slot = SlotSpec {
            mode: Mode::Ft8,
            signals: vec![SignalSpec {
                message: "CQ N0AAA EM10".into(),
                af_hz: 1500.0,
                dt_sec: 0.0,
                snr_db: -10.0,
                late_start_ms: None,
            }],
            truncate_ms: None,
        };
        let a = synthesize_slot_from_templates(seed, &slot, std::slice::from_ref(&tpl));
        let b = synthesize_slot_from_templates(seed, &slot, &[tpl]);
        assert_eq!(a.samples, b.samples);
    }

    #[test]
    fn empirical_snr_2500_calibration_within_0_05_db() {
        let tpl = synthetic_unit_sine_template(Mode::Ft8, 1500.0);
        for target_snr in [-10.0, -20.0] {
            let mut snr_errs = Vec::new();
            let mut stddevs = Vec::new();
            for trial in 0..20 {
                let seed = derive_seed(
                    "v1",
                    "cal_test",
                    Mode::Ft8,
                    &format!("snr_{target_snr}"),
                    trial,
                );
                let slot = SlotSpec {
                    mode: Mode::Ft8,
                    signals: vec![SignalSpec {
                        message: "CQ N0AAA EM10".into(),
                        af_hz: 1500.0,
                        dt_sec: 0.0,
                        snr_db: target_snr,
                        late_start_ms: None,
                    }],
                    truncate_ms: None,
                };
                let res = synthesize_slot_from_templates(seed, &slot, std::slice::from_ref(&tpl));
                let emp = res.empirical_snr_2500_db.expect("empirical SNR");
                snr_errs.push(emp - target_snr);
                stddevs.push(res.noise_stddev);
            }
            let mean_err = snr_errs.iter().sum::<f64>() / snr_errs.len() as f64;
            let mean_std = stddevs.iter().sum::<f64>() / stddevs.len() as f64;
            assert!(
                mean_err.abs() <= 0.05,
                "target {target_snr} dB: mean SNR error {mean_err:.4} dB exceeds ±0.05 dB"
            );
            assert!(
                (mean_std - 1.0).abs() <= 0.01,
                "mean noise stddev {mean_std:.5} outside 1.00 ± 0.01"
            );
        }
    }

    #[test]
    fn linear_shift_has_zero_wraparound_at_start() {
        let tpl = synthetic_unit_sine_template(Mode::Ft8, 1500.0);
        let shifted = transform_template(&tpl, Mode::Ft8, 3.0, None);
        assert_eq!(shifted.len(), ft8::params::NMAX);
        // Original active start is 6240; shifting +3.0 s (+36000) moves start to 42240.
        let lead_energy: f64 = shifted[0..42_000].iter().map(|&x| x * x).sum();
        assert_eq!(
            lead_energy, 0.0,
            "shifted signal wrapped around into 0..42000"
        );
    }

    #[test]
    fn late_tx_mutes_leading_active_window_with_fade_in() {
        let tpl = synthetic_unit_sine_template(Mode::Ft8, 1500.0);
        let muted = transform_template(&tpl, Mode::Ft8, 0.0, Some(5_000));
        // 6000 + 5000*12 = 66000 samples must be strictly zero.
        let muted_energy: f64 = muted[0..66_000].iter().map(|&x| x * x).sum();
        assert_eq!(muted_energy, 0.0);
        assert_eq!(muted[66_000], 0.0);
        let tail_energy: f64 = muted[66_200..157_000].iter().map(|&x| x * x).sum();
        assert!(tail_energy > 100.0);
    }
}
