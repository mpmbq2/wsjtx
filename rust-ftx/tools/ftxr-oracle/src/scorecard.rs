// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Machine-readable JSON v1 baseline scorecard builder, truth matcher, 50% threshold estimator,
//! timing statistics, and scorecard diff/comparison gate evaluator.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use clap::ValueEnum;
use ftxr_core::Mode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::corpus::{
    load_committed_manifest, load_generated_manifest, CommittedFixture, CorpusError, GeneratedCase,
};
use crate::jt9::{
    count_ap_tags, Jt9Decode, Jt9Error, Jt9Profile, Jt9RunConfig, Jt9RunOutput, Jt9Runner,
    OracleApTag,
};
use crate::synth::SignalSpec;
use crate::wav;

#[derive(Debug, Error)]
pub enum ScorecardError {
    #[error("corpus error: {0}")]
    Corpus(#[from] CorpusError),
    #[error("jt9 error: {0}")]
    Jt9(#[from] Jt9Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("comparison gate failed: {0}")]
    CompareFailed(String),
    #[error("checkpoint paused at wall-clock limit ({saved} total entries saved); re-run with same --checkpoint to resume")]
    CheckpointPaused { saved: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BaselineSuiteFilter {
    All,
    Committed,
    Synthetic,
    CommittedAndAwgn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scorecard {
    pub schema_version: u32,
    pub generated_at_utc: String,
    pub host: HostInfo,
    pub engine: EngineInfo,
    pub runs: Vec<ScorecardRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub cpu_count: usize,
    pub rustc_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    pub wsjtx_git_rev: String,
    pub wsjtx_version: String,
    pub fftw_patience: u8,
    pub wisdom_warm: bool,
    pub omp_num_threads: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScorecardRun {
    pub mode: Mode,
    pub depth: u8,
    pub profile: Jt9Profile,
    pub cli_template: String,
    pub committed_fixtures: Vec<CommittedFixtureResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awgn_sweep: Option<AwgnSweepSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crowded: Option<CrowdedSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offsets: Option<SuiteRecallSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_tx: Option<SuiteRecallSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ap_qso: Option<ApQsoSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pure_noise: Option<PureNoiseSummary>,
    pub timing_ms: TimingStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommittedFixtureResult {
    pub fixture_id: String,
    pub sha256: String,
    pub elapsed_ms: f64,
    pub decode_count: usize,
    pub ap_counts: BTreeMap<String, usize>,
    pub uncertain_ap_count: usize,
    pub decodes: Vec<ScorecardDecode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScorecardDecode {
    pub snr_db: i8,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub message: String,
    pub ap_tag: OracleApTag,
    pub ap_uncertain: bool,
}

impl From<&Jt9Decode> for ScorecardDecode {
    fn from(d: &Jt9Decode) -> Self {
        Self {
            snr_db: d.snr_db,
            dt_sec: d.dt_sec,
            af_hz: d.af_hz,
            message: d.message.clone(),
            ap_tag: d.ap_tag,
            ap_uncertain: d.ap_uncertain,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwgnSweepSummary {
    pub trials_per_snr: usize,
    pub total_trials: usize,
    pub total_decoded: usize,
    pub total_false_positives: usize,
    pub threshold_50pct_db: Option<f64>,
    pub points: Vec<AwgnSweepPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwgnSweepPoint {
    pub snr_db: f64,
    pub trials: usize,
    pub decoded: usize,
    pub false_positives: usize,
    pub decode_rate: f64,
    pub mean_snr_err_db: Option<f64>,
    pub mean_abs_af_err_hz: Option<f64>,
    pub mean_abs_dt_err_sec: Option<f64>,
    pub median_elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrowdedSummary {
    pub slots: usize,
    pub total_truth_signals: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub recall: f64,
    pub median_elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteRecallSummary {
    pub cases: usize,
    pub total_truth_signals: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub recall: f64,
    pub median_elapsed_ms: f64,
    pub per_case: Vec<CaseOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseOutcome {
    pub case_id: String,
    pub decoded: bool,
    pub false_positives: usize,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApQsoSummary {
    pub total_cases: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub recall: f64,
    pub ap_counts: BTreeMap<String, usize>,
    pub uncertain_ap_count: usize,
    pub median_elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PureNoiseSummary {
    pub slots: usize,
    pub false_decodes_total: usize,
    pub false_decodes_verified: usize,
    pub false_decodes_ap_certain: usize,
    pub false_decodes_ap_uncertain: usize,
    pub fp_rate_per_slot: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingStats {
    pub min: f64,
    pub median: f64,
    pub p95: f64,
    pub mean: f64,
}

#[derive(Debug, Clone)]
pub struct BaselineOptions {
    pub suite: BaselineSuiteFilter,
    pub modes: Vec<Mode>,
    pub depths: Vec<u8>,
    pub profiles: Vec<Jt9Profile>,
    pub patience: u8,
    pub seed_version: String,
    pub jobs: usize,
    pub checkpoint: Option<PathBuf>,
    pub max_wall_secs: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CheckpointRecord {
    key: String,
    output: Jt9RunOutput,
}

/// Append-only crash-safe JSONL checkpoint store keyed by `(wav_sha256, jt9_cli_template, wisdom_warm)`.
struct CheckpointStore {
    entries: Mutex<HashMap<String, Jt9RunOutput>>,
    writer: Mutex<File>,
}

impl CheckpointStore {
    fn open(path: &Path) -> Result<Self, ScorecardError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut entries = HashMap::new();
        if path.exists() {
            let file = File::open(path)?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(rec) = serde_json::from_str::<CheckpointRecord>(trimmed) {
                    entries.insert(rec.key, rec.output);
                }
            }
        }
        let writer = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            entries: Mutex::new(entries),
            writer: Mutex::new(writer),
        })
    }

    fn get(&self, key: &str) -> Option<Jt9RunOutput> {
        self.entries.lock().unwrap().get(key).cloned()
    }

    fn insert(&self, key: String, output: Jt9RunOutput) -> Result<(), ScorecardError> {
        let rec = CheckpointRecord {
            key: key.clone(),
            output: output.clone(),
        };
        let mut line = serde_json::to_string(&rec)?;
        line.push('\n');
        {
            let mut w = self.writer.lock().unwrap();
            w.write_all(line.as_bytes())?;
            w.flush()?;
        }
        self.entries.lock().unwrap().insert(key, output);
        Ok(())
    }

    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

struct BatchContext<'a> {
    jt9: &'a Jt9Runner,
    wisdom_path: Option<&'a Path>,
    jobs: usize,
    checkpoint: Option<&'a CheckpointStore>,
    deadline: Option<Instant>,
}

struct BatchTask {
    wav_path: PathBuf,
    wav_sha256: String,
    cfg: Jt9RunConfig,
}

impl BatchTask {
    fn checkpoint_key(&self, wisdom_warm: bool) -> String {
        let raw = format!(
            "ckpt-v1|{}|{}|w={wisdom_warm}",
            self.wav_sha256,
            self.cfg.cli_template()
        );
        wav::sha256_bytes(raw.as_bytes())
    }
}

/// Runs the baseline `jt9` evaluation matrix and returns a Schema v1 [`Scorecard`].
pub fn run_baseline(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    cache_dir: &Path,
    jt9: &Jt9Runner,
    opts: &BaselineOptions,
) -> Result<Scorecard, ScorecardError> {
    let committed = load_committed_manifest(rust_ftx_root)?;

    let corpus_dir = cache_dir.join("corpus").join(&opts.seed_version);
    let generated = if opts.suite != BaselineSuiteFilter::Committed {
        Some(load_generated_manifest(&corpus_dir)?)
    } else {
        None
    };

    let ft8_warmup = wsjtx_root.join("samples/FT8/210703_133430.wav");
    let ft4_warmup = wsjtx_root.join("samples/FT4/000000_000002.wav");
    let wisdom_path = jt9.ensure_warm_wisdom(opts.patience, &ft8_warmup, &ft4_warmup)?;

    let checkpoint_store = match &opts.checkpoint {
        Some(p) => {
            let store = CheckpointStore::open(p)?;
            eprintln!(
                "[baseline] loaded {} cached entries from {}",
                store.len(),
                p.display()
            );
            Some(store)
        }
        None => None,
    };

    let deadline = opts
        .max_wall_secs
        .map(|s| Instant::now() + Duration::from_secs(s));
    let ctx = BatchContext {
        jt9,
        wisdom_path: wisdom_path.as_deref(),
        jobs: opts.jobs.max(1),
        checkpoint: checkpoint_store.as_ref(),
        deadline,
    };

    let mut runs = Vec::new();
    for &mode in &opts.modes {
        for &depth in &opts.depths {
            for &profile in &opts.profiles {
                eprintln!(
                    "[baseline] mode={mode:?} depth={depth} profile={} jobs={}",
                    profile.as_str(),
                    ctx.jobs
                );
                let run = execute_single_run(
                    rust_ftx_root,
                    &corpus_dir,
                    &ctx,
                    &committed.fixtures,
                    generated.as_ref().map(|g| g.cases.as_slice()),
                    opts.suite,
                    mode,
                    depth,
                    profile,
                    opts.patience,
                )?;
                runs.push(run);
            }
        }
    }

    Ok(Scorecard {
        schema_version: 1,
        generated_at_utc: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        host: collect_host_info(),
        engine: EngineInfo {
            name: "jt9".to_string(),
            wsjtx_git_rev: git_head_sha(wsjtx_root),
            wsjtx_version: "3.0.2.0".to_string(),
            fftw_patience: opts.patience,
            wisdom_warm: wisdom_path.is_some(),
            omp_num_threads: 1,
        },
        runs,
    })
}

/// Executes a batch of isolated `jt9` invocations across up to `jobs` worker threads,
/// honoring optional checkpoint resume and soft wall-clock pause deadlines, and returning
/// outputs in strict deterministic input order.
fn run_batch_jt9(
    ctx: &BatchContext<'_>,
    tasks: &[BatchTask],
    jobs_override: Option<usize>,
    label: &str,
) -> Result<Vec<Jt9RunOutput>, ScorecardError> {
    let total = tasks.len();
    if total == 0 {
        return Ok(Vec::new());
    }
    let wisdom_warm = ctx.wisdom_path.is_some();
    let workers = jobs_override.unwrap_or(ctx.jobs).max(1).min(total);

    let next_idx = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let cache_hits = AtomicUsize::new(0);
    let paused = AtomicBool::new(false);
    let results: Vec<Mutex<Option<Result<Jt9RunOutput, ScorecardError>>>> =
        (0..total).map(|_| Mutex::new(None)).collect();

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                if paused.load(Ordering::Relaxed) {
                    break;
                }
                let idx = next_idx.fetch_add(1, Ordering::Relaxed);
                if idx >= total {
                    break;
                }
                let task = &tasks[idx];
                let key = task.checkpoint_key(wisdom_warm);

                if let Some(store) = ctx.checkpoint {
                    if let Some(hit) = store.get(&key) {
                        *results[idx].lock().unwrap() = Some(Ok(hit));
                        cache_hits.fetch_add(1, Ordering::Relaxed);
                        completed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }

                if let Some(dl) = ctx.deadline {
                    if Instant::now() >= dl {
                        paused.store(true, Ordering::Relaxed);
                        break;
                    }
                }

                let res = match ctx.jt9.run_wav(&task.wav_path, &task.cfg, ctx.wisdom_path) {
                    Ok(out) => {
                        if let Some(store) = ctx.checkpoint {
                            if let Err(e) = store.insert(key, out.clone()) {
                                Err(e)
                            } else {
                                Ok(out)
                            }
                        } else {
                            Ok(out)
                        }
                    }
                    Err(e) => Err(ScorecardError::Jt9(e)),
                };
                *results[idx].lock().unwrap() = Some(res);
                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                if total >= 100 && (done % 100 == 0 || done == total) {
                    eprintln!("    [{label}] {done}/{total}");
                }
            });
        }
    });

    if paused.load(Ordering::Relaxed) {
        let saved = ctx.checkpoint.map_or(0, CheckpointStore::len);
        return Err(ScorecardError::CheckpointPaused { saved });
    }

    let hits = cache_hits.load(Ordering::Relaxed);
    if hits > 0 && hits == total {
        eprintln!("    [{label}] {total}/{total} (all from checkpoint)");
    }

    let mut out = Vec::with_capacity(total);
    for slot in results {
        let res = slot.into_inner().unwrap().expect("every task executed");
        out.push(res?);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn execute_single_run(
    rust_ftx_root: &Path,
    corpus_dir: &Path,
    ctx: &BatchContext<'_>,
    committed_fixtures: &[CommittedFixture],
    generated_cases: Option<&[GeneratedCase]>,
    suite_filter: BaselineSuiteFilter,
    mode: Mode,
    depth: u8,
    profile: Jt9Profile,
    patience: u8,
) -> Result<ScorecardRun, ScorecardError> {
    let base_cfg = Jt9RunConfig::new(mode, depth, profile, patience);
    let mut all_timings_ms: Vec<f64> = Vec::new();

    // 1. Committed fixtures (always run unless filter == Synthetic).
    let mut committed_results = Vec::new();
    if suite_filter != BaselineSuiteFilter::Synthetic {
        let mode_fixtures: Vec<&CommittedFixture> = committed_fixtures
            .iter()
            .filter(|f| f.mode == mode)
            .collect();
        let tasks: Vec<BatchTask> = mode_fixtures
            .iter()
            .map(|fix| BatchTask {
                wav_path: rust_ftx_root.join(&fix.relative_path),
                wav_sha256: fix.sha256.clone(),
                cfg: base_cfg.clone(),
            })
            .collect();
        let outputs = run_batch_jt9(ctx, &tasks, Some(1), "committed")?;
        for (fix, out) in mode_fixtures.into_iter().zip(outputs) {
            all_timings_ms.push(out.elapsed_ms);
            let ap_counts = count_ap_tags(&out.decodes);
            let uncertain_ap_count = out.decodes.iter().filter(|d| d.ap_uncertain).count();
            let decodes = out.decodes.iter().map(ScorecardDecode::from).collect();
            committed_results.push(CommittedFixtureResult {
                fixture_id: fix.fixture_id.clone(),
                sha256: fix.sha256.clone(),
                elapsed_ms: round3(out.elapsed_ms),
                decode_count: out.decodes.len(),
                ap_counts,
                uncertain_ap_count,
                decodes,
            });
        }
    }

    let mut awgn_sweep = None;
    let mut crowded = None;
    let mut offsets = None;
    let mut late_tx = None;
    let mut ap_qso = None;
    let mut pure_noise = None;

    if let Some(cases) = generated_cases {
        let mode_prefix = match mode {
            Mode::Ft8 => "ft8",
            Mode::Ft4 => "ft4",
        };

        // 2. AWGN sweep: run on blind-no-ap and blind-cq-ap (CQ messages exercise a1 in blind-cq-ap).
        if profile != Jt9Profile::QsoAp {
            let suite_name = format!("{mode_prefix}_awgn");
            let awgn_cases: Vec<&GeneratedCase> =
                cases.iter().filter(|c| c.suite == suite_name).collect();
            if !awgn_cases.is_empty() {
                eprintln!("  -> {suite_name} ({} files)", awgn_cases.len());
                awgn_sweep = Some(eval_awgn_sweep(
                    corpus_dir,
                    ctx,
                    &base_cfg,
                    &awgn_cases,
                    &suite_name,
                    &mut all_timings_ms,
                )?);
            }
        }

        if suite_filter != BaselineSuiteFilter::CommittedAndAwgn {
            // 3. Crowded multi-signal slots (blind profiles).
            if profile != Jt9Profile::QsoAp {
                let suite_name = format!("{mode_prefix}_crowded");
                let crowded_cases: Vec<&GeneratedCase> =
                    cases.iter().filter(|c| c.suite == suite_name).collect();
                if !crowded_cases.is_empty() {
                    eprintln!("  -> {suite_name} ({} files)", crowded_cases.len());
                    crowded = Some(eval_crowded(
                        corpus_dir,
                        ctx,
                        &base_cfg,
                        &crowded_cases,
                        &suite_name,
                        &mut all_timings_ms,
                    )?);
                }
            }

            // 4. Offsets & Late TX (blind-no-ap).
            if profile == Jt9Profile::BlindNoAp {
                let off_suite = format!("{mode_prefix}_offsets");
                let off_cases: Vec<&GeneratedCase> =
                    cases.iter().filter(|c| c.suite == off_suite).collect();
                if !off_cases.is_empty() {
                    eprintln!("  -> {off_suite} ({} files)", off_cases.len());
                    offsets = Some(eval_recall_suite(
                        corpus_dir,
                        ctx,
                        &base_cfg,
                        &off_cases,
                        &off_suite,
                        &mut all_timings_ms,
                    )?);
                }

                let late_suite = format!("{mode_prefix}_late_tx");
                let late_cases: Vec<&GeneratedCase> =
                    cases.iter().filter(|c| c.suite == late_suite).collect();
                if !late_cases.is_empty() {
                    eprintln!("  -> {late_suite} ({} files)", late_cases.len());
                    late_tx = Some(eval_recall_suite(
                        corpus_dir,
                        ctx,
                        &base_cfg,
                        &late_cases,
                        &late_suite,
                        &mut all_timings_ms,
                    )?);
                }
            }

            // 5. AP QSO suite (run on both blind-no-ap and qso-ap for direct AP gain comparison).
            if profile == Jt9Profile::BlindNoAp || profile == Jt9Profile::QsoAp {
                let ap_suite = format!("{mode_prefix}_ap_qso");
                let ap_cases: Vec<&GeneratedCase> =
                    cases.iter().filter(|c| c.suite == ap_suite).collect();
                if !ap_cases.is_empty() {
                    eprintln!("  -> {ap_suite} ({} files)", ap_cases.len());
                    ap_qso = Some(eval_ap_qso(
                        corpus_dir,
                        ctx,
                        &base_cfg,
                        &ap_cases,
                        &ap_suite,
                        &mut all_timings_ms,
                    )?);
                }
            }

            // 6. Pure AWGN noise slots (all profiles: verifies FP rate without AP, with a1, and with a3-a6).
            let noise_suite = format!("{mode_prefix}_noise");
            let noise_cases: Vec<&GeneratedCase> =
                cases.iter().filter(|c| c.suite == noise_suite).collect();
            if !noise_cases.is_empty() {
                eprintln!("  -> {noise_suite} ({} files)", noise_cases.len());
                let mut noise_cfg = base_cfg.clone();
                if profile == Jt9Profile::QsoAp {
                    // Use QSOProgress=3 at 1500 Hz so a3..a6 AP false-positive bounds are actively tested.
                    noise_cfg.my_call = Some("N0AAA".into());
                    noise_cfg.his_call = Some("K9BBB".into());
                    noise_cfg.qso_progress = Some(3);
                    noise_cfg.rx_freq_hz = 1500;
                }
                pure_noise = Some(eval_pure_noise(
                    corpus_dir,
                    ctx,
                    &noise_cfg,
                    &noise_cases,
                    &noise_suite,
                    &mut all_timings_ms,
                )?);
            }
        }
    }

    Ok(ScorecardRun {
        mode,
        depth,
        profile,
        cli_template: base_cfg.cli_template(),
        committed_fixtures: committed_results,
        awgn_sweep,
        crowded,
        offsets,
        late_tx,
        ap_qso,
        pure_noise,
        timing_ms: compute_timing_stats(&all_timings_ms),
    })
}

fn eval_awgn_sweep(
    corpus_dir: &Path,
    ctx: &BatchContext<'_>,
    cfg: &Jt9RunConfig,
    cases: &[&GeneratedCase],
    label: &str,
    all_timings: &mut Vec<f64>,
) -> Result<AwgnSweepSummary, ScorecardError> {
    let tasks: Vec<BatchTask> = cases
        .iter()
        .map(|c| BatchTask {
            wav_path: corpus_dir.join(&c.relative_path),
            wav_sha256: c.sha256.clone(),
            cfg: cfg.clone(),
        })
        .collect();
    let outputs = run_batch_jt9(ctx, &tasks, None, label)?;

    let mut by_snr: BTreeMap<i32, Vec<(&GeneratedCase, Jt9RunOutput)>> = BTreeMap::new();
    for (&case, out) in cases.iter().zip(outputs) {
        let snr_key = (case.snr_db.unwrap_or(0.0) * 10.0).round() as i32;
        by_snr.entry(snr_key).or_default().push((case, out));
    }

    let mut points = Vec::new();
    let mut total_trials = 0usize;
    let mut total_decoded = 0usize;
    let mut total_fps = 0usize;
    let mut trials_per_snr = 0usize;

    for (&snr_tenths, group) in by_snr.iter().rev() {
        let snr_db = f64::from(snr_tenths) / 10.0;
        let trials = group.len();
        trials_per_snr = trials_per_snr.max(trials);
        let mut decoded = 0usize;
        let mut fps = 0usize;
        let mut snr_errs = Vec::new();
        let mut af_abs_errs = Vec::new();
        let mut dt_abs_errs = Vec::new();
        let mut times = Vec::with_capacity(trials);

        for (case, out) in group {
            times.push(out.elapsed_ms);
            all_timings.push(out.elapsed_ms);

            let m = match_truth(&case.slot.signals, &out.decodes);
            decoded += m.true_positives;
            fps += m.false_positives;
            snr_errs.extend(m.snr_errs);
            af_abs_errs.extend(m.af_abs_errs);
            dt_abs_errs.extend(m.dt_abs_errs);
        }

        total_trials += trials;
        total_decoded += decoded;
        total_fps += fps;

        points.push(AwgnSweepPoint {
            snr_db,
            trials,
            decoded,
            false_positives: fps,
            decode_rate: round4(decoded as f64 / trials.max(1) as f64),
            mean_snr_err_db: mean_opt(&snr_errs).map(round3),
            mean_abs_af_err_hz: mean_opt(&af_abs_errs).map(round3),
            mean_abs_dt_err_sec: mean_opt(&dt_abs_errs).map(round4),
            median_elapsed_ms: round3(percentile(&mut times, 0.50)),
        });
    }

    let threshold_50pct_db = estimate_50pct_threshold(&points).map(round3);

    Ok(AwgnSweepSummary {
        trials_per_snr,
        total_trials,
        total_decoded,
        total_false_positives: total_fps,
        threshold_50pct_db,
        points,
    })
}

fn eval_crowded(
    corpus_dir: &Path,
    ctx: &BatchContext<'_>,
    cfg: &Jt9RunConfig,
    cases: &[&GeneratedCase],
    label: &str,
    all_timings: &mut Vec<f64>,
) -> Result<CrowdedSummary, ScorecardError> {
    let tasks: Vec<BatchTask> = cases
        .iter()
        .map(|c| BatchTask {
            wav_path: corpus_dir.join(&c.relative_path),
            wav_sha256: c.sha256.clone(),
            cfg: cfg.clone(),
        })
        .collect();
    let outputs = run_batch_jt9(ctx, &tasks, None, label)?;

    let mut total_truth = 0usize;
    let mut tps = 0usize;
    let mut fps = 0usize;
    let mut times = Vec::with_capacity(cases.len());

    for (&case, out) in cases.iter().zip(outputs) {
        times.push(out.elapsed_ms);
        all_timings.push(out.elapsed_ms);

        total_truth += case.slot.signals.len();
        let m = match_truth(&case.slot.signals, &out.decodes);
        tps += m.true_positives;
        fps += m.false_positives;
    }

    Ok(CrowdedSummary {
        slots: cases.len(),
        total_truth_signals: total_truth,
        true_positives: tps,
        false_positives: fps,
        recall: round4(tps as f64 / total_truth.max(1) as f64),
        median_elapsed_ms: round3(percentile(&mut times, 0.50)),
    })
}

fn eval_recall_suite(
    corpus_dir: &Path,
    ctx: &BatchContext<'_>,
    cfg: &Jt9RunConfig,
    cases: &[&GeneratedCase],
    label: &str,
    all_timings: &mut Vec<f64>,
) -> Result<SuiteRecallSummary, ScorecardError> {
    let tasks: Vec<BatchTask> = cases
        .iter()
        .map(|c| BatchTask {
            wav_path: corpus_dir.join(&c.relative_path),
            wav_sha256: c.sha256.clone(),
            cfg: cfg.clone(),
        })
        .collect();
    let outputs = run_batch_jt9(ctx, &tasks, None, label)?;

    let mut total_truth = 0usize;
    let mut tps = 0usize;
    let mut fps = 0usize;
    let mut times = Vec::with_capacity(cases.len());
    let mut per_case = Vec::with_capacity(cases.len());

    for (&case, out) in cases.iter().zip(outputs) {
        times.push(out.elapsed_ms);
        all_timings.push(out.elapsed_ms);

        total_truth += case.slot.signals.len();
        let m = match_truth(&case.slot.signals, &out.decodes);
        tps += m.true_positives;
        fps += m.false_positives;
        per_case.push(CaseOutcome {
            case_id: case.case_id.clone(),
            decoded: m.true_positives == case.slot.signals.len(),
            false_positives: m.false_positives,
            elapsed_ms: round3(out.elapsed_ms),
        });
    }

    Ok(SuiteRecallSummary {
        cases: cases.len(),
        total_truth_signals: total_truth,
        true_positives: tps,
        false_positives: fps,
        recall: round4(tps as f64 / total_truth.max(1) as f64),
        median_elapsed_ms: round3(percentile(&mut times, 0.50)),
        per_case,
    })
}

fn eval_ap_qso(
    corpus_dir: &Path,
    ctx: &BatchContext<'_>,
    base_cfg: &Jt9RunConfig,
    cases: &[&GeneratedCase],
    label: &str,
    all_timings: &mut Vec<f64>,
) -> Result<ApQsoSummary, ScorecardError> {
    let tasks: Vec<BatchTask> = cases
        .iter()
        .map(|case| {
            let mut cfg = base_cfg.clone();
            if base_cfg.profile == Jt9Profile::QsoAp {
                if let Some(ap) = &case.ap_context {
                    cfg.my_call = Some(ap.my_call.clone());
                    cfg.his_call = Some(ap.his_call.clone());
                    cfg.his_grid = Some(ap.his_grid.clone());
                    cfg.qso_progress = Some(ap.qso_progress);
                    cfg.rx_freq_hz = ap.rx_freq_hz;
                    cfg.allow_a8 = false;
                }
            }
            BatchTask {
                wav_path: corpus_dir.join(&case.relative_path),
                wav_sha256: case.sha256.clone(),
                cfg,
            }
        })
        .collect();
    let outputs = run_batch_jt9(ctx, &tasks, None, label)?;

    let mut tps = 0usize;
    let mut fps = 0usize;
    let mut times = Vec::with_capacity(cases.len());
    let mut all_decodes = Vec::new();

    for (&case, out) in cases.iter().zip(outputs) {
        times.push(out.elapsed_ms);
        all_timings.push(out.elapsed_ms);

        let m = match_truth(&case.slot.signals, &out.decodes);
        tps += m.true_positives;
        fps += m.false_positives;
        all_decodes.extend(out.decodes);
    }

    let ap_counts = count_ap_tags(&all_decodes);
    let uncertain_ap_count = all_decodes.iter().filter(|d| d.ap_uncertain).count();

    Ok(ApQsoSummary {
        total_cases: cases.len(),
        true_positives: tps,
        false_positives: fps,
        recall: round4(tps as f64 / cases.len().max(1) as f64),
        ap_counts,
        uncertain_ap_count,
        median_elapsed_ms: round3(percentile(&mut times, 0.50)),
    })
}

fn eval_pure_noise(
    corpus_dir: &Path,
    ctx: &BatchContext<'_>,
    cfg: &Jt9RunConfig,
    cases: &[&GeneratedCase],
    label: &str,
    all_timings: &mut Vec<f64>,
) -> Result<PureNoiseSummary, ScorecardError> {
    let tasks: Vec<BatchTask> = cases
        .iter()
        .map(|c| BatchTask {
            wav_path: corpus_dir.join(&c.relative_path),
            wav_sha256: c.sha256.clone(),
            cfg: cfg.clone(),
        })
        .collect();
    let outputs = run_batch_jt9(ctx, &tasks, None, label)?;

    let mut total_fp = 0usize;
    let mut verified_fp = 0usize;
    let mut ap_certain_fp = 0usize;
    let mut ap_uncertain_fp = 0usize;

    for out in outputs {
        all_timings.push(out.elapsed_ms);
        for d in &out.decodes {
            total_fp += 1;
            if !d.ap_tag.is_ap() {
                verified_fp += 1;
            } else if d.ap_uncertain {
                ap_uncertain_fp += 1;
            } else {
                ap_certain_fp += 1;
            }
        }
    }

    Ok(PureNoiseSummary {
        slots: cases.len(),
        false_decodes_total: total_fp,
        false_decodes_verified: verified_fp,
        false_decodes_ap_certain: ap_certain_fp,
        false_decodes_ap_uncertain: ap_uncertain_fp,
        fp_rate_per_slot: round5(total_fp as f64 / cases.len().max(1) as f64),
    })
}

struct TruthMatch {
    true_positives: usize,
    false_positives: usize,
    snr_errs: Vec<f64>,
    af_abs_errs: Vec<f64>,
    dt_abs_errs: Vec<f64>,
}

fn normalize_msg(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn match_truth(truth: &[SignalSpec], decodes: &[Jt9Decode]) -> TruthMatch {
    let mut used_truth = vec![false; truth.len()];
    let mut tps = 0usize;
    let mut fps = 0usize;
    let mut snr_errs = Vec::new();
    let mut af_abs_errs = Vec::new();
    let mut dt_abs_errs = Vec::new();

    for d in decodes {
        let d_msg = normalize_msg(&d.message);
        // Pick the closest unmatched truth signal with identical message text.
        let best_idx = truth
            .iter()
            .enumerate()
            .filter(|(idx, t)| !used_truth[*idx] && normalize_msg(&t.message) == d_msg)
            .min_by(|(_, a), (_, b)| {
                let da = (a.af_hz - f64::from(d.af_hz)).abs();
                let db = (b.af_hz - f64::from(d.af_hz)).abs();
                da.total_cmp(&db)
            })
            .map(|(idx, _)| idx);

        if let Some(idx) = best_idx {
            used_truth[idx] = true;
            tps += 1;
            let t = &truth[idx];
            snr_errs.push(f64::from(d.snr_db) - t.snr_db);
            af_abs_errs.push((f64::from(d.af_hz) - t.af_hz).abs());
            dt_abs_errs.push((f64::from(d.dt_sec) - t.dt_sec).abs());
        } else {
            fps += 1;
        }
    }

    TruthMatch {
        true_positives: tps,
        false_positives: fps,
        snr_errs,
        af_abs_errs,
        dt_abs_errs,
    }
}

/// Estimates the 50% decode threshold (in dB) by linear interpolation between the first
/// descending-SNR pair crossing `0.50`.
pub fn estimate_50pct_threshold(points: &[AwgnSweepPoint]) -> Option<f64> {
    if points.is_empty() {
        return None;
    }
    for win in points.windows(2) {
        let hi = &win[0];
        let lo = &win[1];
        if (hi.decode_rate - 0.5).abs() < 1e-9 {
            return Some(hi.snr_db);
        }
        if hi.decode_rate > 0.5 && lo.decode_rate <= 0.5 {
            let denom = hi.decode_rate - lo.decode_rate;
            if denom.abs() < 1e-9 {
                return Some(lo.snr_db);
            }
            let frac = (hi.decode_rate - 0.5) / denom;
            return Some(hi.snr_db + frac * (lo.snr_db - hi.snr_db));
        }
    }
    None
}

fn compute_timing_stats(times: &[f64]) -> TimingStats {
    if times.is_empty() {
        return TimingStats {
            min: 0.0,
            median: 0.0,
            p95: 0.0,
            mean: 0.0,
        };
    }
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let min = sorted[0];
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let median = percentile(&mut sorted, 0.50);
    let p95 = percentile(&mut sorted, 0.95);
    TimingStats {
        min: round3(min),
        median: round3(median),
        p95: round3(p95),
        mean: round3(mean),
    }
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let idx = ((values.len() - 1) as f64 * p).round() as usize;
    values[idx.min(values.len() - 1)]
}

fn mean_opt(vals: &[f64]) -> Option<f64> {
    if vals.is_empty() {
        None
    } else {
        Some(vals.iter().sum::<f64>() / vals.len() as f64)
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn round5(v: f64) -> f64 {
    (v * 100_000.0).round() / 100_000.0
}

fn collect_host_info() -> HostInfo {
    let os = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|txt| {
            txt.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string());

    let rustc_version = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    HostInfo {
        os,
        arch: std::env::consts::ARCH.to_string(),
        cpu_count,
        rustc_version,
    }
}

fn git_head_sha(repo: &Path) -> String {
    Command::new("git")
        .args(["-C", &repo.display().to_string(), "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Compares a candidate scorecard against a baseline scorecard and enforces parity/repeatability gates.
pub fn compare_scorecards(
    baseline_path: &Path,
    candidate_path: &Path,
    max_threshold_delta_db: f64,
    min_decode_ratio: f64,
    require_exact_repeatability: bool,
) -> Result<(), ScorecardError> {
    let base: Scorecard = serde_json::from_str(&fs::read_to_string(baseline_path)?)?;
    let cand: Scorecard = serde_json::from_str(&fs::read_to_string(candidate_path)?)?;

    let mut matched_runs = 0usize;
    let mut failures = Vec::new();

    for c_run in &cand.runs {
        let Some(b_run) = base.runs.iter().find(|r| {
            r.mode == c_run.mode && r.depth == c_run.depth && r.profile == c_run.profile
        }) else {
            continue;
        };
        matched_runs += 1;
        let run_label = format!(
            "{:?}/d{}/{}",
            c_run.mode,
            c_run.depth,
            c_run.profile.as_str()
        );

        // 1. Committed fixtures comparison (excluding deferred a7/a8 if ever present).
        let mut base_total = 0usize;
        let mut cand_total = 0usize;
        let mut exact_set_diffs = 0usize;
        for c_fix in &c_run.committed_fixtures {
            if let Some(b_fix) = b_run
                .committed_fixtures
                .iter()
                .find(|f| f.fixture_id == c_fix.fixture_id)
            {
                let b_msgs: BTreeSet<String> = b_fix
                    .decodes
                    .iter()
                    .filter(|d| !d.ap_tag.is_deferred_a7_a8())
                    .map(|d| d.message.clone())
                    .collect();
                let c_msgs: BTreeSet<String> = c_fix
                    .decodes
                    .iter()
                    .filter(|d| !d.ap_tag.is_deferred_a7_a8())
                    .map(|d| d.message.clone())
                    .collect();
                base_total += b_msgs.len();
                cand_total += c_msgs.len();
                let sym_diff = b_msgs.symmetric_difference(&c_msgs).count();
                exact_set_diffs += sym_diff;
                if sym_diff > 0 && require_exact_repeatability {
                    failures.push(format!(
                        "{run_label} fixture {} decode set diff={sym_diff}",
                        c_fix.fixture_id
                    ));
                }
            }
        }
        if base_total > 0 {
            let ratio = cand_total as f64 / base_total as f64;
            println!(
                "[compare] {run_label} committed: cand={cand_total}/{base_total} (ratio={ratio:.4}, set_diffs={exact_set_diffs})"
            );
            if ratio < min_decode_ratio {
                failures.push(format!(
                    "{run_label} committed decode ratio {ratio:.4} < {min_decode_ratio:.4}"
                ));
            }
        }

        // 2. AWGN 50% threshold & point-level comparison.
        if let (Some(b_awgn), Some(c_awgn)) = (&b_run.awgn_sweep, &c_run.awgn_sweep) {
            let delta = match (b_awgn.threshold_50pct_db, c_awgn.threshold_50pct_db) {
                (Some(bt), Some(ct)) => Some(ct - bt),
                _ => None,
            };
            let point_diffs: usize = b_awgn
                .points
                .iter()
                .zip(c_awgn.points.iter())
                .filter(|(bp, cp)| {
                    (bp.snr_db - cp.snr_db).abs() < 1e-6 && bp.decoded != cp.decoded
                })
                .count();
            println!(
                "[compare] {run_label} awgn: base={:?} dB cand={:?} dB delta={:?} dB (point_diffs={point_diffs})",
                b_awgn.threshold_50pct_db, c_awgn.threshold_50pct_db, delta
            );
            if let Some(d) = delta {
                if d > max_threshold_delta_db {
                    failures.push(format!(
                        "{run_label} AWGN threshold regressed by {d:+.3} dB (> {max_threshold_delta_db:.3} dB)"
                    ));
                }
            }
            if require_exact_repeatability && point_diffs > 0 {
                failures.push(format!(
                    "{run_label} AWGN sweep had {point_diffs} SNR point count diffs"
                ));
            }
        }

        // 3. Crowded recall comparison.
        if let (Some(b_cr), Some(c_cr)) = (&b_run.crowded, &c_run.crowded) {
            let ratio = if b_cr.true_positives > 0 {
                c_cr.true_positives as f64 / b_cr.true_positives as f64
            } else {
                1.0
            };
            println!(
                "[compare] {run_label} crowded: cand={}/{} vs base={}/{} (ratio={ratio:.4})",
                c_cr.true_positives, c_cr.total_truth_signals, b_cr.true_positives, b_cr.total_truth_signals
            );
            if ratio < min_decode_ratio {
                failures.push(format!(
                    "{run_label} crowded decode ratio {ratio:.4} < {min_decode_ratio:.4}"
                ));
            }
            if require_exact_repeatability && c_cr.true_positives != b_cr.true_positives {
                failures.push(format!(
                    "{run_label} crowded TP count mismatch: cand={} vs base={}",
                    c_cr.true_positives, b_cr.true_positives
                ));
            }
        }
    }

    if matched_runs == 0 {
        return Err(ScorecardError::CompareFailed(
            "no matching (mode, depth, profile) runs between baseline and candidate scorecards"
                .to_string(),
        ));
    }

    if !failures.is_empty() {
        return Err(ScorecardError::CompareFailed(failures.join("; ")));
    }

    println!("Scorecard comparison passed across {matched_runs} matched run(s).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_50pct_threshold_linearly() {
        let points = vec![
            AwgnSweepPoint {
                snr_db: -19.0,
                trials: 20,
                decoded: 18,
                false_positives: 0,
                decode_rate: 0.90,
                mean_snr_err_db: None,
                mean_abs_af_err_hz: None,
                mean_abs_dt_err_sec: None,
                median_elapsed_ms: 100.0,
            },
            AwgnSweepPoint {
                snr_db: -20.0,
                trials: 20,
                decoded: 15,
                false_positives: 0,
                decode_rate: 0.75,
                mean_snr_err_db: None,
                mean_abs_af_err_hz: None,
                mean_abs_dt_err_sec: None,
                median_elapsed_ms: 100.0,
            },
            AwgnSweepPoint {
                snr_db: -21.0,
                trials: 20,
                decoded: 5,
                false_positives: 0,
                decode_rate: 0.25,
                mean_snr_err_db: None,
                mean_abs_af_err_hz: None,
                mean_abs_dt_err_sec: None,
                median_elapsed_ms: 100.0,
            },
        ];
        let t = estimate_50pct_threshold(&points).unwrap();
        assert!((t - (-20.5)).abs() < 1e-6);
    }
}
