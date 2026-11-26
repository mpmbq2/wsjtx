// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Clap CLI argument definitions for `ftxr-modem` standalone CLI and `--worker` daemon mode.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use ftxr_core::{ContestMode, Mode};

use crate::format::OutputFormat;
use crate::wav::SampleFormat;

pub const SHORT_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (ipc-v1)");

pub const LONG_VERSION: &str = concat!(
    "ftxr-modem ",
    env!("CARGO_PKG_VERSION"),
    " (ipc-v1)\n",
    "Copyright (C) 2026 ftxr-modem Contributors\n",
    "License GPLv3: GNU General Public License version 3 <https://www.gnu.org/licenses/gpl-3.0.html>.\n",
    "This is free software: you are free to change and redistribute it.\n",
    "There is NO WARRANTY, to the extent permitted by law.\n\n",
    "Signal processing and protocol algorithms derived from WSJT-X\n",
    "(Copyright (C) 2001-2026 WSJT Development Group, licensed GPLv3)."
);

#[derive(Debug, Parser)]
#[command(
    name = "ftxr-modem",
    about = "Pure-Rust standalone FT8 & FT4 modem CLI and warm stdio IPC v1 worker",
    disable_version_flag = true
)]
pub struct Cli {
    /// Print short version line (`ftxr-modem <semver> (ipc-v1)`) or JSON (`--json`) and exit cleanly.
    #[arg(long = "version", short = 'V')]
    pub version: bool,

    /// Emit JSON metadata for `--version`.
    #[arg(long = "json")]
    pub json: bool,

    /// Print full GPL-3.0-only and WSJT-X attribution license notice and exit cleanly.
    #[arg(long = "license")]
    pub license: bool,

    /// Run persistent warm stdio IPC v1 worker daemon reading/writing `FTXR` framed packets.
    #[arg(long = "worker")]
    pub worker: bool,

    /// Override Rayon RX worker threads (`auto` if omitted; applies to `--worker`).
    #[arg(long = "threads", value_name = "N")]
    pub threads: Option<usize>,

    /// Tracing log filter level emitted strictly to stderr (default: `warn`).
    #[arg(long = "log-level", default_value = "warn")]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliMode {
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

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliRawFormat {
    I16le,
    F32le,
}

impl From<CliRawFormat> for SampleFormat {
    fn from(value: CliRawFormat) -> Self {
        match value {
            CliRawFormat::I16le => SampleFormat::I16Le,
            CliRawFormat::F32le => SampleFormat::F32Le,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliContest {
    None,
    NaVhf,
    EuVhf,
    FieldDay,
    RttyRu,
    WwDigi,
    Fox,
    Hound,
    ArrlDigi,
}

impl From<CliContest> for ContestMode {
    fn from(value: CliContest) -> Self {
        match value {
            CliContest::None => ContestMode::None,
            CliContest::NaVhf => ContestMode::NaVhf,
            CliContest::EuVhf => ContestMode::EuVhf,
            CliContest::FieldDay => ContestMode::FieldDay,
            CliContest::RttyRu => ContestMode::RttyRoundup,
            CliContest::WwDigi => ContestMode::WwDigi,
            CliContest::Fox => ContestMode::Fox,
            CliContest::Hound => ContestMode::Hound,
            CliContest::ArrlDigi => ContestMode::ArrlDigi,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Decode one or more 12 kHz mono/stereo WAV or headerless raw PCM files.
    Decode(DecodeArgs),
    /// Encode and GFSK-modulate an FT8 or FT4 message into WAV or raw PCM.
    Encode(EncodeArgs),
}

#[derive(Debug, Args)]
pub struct DecodeArgs {
    #[arg(long, short = 'm', value_enum, default_value = "ft8")]
    pub mode: CliMode,

    #[arg(long, short = 'd', default_value_t = 3)]
    pub depth: u8,

    #[arg(long = "freq-min", alias = "flow", default_value_t = 200.0)]
    pub freq_min_hz: f32,

    #[arg(long = "freq-max", alias = "fhigh", default_value_t = 4000.0)]
    pub freq_max_hz: f32,

    #[arg(long = "rx-freq", default_value_t = 1500.0)]
    pub rx_freq_hz: f32,

    #[arg(long = "freq-tol", default_value_t = 20.0)]
    pub freq_tol_hz: f32,

    #[arg(long, value_enum, default_value = "jt9")]
    pub format: OutputFormat,

    /// Enable Costas frequency drift and 8-bucket QSB envelope telemetry.
    #[arg(long)]
    pub telemetry: bool,

    /// Explicit 6-digit UTC timestamp (`HHMMSS`, e.g. `133430`). Defaults to filename stamp or `000000`.
    #[arg(long)]
    pub utc: Option<String>,

    /// Override Rayon RX worker threads (`None` uses auto heuristic).
    #[arg(long)]
    pub threads: Option<usize>,

    /// Treat files as headerless raw PCM (`i16le` or `f32le`) at 12 kHz mono.
    #[arg(long = "raw-pcm", value_enum)]
    pub raw_pcm: Option<CliRawFormat>,

    /// Mark input as an early live partial pass (bounds deep subtraction passes).
    #[arg(long = "early-pass")]
    pub early_pass: bool,

    /// Clear session callsign hash table between multiple input files.
    #[arg(long = "reset-between-files")]
    pub reset_between_files: bool,

    /// Suppress non-essential stderr diagnostics.
    #[arg(long, short = 'q')]
    pub quiet: bool,

    /// Local station callsign for AP passes & hash table exclusion.
    #[arg(long = "my-call")]
    pub my_call: Option<String>,

    /// Target DX callsign for AP passes (`a2`–`a6`).
    #[arg(long = "his-call")]
    pub his_call: Option<String>,

    /// Target DX 4-char grid locator (`a1`–`a6` context).
    #[arg(long = "his-grid")]
    pub his_grid: Option<String>,

    /// WSJT-X QSO progress state `0..=5`.
    #[arg(long = "qso-progress")]
    pub qso_progress: Option<u8>,

    /// Contest mode for AP hypothesis rules.
    #[arg(long, value_enum, default_value = "none")]
    pub contest: CliContest,

    /// Disable AP passes even when QSO context is given.
    #[arg(long = "no-ap")]
    pub no_ap: bool,

    /// Restrict AP passes strictly to CQ hypotheses (`a1` / FT4 CQ).
    #[arg(long = "ap-cq-only")]
    pub ap_cq_only: bool,

    /// Input 12 kHz `.wav` or raw PCM files (`-` reads raw stdin if `--raw-pcm` is set).
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct EncodeArgs {
    #[arg(long, short = 'm', value_enum)]
    pub mode: CliMode,

    #[arg(long = "af", alias = "af-hz", default_value_t = 1500.0)]
    pub af_hz: f32,

    #[arg(long = "sr", alias = "sample-rate", default_value_t = 12_000)]
    pub sample_rate: u32,

    #[arg(long = "start-delay-ms")]
    pub start_delay_ms: Option<u32>,

    #[arg(long = "offset-ms", default_value_t = 0)]
    pub offset_ms: u64,

    /// Zero-pad active modulated audio out to full 15.0s (FT8) or 7.5s (FT4) slot duration.
    #[arg(long = "full-slot")]
    pub full_slot: bool,

    /// Linear peak amplitude scaling factor in `(0.0, 1.0]`.
    #[arg(long, default_value_t = 1.0)]
    pub amplitude: f32,

    /// Output sample encoding (`i16le` or `f32le`). Defaults to `i16le` for `.wav` or `f32le` for `--raw-pcm`.
    #[arg(long = "sample-format", value_enum)]
    pub sample_format: Option<CliRawFormat>,

    /// Write headerless raw PCM instead of RIFF/WAV.
    #[arg(long = "raw-pcm")]
    pub raw_pcm: bool,

    /// Print JSON summary metadata (`sample_count`, `duration_ms`, etc.) to stdout after writing `--out`.
    #[arg(long = "json-meta")]
    pub json_meta: bool,

    #[arg(long, short = 'o')]
    pub out: PathBuf,

    pub message: String,
}
