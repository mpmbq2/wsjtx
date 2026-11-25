// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Waveform numerical diff (`ft8sim`/`ft4sim`) and TX-to-`jt9` loopback verification (`tx-loopback`).

use std::fs;
use std::path::Path;
use std::time::Instant;

use ftxr_core::modulate::ft4::{encode_ft4_tones_from_77bits, gen_ft4_wave_complex};
use ftxr_core::modulate::ft8::{encode_ft8_tones_from_77bits, gen_ft8_wave_complex};
use ftxr_core::modulate::modulate_slot_pcm;
use ftxr_core::protocol::hashtable::CallsignHashTable;
use ftxr_core::protocol::pack77::pack77;
use ftxr_core::protocol::types::UnpackContext;
use ftxr_core::{Mode, ModulateConfig};
use serde::Serialize;
use thiserror::Error;

use crate::jt9::{Jt9Error, Jt9Profile, Jt9RunConfig, Jt9Runner};
use crate::sim::{SimError, SimRunner};
use crate::synth::{derive_seed, snr_db_to_sig_scale, GaussianRng};
use crate::wav::{self, WavError};

const SIM_CLEAN_PEAK: f64 = 32_766.9;
const TX_AMPLITUDE_I16: f64 = 16_000.0;

#[derive(Debug, Error)]
pub enum TxLoopbackError {
    #[error("simulator error: {0}")]
    Sim(#[from] SimError),
    #[error("jt9 error: {0}")]
    Jt9(#[from] Jt9Error),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("modem error: {0}")]
    Core(#[from] ftxr_core::FtxrError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("waveform parity failure ({mode:?}): {detail}")]
    WaveformParity { mode: Mode, detail: String },
    #[error("loopback assertion failed [{case}]: {detail}")]
    Assertion { case: String, detail: String },
}

#[derive(Debug, Serialize)]
pub struct WaveformParityMetrics {
    pub ft8_max_abs_diff: f64,
    pub ft8_rms_diff: f64,
    pub ft8_corr: f64,
    pub ft4_max_abs_diff: f64,
    pub ft4_rms_diff: f64,
    pub ft4_corr: f64,
}

#[derive(Debug, Serialize)]
pub struct TxLoopbackReport {
    pub elapsed_ms: f64,
    pub waveform_parity: WaveformParityMetrics,
    pub total_loopback_cases: usize,
    pub passed_loopback_cases: usize,
    pub ft8_cases: usize,
    pub ft4_cases: usize,
}

fn compare_waveforms(a: &[f64], b: &[f64]) -> (f64, f64, f64) {
    let mut max_diff = 0.0_f64;
    let mut sum_sq_diff = 0.0_f64;
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let d = (x - y).abs();
        if d > max_diff {
            max_diff = d;
        }
        sum_sq_diff += d * d;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let rms = (sum_sq_diff / (a.len() as f64)).sqrt();
    let corr = dot / (norm_a.sqrt() * norm_b.sqrt());
    (max_diff, rms, corr)
}

fn verify_waveform_numerical_parity(
    sim: &SimRunner,
) -> Result<WaveformParityMetrics, TxLoopbackError> {
    let ht = CallsignHashTable::new();
    let ctx = UnpackContext::default();
    let msg = "CQ K1ABC FN42";
    let packed = pack77(msg, None, &ctx, &ht)?;

    // FT8: ft8sim snr=99 places sin(phi) starting at index 6000 (0.5 s).
    let (ft8_sim_i16, _) = sim.run_ft8sim(msg, 1500.0, 0.0, 0.0, 0.0, 99.0)?;
    let tones8 = encode_ft8_tones_from_77bits(&packed.bits);
    let cw8 = gen_ft8_wave_complex(&tones8, 1500.0, 12_000);
    let sim8_active: Vec<f64> = ft8_sim_i16[6000..6000 + cw8.len()]
        .iter()
        .map(|&s| f64::from(s) / SIM_CLEAN_PEAK)
        .collect();
    let rust8_im: Vec<f64> = cw8.iter().map(|c| c.im as f64).collect();
    // Early-window check (first 1000 active samples before Fortran f32 phase accumulation drifts)
    let (ft8_early_max, _, _) = compare_waveforms(&sim8_active[..1000], &rust8_im[..1000]);
    let (ft8_max, ft8_rms, ft8_corr) = compare_waveforms(&sim8_active, &rust8_im);
    // Fortran gen_ft8wave uses 32-bit float phi accumulation over 19,000 carrier cycles,
    // causing ~2.3e-3 rad drift by 12.64 s relative to exact f64 phase.
    if ft8_early_max > 2e-4 || ft8_max > 3e-3 || ft8_rms > 1.5e-3 || ft8_corr <= 0.999_995 {
        return Err(TxLoopbackError::WaveformParity {
            mode: Mode::Ft8,
            detail: format!(
                "early_max={ft8_early_max:.3e}, max_diff={ft8_max:.3e}, rms={ft8_rms:.3e}, corr={ft8_corr:.9}"
            ),
        });
    }

    // FT4: ft4sim snr=99 places cos(phi) starting at index 6000 - 576 = 5424.
    let (ft4_sim_i16, _) = sim.run_ft4sim(msg, 1500.0, 0.0, 0.0, 0.0, 99.0)?;
    let tones4 = encode_ft4_tones_from_77bits(&packed.bits);
    let cw4 = gen_ft4_wave_complex(&tones4, 1500.0, 12_000);
    let sim4_active: Vec<f64> = ft4_sim_i16[5424..5424 + cw4.len()]
        .iter()
        .map(|&s| f64::from(s) / SIM_CLEAN_PEAK)
        .collect();
    let rust4_re: Vec<f64> = cw4.iter().map(|c| c.re as f64).collect();
    let (ft4_early_max, _, _) = compare_waveforms(&sim4_active[..1000], &rust4_re[..1000]);
    let (ft4_max, ft4_rms, ft4_corr) = compare_waveforms(&sim4_active, &rust4_re);
    if ft4_early_max > 2e-4 || ft4_max > 3e-3 || ft4_rms > 1.5e-3 || ft4_corr <= 0.999_995 {
        return Err(TxLoopbackError::WaveformParity {
            mode: Mode::Ft4,
            detail: format!(
                "early_max={ft4_early_max:.3e}, max_diff={ft4_max:.3e}, rms={ft4_rms:.3e}, corr={ft4_corr:.9}"
            ),
        });
    }

    println!(
        "[PASS] Waveform numerical parity: FT8 early={ft8_early_max:.2e} max={ft8_max:.2e} rms={ft8_rms:.2e} corr={ft8_corr:.9}; FT4 early={ft4_early_max:.2e} max={ft4_max:.2e} rms={ft4_rms:.2e} corr={ft4_corr:.9}"
    );
    Ok(WaveformParityMetrics {
        ft8_max_abs_diff: ft8_max,
        ft8_rms_diff: ft8_rms,
        ft8_corr,
        ft4_max_abs_diff: ft4_max,
        ft4_rms_diff: ft4_rms,
        ft4_corr,
    })
}

fn slot_f32_to_i16_with_optional_awgn(
    slot_f32: &[f32],
    mode: Mode,
    snr_db: Option<f64>,
    seed_tag: &str,
) -> Vec<i16> {
    match snr_db {
        None => slot_f32
            .iter()
            .map(|&s| (f64::from(s) * TX_AMPLITUDE_I16).round().clamp(-32767.0, 32767.0) as i16)
            .collect(),
        Some(snr) => {
            // Note: WSJT-X snr_2500 convention scales a unit-amplitude (0.5 power) signal by
            // `sig = sqrt(2*2500/6000) * 10^(snr/20)` relative to unit-variance N(0,1) noise.
            let sig_scale = snr_db_to_sig_scale(snr);
            let noise_gain = 100.0;
            let mut rng = GaussianRng::from_seed(derive_seed("v1", "tx-loopback", mode, seed_tag, 0));
            slot_f32
                .iter()
                .map(|&s| {
                    let sample = (f64::from(s) * sig_scale + rng.next_gaussian()) * noise_gain;
                    sample.round().clamp(-32767.0, 32767.0) as i16
                })
                .collect()
        }
    }
}

fn decimate_48k_to_12k(samples_48k: &[f32]) -> Vec<f32> {
    samples_48k.iter().step_by(4).copied().collect()
}

fn expected_dt_sec(mode: Mode, start_delay_ms: u32) -> f32 {
    let ramp_lead = match mode {
        Mode::Ft8 => 0.0,
        Mode::Ft4 => 0.048,
    };
    (start_delay_ms as f32) / 1000.0 + ramp_lead - 0.500
}

#[allow(clippy::too_many_arguments)]
fn run_single_jt9_assertion(
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    tmp_dir: &Path,
    case_name: &str,
    mode: Mode,
    samples_i16: &[i16],
    expected_msg: &str,
    expected_af_hz: f32,
    expected_dt: f32,
) -> Result<(), TxLoopbackError> {
    let wav_path = tmp_dir.join("000000_000001.wav");
    wav::write_wav_12k_mono(&wav_path, samples_i16)?;

    let cfg = Jt9RunConfig::new(mode, 3, Jt9Profile::BlindNoAp, 1);
    let out = jt9.run_wav(&wav_path, &cfg, wisdom)?;

    let found = out.decodes.iter().find(|d| {
        d.message == expected_msg
            && (d.af_hz - expected_af_hz).abs() <= 2.0
            && (d.dt_sec - expected_dt).abs() <= 0.12
    });

    if found.is_none() {
        return Err(TxLoopbackError::Assertion {
            case: case_name.to_string(),
            detail: format!(
                "expected msg={expected_msg:?} af={expected_af_hz:.1} dt={expected_dt:.2}; got {:?}",
                out.decodes
            ),
        });
    }
    Ok(())
}

pub fn run_tx_loopback(
    wsjtx_root: &Path,
    sim: &SimRunner,
    jt9: &Jt9Runner,
    out_json: Option<&Path>,
) -> Result<TxLoopbackReport, TxLoopbackError> {
    let started = Instant::now();
    let waveform_parity = verify_waveform_numerical_parity(sim)?;

    let ft8_warmup = wsjtx_root.join("samples/FT8/210703_133430.wav");
    let ft4_warmup = wsjtx_root.join("samples/FT4/000000_000002.wav");
    let wisdom = jt9.ensure_warm_wisdom(1, &ft8_warmup, &ft4_warmup)?;

    let tmp = tempfile::tempdir()?;
    let mut ft8_cases = 0usize;
    let mut ft4_cases = 0usize;

    // 1. Nominal multi-type + frequency sweep + AWGN (+10 dB / -10 dB) + 48 kHz decimation
    let nominal_types: &[(&str, &str)] = &[
        ("TNX BOB 73 GL", "TNX BOB 73 GL"),
        ("K1ABC RR73; W9XYZ <KH1/KH7Z> -08", "K1ABC RR73; W9XYZ <...> -08"),
        ("K1ABC W9XYZ 6A WI", "K1ABC W9XYZ 6A WI"),
        ("W9XYZ K1ABC R 17B EMA", "W9XYZ K1ABC R 17B EMA"),
        ("123456789ABCDEF012", "123456789ABCDEF012"),
        ("CQ K1ABC FN42", "CQ K1ABC FN42"),
        ("G4ABC/P PA9XYZ JO22", "G4ABC/P PA9XYZ JO22"),
        ("W9XYZ K1ABC R 589 MA", "W9XYZ K1ABC R 589 MA"),
        ("CQ PJ4/K1ABC", "CQ PJ4/K1ABC"),
        ("<PA9XYZ> <G4ABC/P> 570123 IO91NP", "<...> <...> 570123 IO91NP"),
    ];
    let af_sweep = [500.0_f32, 1000.0, 1500.0, 2200.0, 2700.0];

    for &mode in &[Mode::Ft8, Mode::Ft4] {
        let slot_samples = mode.rx_window_samples();
        let delays: &[u32] = match mode {
            Mode::Ft8 => &[500, 0],
            Mode::Ft4 => &[300, 452, 0],
        };

        for (idx, &(tx_msg, rx_msg)) in nominal_types.iter().enumerate() {
            let af = af_sweep[idx % af_sweep.len()];
            let delay = delays[idx % delays.len()];
            let snr = match idx % 3 {
                0 => None,
                1 => Some(10.0),
                _ => Some(-10.0),
            };

            let ht = CallsignHashTable::new();
            let ctx = UnpackContext::default();
            let mut cfg = ModulateConfig::default_for(mode, af, 12_000);
            cfg.start_delay_ms = delay;
            let (slot, _) = modulate_slot_pcm(tx_msg, &cfg, slot_samples, &ctx, &ht)?;
            let i16_buf = slot_f32_to_i16_with_optional_awgn(
                &slot,
                mode,
                snr,
                &format!("nom-{mode:?}-{idx}"),
            );
            run_single_jt9_assertion(
                jt9,
                wisdom.as_deref(),
                tmp.path(),
                &format!("nominal-{mode:?}-{idx}"),
                mode,
                &i16_buf,
                rx_msg,
                af,
                expected_dt_sec(mode, delay),
            )?;
            match mode {
                Mode::Ft8 => ft8_cases += 1,
                Mode::Ft4 => ft4_cases += 1,
            }
        }

        // 48 kHz synthesis decimated 4x to 12 kHz
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let cfg48 = ModulateConfig::default_for(mode, 1500.0, 48_000);
        let (slot48, _) = modulate_slot_pcm("CQ N1MPM FN33", &cfg48, slot_samples * 4, &ctx, &ht)?;
        let slot12 = decimate_48k_to_12k(&slot48);
        let i16_buf = slot_f32_to_i16_with_optional_awgn(&slot12, mode, Some(10.0), &format!("48k-{mode:?}"));
        run_single_jt9_assertion(
            jt9,
            wisdom.as_deref(),
            tmp.path(),
            &format!("48k-decim-{mode:?}"),
            mode,
            &i16_buf,
            "CQ N1MPM FN33",
            1500.0,
            expected_dt_sec(mode, cfg48.start_delay_ms),
        )?;
        match mode {
            Mode::Ft8 => ft8_cases += 1,
            Mode::Ft4 => ft4_cases += 1,
        }
    }
    println!("[PASS] Nominal multi-type + AF sweep + 48kHz decimation + AWGN ({ft8_cases} FT8, {ft4_cases} FT4)");

    // 2. Late-start TX: FT8 (1..5 s) and FT4 (0.5..1.5 s)
    let ft8_offsets = [1000_u64, 2000, 3000, 4000, 5000];
    for &delay in &[500_u32, 0] {
        for &offset_ms in &ft8_offsets {
            let ht = CallsignHashTable::new();
            let ctx = UnpackContext::default();
            let mut cfg = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
            cfg.start_delay_ms = delay;
            cfg.offset_ms = offset_ms;
            let (slot, _) = modulate_slot_pcm("CQ N1MPM FN33", &cfg, Mode::Ft8.rx_window_samples(), &ctx, &ht)?;
            let i16_buf = slot_f32_to_i16_with_optional_awgn(&slot, Mode::Ft8, Some(10.0), &format!("late8-{delay}-{offset_ms}"));
            run_single_jt9_assertion(
                jt9,
                wisdom.as_deref(),
                tmp.path(),
                &format!("late-ft8-delay{delay}-off{offset_ms}"),
                Mode::Ft8,
                &i16_buf,
                "CQ N1MPM FN33",
                1500.0,
                expected_dt_sec(Mode::Ft8, delay),
            )?;
            ft8_cases += 1;
        }
    }

    let ft4_offsets = [500_u64, 1000, 1500];
    for &delay in &[300_u32, 0] {
        for &offset_ms in &ft4_offsets {
            let ht = CallsignHashTable::new();
            let ctx = UnpackContext::default();
            let mut cfg = ModulateConfig::default_for(Mode::Ft4, 1500.0, 12_000);
            cfg.start_delay_ms = delay;
            cfg.offset_ms = offset_ms;
            let (slot, _) = modulate_slot_pcm("CQ N1MPM FN33", &cfg, Mode::Ft4.rx_window_samples(), &ctx, &ht)?;
            let i16_buf = slot_f32_to_i16_with_optional_awgn(&slot, Mode::Ft4, Some(10.0), &format!("late4-{delay}-{offset_ms}"));
            run_single_jt9_assertion(
                jt9,
                wisdom.as_deref(),
                tmp.path(),
                &format!("late-ft4-delay{delay}-off{offset_ms}"),
                Mode::Ft4,
                &i16_buf,
                "CQ N1MPM FN33",
                1500.0,
                expected_dt_sec(Mode::Ft4, delay),
            )?;
            ft4_cases += 1;
        }
    }
    println!("[PASS] UTC-aligned late-start TX (FT8 1..5 s, FT4 0.5..1.5 s)");

    // 3. Mid-slot hot-swap: Message A from 0..T_swap, Message B from T_swap..end
    let run_hotswap = |mode: Mode, t_swap_ms: u64, af_a: f32, af_b: f32| -> Result<(), TxLoopbackError> {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let slot_samples = mode.rx_window_samples();
        let cfg_a = ModulateConfig::default_for(mode, af_a, 12_000);
        let mut cfg_b = ModulateConfig::default_for(mode, af_b, 12_000);
        cfg_b.offset_ms = t_swap_ms;

        let (mut slot_a, _) = modulate_slot_pcm("CQ N1MPM FN33", &cfg_a, slot_samples, &ctx, &ht)?;
        let (slot_b, wave_b) = modulate_slot_pcm("K1ABC N1MPM -08", &cfg_b, slot_samples, &ctx, &ht)?;
        if wave_b.samples.is_empty() || wave_b.samples[0].abs() >= 1e-6 {
            return Err(TxLoopbackError::Assertion {
                case: format!("hotswap-{mode:?}-{t_swap_ms}"),
                detail: "hot-swap tail did not start at 0.0 (click risk)".into(),
            });
        }
        let split_idx = ((t_swap_ms as f64) * 12_000.0 / 1000.0).round() as usize;
        slot_a[split_idx..].copy_from_slice(&slot_b[split_idx..]);

        let i16_buf = slot_f32_to_i16_with_optional_awgn(
            &slot_a,
            mode,
            Some(10.0),
            &format!("swap-{mode:?}-{t_swap_ms}-{af_b}"),
        );
        run_single_jt9_assertion(
            jt9,
            wisdom.as_deref(),
            tmp.path(),
            &format!("hotswap-{mode:?}-t{t_swap_ms}-af{af_b}"),
            mode,
            &i16_buf,
            "K1ABC N1MPM -08",
            af_b,
            expected_dt_sec(mode, cfg_b.start_delay_ms),
        )
    };

    for &t_swap in &[1500_u64, 3000] {
        run_hotswap(Mode::Ft8, t_swap, 1500.0, 1500.0)?;
        ft8_cases += 1;
    }
    for &t_swap in &[1500_u64, 3000, 5000] {
        run_hotswap(Mode::Ft8, t_swap, 1500.0, 1800.0)?;
        ft8_cases += 1;
    }
    for &t_swap in &[600_u64, 1000] {
        run_hotswap(Mode::Ft4, t_swap, 1500.0, 1500.0)?;
        ft4_cases += 1;
    }
    for &t_swap in &[600_u64, 1000, 1500] {
        run_hotswap(Mode::Ft4, t_swap, 1500.0, 1800.0)?;
        ft4_cases += 1;
    }
    println!("[PASS] Mid-slot hot-swap (FT8 & FT4, same AF & shifted AF)");

    // 4. Multi-file hash-table chain (12-bit + 22-bit hash resolution inside jt9)
    for &mode in &[Mode::Ft8, Mode::Ft4] {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let cfg = ModulateConfig::default_for(mode, 1500.0, 12_000);
        let slot_samples = mode.rx_window_samples();

        let chain_msgs = [
            "CQ PJ4/K1ABC",
            "<PJ4/K1ABC> W9XYZ -11",
            "<W9XYZ> PJ4/K1ABC RR73",
        ];
        let mut paths = Vec::new();
        for (i, &m) in chain_msgs.iter().enumerate() {
            let (slot, _) = modulate_slot_pcm(m, &cfg, slot_samples, &ctx, &ht)?;
            let i16_buf = slot_f32_to_i16_with_optional_awgn(&slot, mode, Some(10.0), &format!("chain-{mode:?}-{i}"));
            let p = tmp.path().join(format!("000000_{:06}.wav", i + 10));
            wav::write_wav_12k_mono(&p, &i16_buf)?;
            paths.push(p);
        }
        let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
        let jcfg = Jt9RunConfig::new(mode, 3, Jt9Profile::BlindNoAp, 1);
        let out = jt9.run_wavs(&path_refs, &jcfg, wisdom.as_deref())?;

        let expected_rx = [
            "CQ PJ4/K1ABC",
            "<PJ4/K1ABC> W9XYZ -11",
            "<W9XYZ> PJ4/K1ABC RR73",
        ];
        for exp in expected_rx {
            if !out.decodes.iter().any(|d| d.message == exp) {
                return Err(TxLoopbackError::Assertion {
                    case: format!("hash-chain-{mode:?}"),
                    detail: format!("missing resolved decode {exp:?}; got {:?}", out.decodes),
                });
            }
        }
        match mode {
            Mode::Ft8 => ft8_cases += 1,
            Mode::Ft4 => ft4_cases += 1,
        }
    }
    println!("[PASS] Multi-file hash-table chain (12-bit + 22-bit resolution in FT8 & FT4)");

    let total = ft8_cases + ft4_cases;
    let report = TxLoopbackReport {
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        waveform_parity,
        total_loopback_cases: total,
        passed_loopback_cases: total,
        ft8_cases,
        ft4_cases,
    };

    if let Some(out_path) = out_json {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(out_path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
        println!("Wrote tx-loopback report to {}", out_path.display());
    }

    Ok(report)
}
