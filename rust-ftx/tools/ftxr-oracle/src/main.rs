// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Development-only WSJT-X oracle harness (`ftxr-oracle`).
//!
//! Subcommands: `smoke`, `init-committed`, `generate`, `run-jt9`, `baseline`, `compare`, `codec-diff`, `tx-loopback`, `stage2-verify`, `ft8-mvp-bench`, `osd-oracle`, `ft8-stage3`, `ft4-stage4`.

mod codec_oracle;
mod corpus;
mod ft4_stage4;
mod ft8_mvp_bench;
mod ft8_stage3;
mod jt9;
mod osd_oracle;
mod scorecard;
mod sim;
mod smoke;
mod stage2_verify;
mod synth;
mod tx_loopback;
mod wav;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ftxr_core::Mode;

use crate::corpus::{generate_synthetic_corpus, init_committed_fixtures, SynthSuite};
use crate::jt9::{Jt9Profile, Jt9RunConfig, Jt9Runner};
use crate::scorecard::{compare_scorecards, run_baseline, BaselineOptions, BaselineSuiteFilter};
use crate::sim::SimRunner;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliMode {
    Ft8,
    Ft4,
}

impl From<CliMode> for Mode {
    fn from(value: CliMode) -> Self {
        match value {
            CliMode::Ft8 => Mode::Ft8,
            CliMode::Ft4 => Mode::Ft4,
        }
    }
}

impl std::fmt::Display for CliMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliMode::Ft8 => f.write_str("ft8"),
            CliMode::Ft4 => f.write_str("ft4"),
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "ftxr-oracle",
    version,
    about = "WSJT-X oracle test, deterministic synthetic corpus generator, and baseline benchmark harness"
)]
struct Cli {
    /// Directory containing compiled WSJT-X oracle binaries (jt9, ft8code, ft4code, ft8sim, ft4sim, ft4sim_mult).
    #[arg(long, env = "FTXR_ORACLE_BIN_DIR")]
    oracle_bin_dir: Option<PathBuf>,

    /// Cache directory for clean templates, FFTW wisdom, and generated synthetic corpora.
    #[arg(long, env = "FTXR_ORACLE_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Concurrency limit (kept at 1 for deterministic timing runs).
    #[arg(long, default_value_t = 1)]
    jobs: usize,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Verify all 6 WSJT-X oracle binaries build, run cleanly, and pass functional smoke checks.
    Smoke,

    /// Generate/freeze the compact committed regression fixtures and write fixtures/committed/manifest.json.
    InitCommitted {
        /// Overwrite existing frozen fixtures (including stochastic Watterson fading).
        #[arg(long)]
        overwrite: bool,
    },

    /// Generate deterministic seeded synthetic WAV suites under <cache-dir>/corpus/<seed-version>/.
    Generate {
        /// Suite to generate.
        #[arg(long, value_enum, default_value_t = SynthSuite::All)]
        suite: SynthSuite,

        /// Trials per SNR point for AWGN sweeps (default 20; use 100 for high-precision cliff curves).
        #[arg(long, default_value_t = 20)]
        trials: usize,

        /// Number of pure AWGN noise slots per mode (default 100; use 500 for full FP sweeps).
        #[arg(long, default_value_t = 100)]
        noise_trials: usize,

        /// Deterministic seed schema version tag.
        #[arg(long, default_value = "v1")]
        seed_version: String,

        /// Optional custom output directory (defaults to <cache-dir>/corpus/<seed-version>).
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },

    /// Execute a single isolated jt9 subprocess on one WAV file and emit parsed JSON.
    RunJt9 {
        #[arg(long, value_enum)]
        mode: CliMode,
        #[arg(long, default_value_t = 3)]
        depth: u8,
        #[arg(long, value_enum, default_value_t = Jt9Profile::BlindNoAp)]
        profile: Jt9Profile,
        #[arg(long, default_value_t = 1)]
        patience: u8,
        #[arg(long)]
        my_call: Option<String>,
        #[arg(long)]
        his_call: Option<String>,
        #[arg(long)]
        his_grid: Option<String>,
        #[arg(long)]
        qso_progress: Option<u8>,
        #[arg(long, default_value_t = 1500)]
        rx_freq: u16,
        #[arg(long)]
        allow_a8: bool,
        wav: PathBuf,
    },

    /// Capture a deterministic machine-readable JSON v1 baseline scorecard for classic jt9.
    Baseline {
        /// Which corpus partition to run.
        #[arg(long, value_enum, default_value_t = BaselineSuiteFilter::All)]
        suite: BaselineSuiteFilter,

        /// Comma-separated modes to evaluate (e.g., ft8,ft4).
        #[arg(long, value_delimiter = ',', default_values_t = [CliMode::Ft8, CliMode::Ft4])]
        modes: Vec<CliMode>,

        /// Comma-separated decode depths (1,2,3).
        #[arg(long, value_delimiter = ',', default_values_t = [1, 2, 3])]
        depths: Vec<u8>,

        /// Comma-separated invocation profiles.
        #[arg(long, value_delimiter = ',', default_values_t = [Jt9Profile::BlindNoAp, Jt9Profile::BlindCqAp, Jt9Profile::QsoAp])]
        profiles: Vec<Jt9Profile>,

        /// FFTW planner patience level (0=ESTIMATE, 1=ESTIMATE_PATIENT).
        #[arg(long, default_value_t = 1)]
        patience: u8,

        /// Seed version directory inside <cache-dir>/corpus/.
        #[arg(long, default_value = "v1")]
        seed_version: String,

        /// Destination JSON scorecard file path.
        #[arg(long)]
        out: PathBuf,

        /// Optional append-only JSONL checkpoint file for resumable baseline runs.
        #[arg(long)]
        checkpoint: Option<PathBuf>,

        /// Optional soft wall-clock limit in seconds; pauses cleanly with exit code 3 when reached.
        #[arg(long)]
        max_wall_secs: Option<u64>,
    },

    /// Compare a candidate scorecard against a baseline scorecard and enforce parity/repeatability gates.
    Compare {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long, default_value_t = 0.5)]
        max_threshold_delta_db: f64,
        #[arg(long, default_value_t = 0.95)]
        min_decode_ratio: f64,
        /// Require 0 decode-set and 0 SNR-point count deltas (for determinism verification).
        #[arg(long)]
        exact: bool,
    },

    /// Differential fuzzing and golden-table verification of Rust 77-bit/FEC/tones vs ft8code/ft4code.
    CodecDiff {
        /// Deterministic PRNG seed.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Number of fuzzed messages per mode (FT8 and FT4).
        #[arg(long, default_value_t = 5000)]
        count: usize,
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Waveform numerical diff (ft8sim/ft4sim) and TX-to-jt9 loopback verification suite.
    TxLoopback {
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Stage 2 stage-by-stage Fortran numerical verification (ft8_stage_dump vs ftxr-core).
    Stage2Verify {
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Stage 2 end-to-end FT8 decoder MVP benchmark and gate verification.
    Ft8MvpBench {
        /// AWGN trials per SNR for Rust decoder (0..-18 dB).
        #[arg(long, default_value_t = 50)]
        trials: usize,
        /// AWGN trials per SNR for jt9 oracle cross-check.
        #[arg(long, default_value_t = 10)]
        jt9_trials: usize,
        /// Number of pure-AWGN noise slots for false-positive check.
        #[arg(long, default_value_t = 100)]
        noise_slots: usize,
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Stage 3 Suite A: Fortran `osd174_91` vs Rust bit-exact OSD oracle.
    OsdOracle {
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Stage 3 FT8 weak-signal parity suites (B, C, D, E).
    Ft8Stage3 {
        /// Comma-separated suites to run (b, c, d, e, or all).
        #[arg(long, value_delimiter = ',', default_values_t = ["all".to_string()])]
        suites: Vec<String>,
        /// AWGN trials per SNR for Rust decoder (Suite C).
        #[arg(long, default_value_t = 100)]
        trials: usize,
        /// AWGN trials per SNR for jt9 oracle cross-check (Suite C).
        #[arg(long, default_value_t = 100)]
        jt9_trials: usize,
        /// Number of pure-AWGN noise slots for false-positive audit (Suite E).
        #[arg(long, default_value_t = 500)]
        noise_slots: usize,
        /// Optional comma-separated subset of Suite C cliff configs to run.
        #[arg(long, value_delimiter = ',')]
        cliff_configs: Option<Vec<String>>,
        /// Optional wall-clock checkpoint limit (seconds); pauses cleanly before next step.
        #[arg(long)]
        max_wall_secs: Option<u64>,
        /// Enable fresh-binary incremental step caching under `.oracle-cache/stage3-resume/`.
        #[arg(long)]
        resume: bool,
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Execute Stage 4 FT4 RX parity & exit-gate oracle verification harness.
    Ft4Stage4 {
        /// Comma-separated suites to run (all, dump-diff, real-corpus, crowded, awgn, ap-matrix, noise, loopback).
        #[arg(long, value_enum, value_delimiter = ',', default_values_t = [ft4_stage4::Ft4Stage4Suite::All])]
        suites: Vec<ft4_stage4::Ft4Stage4Suite>,
        /// AWGN trials per SNR point in -13..-21 dB (default 100 for Stage 4 gate).
        #[arg(long, default_value_t = 100)]
        trials: usize,
        /// Pure-AWGN noise slots per mode (default 300 for Stage 4 gate).
        #[arg(long, default_value_t = 300)]
        noise_slots: usize,
        /// Optional JSON report output path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn default_rust_ftx_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn default_wsjtx_root(rust_ftx_root: &Path) -> PathBuf {
    rust_ftx_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| rust_ftx_root.to_path_buf())
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let rust_ftx_root = default_rust_ftx_root();
    let wsjtx_root = default_wsjtx_root(&rust_ftx_root);
    let bin_dir = cli
        .oracle_bin_dir
        .unwrap_or_else(|| rust_ftx_root.join(".oracle-build"));
    let cache_dir = cli
        .cache_dir
        .unwrap_or_else(|| rust_ftx_root.join(".oracle-cache"));
    let jobs = cli.jobs.max(1);

    match cli.command {
        Commands::Smoke => {
            smoke::run_smoke(&wsjtx_root, &bin_dir, &cache_dir)?;
        }
        Commands::InitCommitted { overwrite } => {
            let sim = SimRunner::new(bin_dir, cache_dir);
            let manifest = init_committed_fixtures(&rust_ftx_root, &sim, overwrite)?;
            let total_committed_bytes: u64 = manifest
                .fixtures
                .iter()
                .filter(|f| f.relative_path.starts_with("fixtures/committed/"))
                .map(|f| f.size_bytes)
                .sum();
            println!(
                "Initialized committed manifest ({} fixtures, {:.2} MiB under fixtures/committed/).",
                manifest.fixtures.len(),
                total_committed_bytes as f64 / (1024.0 * 1024.0)
            );
        }
        Commands::Generate {
            suite,
            trials,
            noise_trials,
            seed_version,
            out_dir,
        } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let target_dir = out_dir.unwrap_or_else(|| cache_dir.join("corpus").join(&seed_version));
            let manifest = generate_synthetic_corpus(
                &sim,
                &target_dir,
                suite,
                trials,
                noise_trials,
                &seed_version,
            )?;
            println!(
                "Generated {} synthetic WAV case(s) in {} (seed_version={}).",
                manifest.cases.len(),
                target_dir.display(),
                manifest.seed_version
            );
        }
        Commands::RunJt9 {
            mode,
            depth,
            profile,
            patience,
            my_call,
            his_call,
            his_grid,
            qso_progress,
            rx_freq,
            allow_a8,
            wav,
        } => {
            let jt9 = Jt9Runner::new(bin_dir, cache_dir);
            let ft8_warmup = wsjtx_root.join("samples/FT8/210703_133430.wav");
            let ft4_warmup = wsjtx_root.join("samples/FT4/000000_000002.wav");
            let wisdom = jt9.ensure_warm_wisdom(patience, &ft8_warmup, &ft4_warmup)?;
            let mut cfg = Jt9RunConfig::new(mode.into(), depth, profile, patience);
            cfg.my_call = my_call;
            cfg.his_call = his_call;
            cfg.his_grid = his_grid;
            cfg.qso_progress = qso_progress;
            cfg.rx_freq_hz = rx_freq;
            cfg.allow_a8 = allow_a8;
            let res = jt9.run_wav(&wav, &cfg, wisdom.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        Commands::Baseline {
            suite,
            modes,
            depths,
            profiles,
            patience,
            seed_version,
            out,
            checkpoint,
            max_wall_secs,
        } => {
            let jt9 = Jt9Runner::new(bin_dir, cache_dir.clone());
            let opts = BaselineOptions {
                suite,
                modes: modes.into_iter().map(Mode::from).collect(),
                depths,
                profiles,
                patience,
                seed_version,
                jobs,
                checkpoint,
                max_wall_secs,
            };
            let sc = run_baseline(&wsjtx_root, &rust_ftx_root, &cache_dir, &jt9, &opts)?;
            if let Some(parent) = out.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            let json = serde_json::to_string_pretty(&sc)?;
            fs::write(&out, format!("{json}\n"))?;
            println!(
                "Wrote baseline scorecard ({} run(s)) to {}",
                sc.runs.len(),
                out.display()
            );
        }
        Commands::Compare {
            baseline,
            candidate,
            max_threshold_delta_db,
            min_decode_ratio,
            exact,
        } => {
            compare_scorecards(
                &baseline,
                &candidate,
                max_threshold_delta_db,
                min_decode_ratio,
                exact,
            )?;
        }
        Commands::CodecDiff { seed, count, out } => {
            let sim = SimRunner::new(bin_dir, cache_dir);
            codec_oracle::run_codec_diff(&sim, seed, count, out.as_deref())?;
        }
        Commands::TxLoopback { out } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let jt9 = Jt9Runner::new(sim.bin_path("jt9").parent().unwrap().to_path_buf(), cache_dir);
            tx_loopback::run_tx_loopback(&wsjtx_root, &sim, &jt9, out.as_deref())?;
        }
        Commands::Stage2Verify { out } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let resolved_bin_dir = sim.bin_path("jt9").parent().unwrap().to_path_buf();
            stage2_verify::run_stage2_verify(
                &wsjtx_root,
                &rust_ftx_root,
                &resolved_bin_dir,
                &cache_dir,
                out.as_deref(),
            )?;
        }
        Commands::Ft8MvpBench {
            trials,
            jt9_trials,
            noise_slots,
            out,
        } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let jt9 = Jt9Runner::new(sim.bin_path("jt9").parent().unwrap().to_path_buf(), cache_dir);
            ft8_mvp_bench::run_ft8_mvp_bench(
                &wsjtx_root,
                &rust_ftx_root,
                &jt9,
                trials,
                jt9_trials,
                noise_slots,
                out.as_deref(),
            )?;
        }
        Commands::OsdOracle { out } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let resolved_bin_dir = sim.bin_path("jt9").parent().unwrap().to_path_buf();
            osd_oracle::run_suite_a_osd_oracle(
                &rust_ftx_root,
                &resolved_bin_dir,
                &cache_dir,
                out.as_deref(),
            )?;
        }
        Commands::Ft8Stage3 {
            suites,
            trials,
            jt9_trials,
            noise_slots,
            cliff_configs,
            max_wall_secs,
            resume,
            out,
        } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let resolved_bin_dir = sim.bin_path("jt9").parent().unwrap().to_path_buf();
            let jt9 = Jt9Runner::new(resolved_bin_dir.clone(), cache_dir.clone());
            let opts = ft8_stage3::Stage3Options {
                suites: suites.into_iter().collect(),
                trials,
                jt9_trials,
                noise_slots,
                jobs: cli.jobs,
                cliff_configs: cliff_configs.map(|v| v.into_iter().collect()),
                max_wall_secs,
                resume_cache: resume,
            };
            ft8_stage3::run_stage3_suites(
                &wsjtx_root,
                &rust_ftx_root,
                &resolved_bin_dir,
                &cache_dir,
                &jt9,
                &opts,
                out.as_deref(),
            )?;
        }
        Commands::Ft4Stage4 {
            suites,
            trials,
            noise_slots,
            out,
        } => {
            let sim = SimRunner::new(bin_dir, cache_dir.clone());
            let resolved_bin_dir = sim.bin_path("jt9").parent().unwrap().to_path_buf();
            let jt9 = Jt9Runner::new(resolved_bin_dir.clone(), cache_dir.clone());
            let opts = ft4_stage4::Stage4Options {
                suites: suites.into_iter().collect(),
                trials,
                noise_slots,
                jobs,
            };
            ft4_stage4::run_stage4_suites(
                &wsjtx_root,
                &rust_ftx_root,
                &resolved_bin_dir,
                &cache_dir,
                &jt9,
                &opts,
                out.as_deref(),
            )?;
        }
    }

    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if let Some(scorecard::ScorecardError::CheckpointPaused { .. }) =
                err.downcast_ref::<scorecard::ScorecardError>()
            {
                eprintln!("{err}");
                return ExitCode::from(3);
            }
            if let Some(ft8_stage3::Stage3Error::CheckpointPaused { .. }) =
                err.downcast_ref::<ft8_stage3::Stage3Error>()
            {
                eprintln!("{err}");
                return ExitCode::from(3);
            }
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
