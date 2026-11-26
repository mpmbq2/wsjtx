// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2026 WSJT Development Group).

//! Stage 5 warm IPC 3-pass latency, concurrency, and jt9 oracle benchmark (`bench-3pass`).

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use ftxr_core::Mode;
use serde::Serialize;
use thiserror::Error;

use crate::ipc_client::{
    read_proc_status, resolve_modem_binary, FtxrIpcClient, IpcClientError,
};
use crate::jt9::{Jt9Error, Jt9Profile, Jt9RunConfig, Jt9Runner};
use crate::wav::{read_wav_12k_mono, write_wav_12k_mono, WavError};

#[derive(Debug, Error)]
pub enum Bench3PassError {
    #[error("IPC client error: {0}")]
    Ipc(#[from] IpcClientError),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("jt9 error: {0}")]
    Jt9(#[from] Jt9Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencyStatsMs {
    pub count: usize,
    pub min: f64,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

impl LatencyStatsMs {
    pub fn from_samples(raw: &[f64]) -> Self {
        if raw.is_empty() {
            return Self {
                count: 0,
                min: 0.0,
                median: 0.0,
                p95: 0.0,
                max: 0.0,
                mean: 0.0,
            };
        }
        let mut s = raw.to_vec();
        s.sort_by(f64::total_cmp);
        let n = s.len();
        let median = if n % 2 == 1 {
            s[n / 2]
        } else {
            0.5 * (s[n / 2 - 1] + s[n / 2])
        };
        let p95_idx = ((n as f64) * 0.95).floor() as usize;
        let p95 = s[p95_idx.min(n - 1)];
        let mean = s.iter().sum::<f64>() / (n as f64);
        Self {
            count: n,
            min: s[0],
            median,
            p95,
            max: s[n - 1],
            mean,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PassComparison {
    pub label: String,
    pub samples_12k: usize,
    pub early_pass: bool,
    pub rust_decodes: usize,
    pub jt9_decodes: Option<usize>,
    pub rust_stats_ms: LatencyStatsMs,
    pub jt9_stats_ms: Option<LatencyStatsMs>,
}

#[derive(Debug, Serialize)]
pub struct Fixture3PassReport {
    pub fixture_id: String,
    pub mode: Mode,
    pub passes: Vec<PassComparison>,
}

#[derive(Debug, Serialize)]
pub struct Bench3PassReport {
    pub host_cpus: usize,
    pub worker_threads: usize,
    pub worker_version: String,
    pub repeats: usize,
    pub fixtures: Vec<Fixture3PassReport>,
    pub telemetry_disabled_ms: LatencyStatsMs,
    pub telemetry_enabled_ms: LatencyStatsMs,
    pub telemetry_overhead_pct: f64,
    pub concurrent_encode_tx_ms: LatencyStatsMs,
    pub worker_rss_after_kb: Option<u64>,
    pub worker_threads_after: Option<usize>,
    pub elapsed_total_ms: f64,
}

fn slice_trunc(samples: &[i16], n: usize) -> Vec<i16> {
    let take = samples.len().min(n);
    samples[..take].to_vec()
}

pub fn run_bench_3pass(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    jt9: &Jt9Runner,
    modem_bin: Option<&Path>,
    threads: Option<usize>,
    repeats: usize,
    out_json: Option<&Path>,
) -> Result<Bench3PassReport, Bench3PassError> {
    let t_all = Instant::now();
    let repeats = repeats.max(1);
    let bin_path = resolve_modem_binary(rust_ftx_root, modem_bin)?;

    let ft8_warm = wsjtx_root.join("samples/FT8/210703_133430.wav");
    let ft4_warm = wsjtx_root.join("samples/FT4/000000_000002.wav");
    let wisdom = jt9.ensure_warm_wisdom(1, &ft8_warm, &ft4_warm)?;

    let mut client = FtxrIpcClient::spawn(&bin_path, threads, false)?;
    let host_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    let ft8_fixtures: Vec<(&str, std::path::PathBuf)> = vec![
        ("ft8/210703_133430", wsjtx_root.join("samples/FT8/210703_133430.wav")),
        (
            "ft8/191111_110200",
            rust_ftx_root.join("fixtures/committed/ft8/191111_110200.wav"),
        ),
        (
            "ft8/000000_000101",
            rust_ftx_root.join("fixtures/committed/ft8/000000_000101.wav"),
        ),
    ];

    let ft4_fixtures: Vec<(&str, std::path::PathBuf)> = vec![
        ("ft4/000000_000002", wsjtx_root.join("samples/FT4/000000_000002.wav")),
        (
            "ft4/000000_000201",
            rust_ftx_root.join("fixtures/committed/ft4/000000_000201.wav"),
        ),
    ];

    let mut reports = Vec::new();
    let tmp = tempfile::tempdir()?;

    // Warmup slot so Rayon workers and page tables are hot before timed loops.
    let warm_samples = read_wav_12k_mono(&ft8_warm)?;
    let _ = client.decode_slot_i16(Mode::Ft8, 3, &warm_samples, 1500.0, false, false, None, None);

    // 1. FT8 3-pass cadence (11.8s / 13.5s / 14.8s)
    let ft8_schedule = [
        ("pass1_11.8s", 141_600usize, true),
        ("pass2_13.5s", 162_000usize, true),
        ("pass3_14.8s", 177_600usize, false),
    ];

    for (fid, path) in &ft8_fixtures {
        let full = read_wav_12k_mono(path)?;
        let mut passes = Vec::new();
        for &(label, len, early) in &ft8_schedule {
            let chunk = slice_trunc(&full, len);
            let mut rust_ms = Vec::with_capacity(repeats);
            let mut rust_decodes = 0usize;
            for r in 0..repeats {
                let _ = client.session_reset();
                let res = client.decode_slot_i16(
                    Mode::Ft8,
                    3,
                    &chunk,
                    1500.0,
                    early,
                    false,
                    None,
                    Some(&format!("{fid}_{label}_{r}")),
                )?;
                rust_decodes = res.decoded_count;
                rust_ms.push(res.wall_ms);
            }

            // Benchmark Pass 3 vs warm-wisdom jt9 subprocess.
            let (jt9_decodes, jt9_stats_ms) = if !early {
                let trunc_wav = tmp.path().join("000000_000099.wav");
                let mut padded = chunk.clone();
                if padded.len() < full.len() {
                    padded.resize(full.len(), 0);
                }
                write_wav_12k_mono(&trunc_wav, &padded)?;
                let jt9_cfg = Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::BlindCqAp, 1);
                let mut j_ms = Vec::with_capacity(repeats);
                let mut j_cnt = 0usize;
                for _ in 0..repeats {
                    let jr = jt9.run_wav(&trunc_wav, &jt9_cfg, wisdom.as_deref())?;
                    j_cnt = jr.decodes.len();
                    j_ms.push(jr.elapsed_ms);
                }
                (Some(j_cnt), Some(LatencyStatsMs::from_samples(&j_ms)))
            } else {
                (None, None)
            };

            passes.push(PassComparison {
                label: label.to_string(),
                samples_12k: len,
                early_pass: early,
                rust_decodes,
                jt9_decodes,
                rust_stats_ms: LatencyStatsMs::from_samples(&rust_ms),
                jt9_stats_ms,
            });
        }
        reports.push(Fixture3PassReport {
            fixture_id: (*fid).to_string(),
            mode: Mode::Ft8,
            passes,
        });
    }

    // 2. FT4 3-pass cadence (5.0s / 5.5s / 6.05s)
    let ft4_schedule = [
        ("pass1_5.0s", 60_000usize, true),
        ("pass2_5.5s", 66_000usize, true),
        ("pass3_full", 72_576usize, false),
    ];

    for (fid, path) in &ft4_fixtures {
        let full = read_wav_12k_mono(path)?;
        let mut passes = Vec::new();
        for &(label, len, early) in &ft4_schedule {
            let chunk = slice_trunc(&full, len);
            let mut rust_ms = Vec::with_capacity(repeats);
            let mut rust_decodes = 0usize;
            for r in 0..repeats {
                let _ = client.session_reset();
                let res = client.decode_slot_i16(
                    Mode::Ft4,
                    3,
                    &chunk,
                    1500.0,
                    early,
                    false,
                    None,
                    Some(&format!("{fid}_{label}_{r}")),
                )?;
                rust_decodes = res.decoded_count;
                rust_ms.push(res.wall_ms);
            }

            let (jt9_decodes, jt9_stats_ms) = if !early {
                let j_wav = tmp.path().join("000000_000098.wav");
                write_wav_12k_mono(&j_wav, &chunk)?;
                let jt9_cfg = Jt9RunConfig::new(Mode::Ft4, 3, Jt9Profile::BlindCqAp, 1);
                let mut j_ms = Vec::with_capacity(repeats);
                let mut j_cnt = 0usize;
                for _ in 0..repeats {
                    let jr = jt9.run_wav(&j_wav, &jt9_cfg, wisdom.as_deref())?;
                    j_cnt = jr.decodes.len();
                    j_ms.push(jr.elapsed_ms);
                }
                (Some(j_cnt), Some(LatencyStatsMs::from_samples(&j_ms)))
            } else {
                (None, None)
            };

            passes.push(PassComparison {
                label: label.to_string(),
                samples_12k: len,
                early_pass: early,
                rust_decodes,
                jt9_decodes,
                rust_stats_ms: LatencyStatsMs::from_samples(&rust_ms),
                jt9_stats_ms,
            });
        }
        reports.push(Fixture3PassReport {
            fixture_id: (*fid).to_string(),
            mode: Mode::Ft4,
            passes,
        });
    }

    // 3. Telemetry off vs on overhead benchmark on crowded 210703_133430 Pass 3
    let heavy_ft8 = slice_trunc(&warm_samples, 177_600);
    let mut off_ms = Vec::new();
    let mut on_ms = Vec::new();
    for i in 0..repeats.max(5) {
        let _ = client.session_reset();
        let r0 = client.decode_slot_i16(
            Mode::Ft8,
            3,
            &heavy_ft8,
            1500.0,
            false,
            false,
            None,
            Some(&format!("telem_off_{i}")),
        )?;
        off_ms.push(r0.worker_elapsed_ms);

        let _ = client.session_reset();
        let r1 = client.decode_slot_i16(
            Mode::Ft8,
            3,
            &heavy_ft8,
            1500.0,
            false,
            true,
            None,
            Some(&format!("telem_on_{i}")),
        )?;
        on_ms.push(r1.worker_elapsed_ms);
    }
    let off_stats = LatencyStatsMs::from_samples(&off_ms);
    let on_stats = LatencyStatsMs::from_samples(&on_ms);
    let telemetry_overhead_pct = if off_stats.median > 0.0 {
        ((on_stats.median - off_stats.median) / off_stats.median) * 100.0
    } else {
        0.0
    };

    // 4. Concurrent encode_tx during heavy Pass 3 RX saturation
    let mut tx_latencies = Vec::new();
    for r in 0..5 {
        let _dec_id = client.start_decode_slot_i16(Mode::Ft8, 3, &heavy_ft8, false, None)?;
        std::thread::sleep(Duration::from_millis(2));
        let _tx_id = client.send_encode_tx(
            Mode::Ft8,
            &format!("CQ K1AB{} FN42", r),
            1500.0,
            false,
        )?;
        let (_evs, status, lats) = client.wait_until_idle(Duration::from_secs(10))?;
        assert_eq!(status.as_deref(), Some("completed"));
        tx_latencies.extend(lats);
    }
    let tx_stats = LatencyStatsMs::from_samples(&tx_latencies);

    let proc_after = read_proc_status(client.pid());
    let worker_threads = client.worker_threads;
    let worker_version = client.worker_version.clone();
    let exit = client.shutdown()?;
    assert!(exit.success(), "worker exited with {exit}");

    let report = Bench3PassReport {
        host_cpus,
        worker_threads,
        worker_version,
        repeats,
        fixtures: reports,
        telemetry_disabled_ms: off_stats,
        telemetry_enabled_ms: on_stats,
        telemetry_overhead_pct,
        concurrent_encode_tx_ms: tx_stats,
        worker_rss_after_kb: proc_after.as_ref().map(|p| p.vm_rss_kb),
        worker_threads_after: proc_after.as_ref().map(|p| p.threads),
        elapsed_total_ms: t_all.elapsed().as_secs_f64() * 1000.0,
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
