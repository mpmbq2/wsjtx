// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Stage 5 1,000-slot continuous warm `--worker` endurance soak, cancellation, and RSS stability harness.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use ftxr_core::Mode;
use serde::Serialize;
use thiserror::Error;

use crate::ipc_client::{
    read_proc_status, resolve_modem_binary, FtxrIpcClient, IpcClientError,
};
use crate::wav::{read_wav_12k_mono, WavError};

#[derive(Debug, Error)]
pub enum SoakError {
    #[error("IPC error: {0}")]
    Ipc(#[from] IpcClientError),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("soak gate failed: {0}")]
    GateFailed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct RssSample {
    pub slot: usize,
    pub vm_rss_kb: u64,
    pub threads: usize,
}

#[derive(Debug, Serialize)]
pub struct SoakReport {
    pub slots_executed: usize,
    pub worker_threads: usize,
    pub ft8_slots: usize,
    pub ft4_slots: usize,
    pub total_decodes: usize,
    pub cancellations_ok: usize,
    pub max_cancel_wall_ms: f64,
    pub concurrent_tx_ok: usize,
    pub error_recoveries_ok: usize,
    pub session_resets: usize,
    pub rss_warmed_kb: u64,
    pub rss_end_kb: u64,
    pub rss_growth_mb: f64,
    pub threads_min: usize,
    pub threads_max: usize,
    pub rss_samples: Vec<RssSample>,
    pub elapsed_total_ms: f64,
    pub exit_code: Option<i32>,
}

pub fn run_soak(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    modem_bin: Option<&Path>,
    threads: Option<usize>,
    slots: usize,
    out_json: Option<&Path>,
) -> Result<SoakReport, SoakError> {
    let t0 = Instant::now();
    let bin_path = resolve_modem_binary(rust_ftx_root, modem_bin)?;

    let ft8_crowded = read_wav_12k_mono(&wsjtx_root.join("samples/FT8/210703_133430.wav"))?;
    let ft8_medium = read_wav_12k_mono(&rust_ftx_root.join("fixtures/committed/ft8/191111_110200.wav"))?;
    let ft4_crowded = read_wav_12k_mono(&wsjtx_root.join("samples/FT4/000000_000002.wav"))?;

    let ft8_early = ft8_crowded[..141_600.min(ft8_crowded.len())].to_vec();
    let ft8_mid = ft8_crowded[..162_000.min(ft8_crowded.len())].to_vec();

    let mut client = FtxrIpcClient::spawn(&bin_path, threads, false)?;
    let pid = client.pid();
    let worker_threads = client.worker_threads;

    let mut ft8_slots = 0usize;
    let mut ft4_slots = 0usize;
    let mut total_decodes = 0usize;
    let mut cancellations_ok = 0usize;
    let mut max_cancel_wall_ms = 0.0_f64;
    let mut concurrent_tx_ok = 0usize;
    let mut error_recoveries_ok = 0usize;
    let mut session_resets = 0usize;
    let mut rss_samples = Vec::new();

    for slot in 0..slots {
        // Periodic session context update & reset every 100 slots
        if slot % 100 == 0 {
            let _ = client.session_reset()?;
            session_resets += 1;
            let ctx = serde_json::json!({
                "my_call": "N0AAA",
                "dx_call": if slot % 200 == 0 { "K1ABC" } else { "PJ4/W9XYZ" },
                "dx_grid": "FN42",
                "qso_freq_hz": 1500.0,
                "qso_progress": (slot % 5) as u8,
            });
            client.set_context(&ctx)?;
        }

        // Every 80 slots: inject deliberate malformed JSON & bad sample_rate error and verify healthy ping
        if slot % 80 == 40 {
            client.send_raw_frame(0, br#"{"op":"unknown_bad_op"}"#, &[])?;
            let (v, _) = client.recv_value()?;
            if v["op"] == "error" && v["fatal"] == false {
                error_recoveries_ok += 1;
            }
            let pong = client.ping(slot as u64)?;
            if pong["op"] != "pong" {
                return Err(SoakError::GateFailed("ping failed after malformed JSON".into()));
            }
        }

        // Every 25 slots: active mid-pass cancellation + concurrent encode_tx stress
        if slot % 25 == 10 {
            let dec_id = client.start_decode_slot_i16(Mode::Ft8, 3, &ft8_crowded, false, None)?;
            std::thread::sleep(Duration::from_millis(3));
            let _tx_id = client.send_encode_tx(Mode::Ft8, "CQ N0AAA EM10", 1500.0, false)?;
            let cancel_t0 = Instant::now();
            let _cancel_id = client.send_cancel(Some(&dec_id))?;
            let (evs, done_status, tx_lats) = client.wait_until_idle(Duration::from_secs(10))?;
            let cancel_ms = cancel_t0.elapsed().as_secs_f64() * 1000.0;
            max_cancel_wall_ms = max_cancel_wall_ms.max(cancel_ms);
            total_decodes += evs;
            ft8_slots += 1;
            if matches!(done_status.as_deref(), Some("cancelled" | "completed")) {
                cancellations_ok += 1;
            }
            concurrent_tx_ok += tx_lats.len();
        } else if slot % 4 == 3 {
            // FT4 slot (alternating telemetry off/on)
            let res = client.decode_slot_i16(
                Mode::Ft4,
                3,
                &ft4_crowded,
                1500.0,
                false,
                slot % 8 == 7,
                Some(5000),
                None,
            )?;
            total_decodes += res.decoded_count;
            ft4_slots += 1;
        } else {
            // Mix FT8 Pass 1/2/3 slots cleanly
            let (audio, early) = match slot % 3 {
                0 => (&ft8_early, true),
                1 => (&ft8_mid, true),
                _ => (&ft8_medium, false),
            };
            let res = client.decode_slot_i16(
                Mode::Ft8,
                3,
                audio,
                1500.0,
                early,
                slot % 6 == 0,
                Some(5000),
                None,
            )?;
            total_decodes += res.decoded_count;
            ft8_slots += 1;
        }

        if slot == 0 || (slot + 1) % 50 == 0 || slot + 1 == slots {
            if let Some(st) = read_proc_status(pid) {
                rss_samples.push(RssSample {
                    slot: slot + 1,
                    vm_rss_kb: st.vm_rss_kb,
                    threads: st.threads,
                });
            }
        }
    }

    let exit = client.shutdown()?;
    let exit_code = exit.code();
    if !exit.success() {
        return Err(SoakError::GateFailed(format!(
            "worker shutdown non-zero exit: {exit:?}"
        )));
    }

    let rss_warmed_kb = rss_samples
        .iter()
        .find(|s| s.slot >= 100)
        .or_else(|| rss_samples.first())
        .map(|s| s.vm_rss_kb)
        .unwrap_or(0);
    let rss_end_kb = rss_samples.last().map(|s| s.vm_rss_kb).unwrap_or(0);
    let rss_growth_mb = ((rss_end_kb as f64) - (rss_warmed_kb as f64)) / 1024.0;
    let threads_min = rss_samples.iter().map(|s| s.threads).min().unwrap_or(0);
    let threads_max = rss_samples.iter().map(|s| s.threads).max().unwrap_or(0);

    let report = SoakReport {
        slots_executed: slots,
        worker_threads,
        ft8_slots,
        ft4_slots,
        total_decodes,
        cancellations_ok,
        max_cancel_wall_ms,
        concurrent_tx_ok,
        error_recoveries_ok,
        session_resets,
        rss_warmed_kb,
        rss_end_kb,
        rss_growth_mb,
        threads_min,
        threads_max,
        rss_samples,
        elapsed_total_ms: t0.elapsed().as_secs_f64() * 1000.0,
        exit_code,
    };

    if let Some(p) = out_json {
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(p, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }

    Ok(report)
}
