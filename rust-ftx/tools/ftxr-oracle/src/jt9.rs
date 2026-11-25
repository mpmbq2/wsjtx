// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Classic single-threaded `jt9` (`-8` FT8 / `-5` FT4) subprocess runner, FFTW wisdom manager,
//! standardized CLI profiles, and strict stdout/AP parser.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::ValueEnum;
use ftxr_core::{ApTag, Mode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::wav;

#[derive(Debug, Error)]
pub enum Jt9Error {
    #[error("jt9 binary not found at {0}")]
    BinaryMissing(PathBuf),
    #[error("failed to execute jt9: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("jt9 timed out after {timeout_secs}s on {wav}")]
    Timeout { wav: String, timeout_secs: u64 },
    #[error("jt9 exited with status {status:?} on {wav}; stderr: {stderr}")]
    NonZeroExit {
        wav: String,
        status: Option<i32>,
        stderr: String,
    },
    #[error("jt9 output missing <DecodeFinished> marker on {wav}")]
    MissingDecodeFinished { wav: String },
    #[error("I/O error in jt9 runner: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid jt9 configuration: {0}")]
    InvalidConfig(String),
}

/// Oracle-side AP tag recognizing all WSJT-X `jt9` annotations (`a1`..`a8`),
/// including deferred cross-slot `a7` and RX-freq list `a8` for apples-to-apples filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OracleApTag {
    #[default]
    None,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
    A7,
    A8,
}

impl OracleApTag {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::A1 => "a1",
            Self::A2 => "a2",
            Self::A3 => "a3",
            Self::A4 => "a4",
            Self::A5 => "a5",
            Self::A6 => "a6",
            Self::A7 => "a7",
            Self::A8 => "a8",
        }
    }

    pub fn from_token(tok: &str) -> Option<Self> {
        match tok {
            "a1" => Some(Self::A1),
            "a2" => Some(Self::A2),
            "a3" => Some(Self::A3),
            "a4" => Some(Self::A4),
            "a5" => Some(Self::A5),
            "a6" => Some(Self::A6),
            "a7" => Some(Self::A7),
            "a8" => Some(Self::A8),
            _ => None,
        }
    }

    pub const fn is_ap(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn is_deferred_a7_a8(self) -> bool {
        matches!(self, Self::A7 | Self::A8)
    }

    /// Maps single-slot tags `a1`..`a6` into `ftxr-core`'s canonical [`ApTag`].
    #[allow(dead_code)]
    pub const fn to_core_ap_tag(self, uncertain: bool) -> ApTag {
        match self {
            Self::A1 => ApTag::Ap { kind: 1, uncertain },
            Self::A2 => ApTag::Ap { kind: 2, uncertain },
            Self::A3 => ApTag::Ap { kind: 3, uncertain },
            Self::A4 => ApTag::Ap { kind: 4, uncertain },
            Self::A5 => ApTag::Ap { kind: 5, uncertain },
            Self::A6 => ApTag::Ap { kind: 6, uncertain },
            Self::None | Self::A7 | Self::A8 => ApTag::None,
        }
    }
}

/// Standardized `jt9` AP/callsign invocation profiles eliminating `K1ABC`/`W9XYZ`/`EN37` defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::enum_variant_names)]
pub enum Jt9Profile {
    /// `-c b -x b -Q 1`: blanks calls/grid and selects `QSOProgress=1` so `a1`..`a8` are all disabled.
    BlindNoAp,
    /// `-c b -x b -Q 0`: blanks calls/grid, leaves `a1` (CQ AP) enabled (idle listening).
    BlindCqAp,
    /// `-c <mycall> -x <hiscall> -g . -Q <prog> -f <rx_freq>`: single-slot `a1`..`a6` (`-g .` suppresses `a8`).
    QsoAp,
}

impl Jt9Profile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BlindNoAp => "blind-no-ap",
            Self::BlindCqAp => "blind-cq-ap",
            Self::QsoAp => "qso-ap",
        }
    }
}

impl std::fmt::Display for Jt9Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Complete CLI invocation parameters for one isolated `jt9` subprocess run.
#[derive(Debug, Clone)]
pub struct Jt9RunConfig {
    pub mode: Mode,
    pub depth: u8,
    pub profile: Jt9Profile,
    pub patience: u8,
    pub freq_low_hz: u16,
    pub freq_high_hz: u16,
    pub rx_freq_hz: u16,
    pub freq_tol_hz: u16,
    pub my_call: Option<String>,
    pub his_call: Option<String>,
    pub his_grid: Option<String>,
    pub qso_progress: Option<u8>,
    pub nexp_decode: Option<u8>,
    pub allow_a8: bool,
    pub timeout_secs: u64,
}

impl Jt9RunConfig {
    pub fn new(mode: Mode, depth: u8, profile: Jt9Profile, patience: u8) -> Self {
        Self {
            mode,
            depth,
            profile,
            patience,
            freq_low_hz: 200,
            freq_high_hz: 4000,
            rx_freq_hz: 1500,
            freq_tol_hz: 20,
            my_call: None,
            his_call: None,
            his_grid: None,
            qso_progress: None,
            nexp_decode: None,
            allow_a8: false,
            timeout_secs: 60,
        }
    }

    /// Builds the `jt9` argument vector excluding `-a`, `-t`, and the WAV path.
    pub fn base_args(&self) -> Result<Vec<String>, Jt9Error> {
        if !(1..=3).contains(&self.depth) {
            return Err(Jt9Error::InvalidConfig(format!(
                "depth must be 1..=3, got {}",
                self.depth
            )));
        }
        if self.patience > 4 {
            return Err(Jt9Error::InvalidConfig(format!(
                "patience must be 0..=4, got {}",
                self.patience
            )));
        }

        let mut args = vec![
            match self.mode {
                Mode::Ft8 => "-8".to_string(),
                Mode::Ft4 => "-5".to_string(),
            },
            "-d".to_string(),
            self.depth.to_string(),
            "-w".to_string(),
            self.patience.to_string(),
            "-m".to_string(),
            "1".to_string(),
            "-L".to_string(),
            self.freq_low_hz.to_string(),
            "-H".to_string(),
            self.freq_high_hz.to_string(),
            "-f".to_string(),
            self.rx_freq_hz.to_string(),
            "-F".to_string(),
            self.freq_tol_hz.to_string(),
        ];

        if let Some(exp) = self.nexp_decode {
            args.push("-X".to_string());
            args.push(exp.to_string());
        }

        match self.profile {
            Jt9Profile::BlindNoAp => {
                args.extend([
                    "-c".into(),
                    "b".into(),
                    "-x".into(),
                    "b".into(),
                    "-Q".into(),
                    "1".into(),
                ]);
            }
            Jt9Profile::BlindCqAp => {
                args.extend([
                    "-c".into(),
                    "b".into(),
                    "-x".into(),
                    "b".into(),
                    "-Q".into(),
                    "0".into(),
                ]);
            }
            Jt9Profile::QsoAp => {
                let quote_f90 = |s: &str| -> String {
                    if s.contains('/') || s.contains(' ') || s.contains(',') || s.contains('\'') {
                        format!("'{}'", s.replace('\'', "''"))
                    } else {
                        s.to_string()
                    }
                };
                let my = quote_f90(self.my_call.as_deref().unwrap_or("N0AAA"));
                let his = quote_f90(self.his_call.as_deref().unwrap_or("K9BBB"));
                let grid = quote_f90(if self.allow_a8 {
                    self.his_grid.as_deref().unwrap_or("DM79")
                } else {
                    // Length < 4 suppresses `ft8_a8d` while preserving `a1`..`a6`.
                    "."
                });
                let prog = self.qso_progress.unwrap_or(1);
                args.extend([
                    "-c".into(),
                    my,
                    "-x".into(),
                    his,
                    "-g".into(),
                    grid,
                    "-Q".into(),
                    prog.to_string(),
                ]);
            }
        }

        Ok(args)
    }

    /// Formats a human-readable CLI template string for scorecard metadata.
    pub fn cli_template(&self) -> String {
        let base = self.base_args().unwrap_or_default().join(" ");
        format!("jt9 {base} ...")
    }
}

/// One parsed decode line from `jt9` stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Jt9Decode {
    pub utc: String,
    pub mode: Mode,
    pub snr_db: i8,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub message: String,
    pub ap_tag: OracleApTag,
    pub ap_uncertain: bool,
}

impl Jt9Decode {
    /// Formats WSJT-X / `digi-dx` compatible metadata (`None`, `"a1"`, `"? a1"`, ...).
    #[allow(dead_code)]
    pub fn wsjtx_metadata(&self) -> Option<String> {
        if !self.ap_tag.is_ap() {
            return None;
        }
        let tag = self.ap_tag.as_str();
        if self.ap_uncertain {
            Some(format!("? {tag}"))
        } else {
            Some(tag.to_string())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeFinished {
    pub nsynced: i32,
    pub ndecoded: i32,
    pub navg0: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedJt9Output {
    pub decodes: Vec<Jt9Decode>,
    pub finished: Option<DecodeFinished>,
    pub eof_noticed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jt9RunOutput {
    pub wav_path: String,
    pub cli_args: Vec<String>,
    pub elapsed_ms: f64,
    pub decodes: Vec<Jt9Decode>,
    pub finished: DecodeFinished,
    pub eof_noticed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedLine {
    Decode(Jt9Decode),
    Finished(DecodeFinished),
    EofNotice,
    Ignored,
}

/// Parses a single line of `jt9` stdout.
///
/// Strictly parses columns 1–5 (`utc`, `snr`, `dt`, `af`, `~`|`+`) so `+` in FT4 signal reports
/// (`R+05`) never confuses the mode delimiter, and strips trailing `?` only when accompanied by a
/// valid `a1`..`a8` AP tag so free-text `?` (e.g., `"PSE QSL?"`) is preserved intact.
pub fn parse_jt9_line(line: &str) -> ParsedLine {
    let trimmed_end = line.trim_end_matches(['\r', '\n']);
    let trimmed = trimmed_end.trim();
    if trimmed.is_empty() {
        return ParsedLine::Ignored;
    }
    if trimmed.starts_with("EOF on input file") {
        return ParsedLine::EofNotice;
    }
    if let Some(rest) = trimmed.strip_prefix("<DecodeFinished>") {
        let nums: Vec<i32> = rest
            .split_whitespace()
            .filter_map(|t| t.parse::<i32>().ok())
            .collect();
        if nums.len() >= 3 {
            return ParsedLine::Finished(DecodeFinished {
                nsynced: nums[0],
                ndecoded: nums[1],
                navg0: nums[2],
            });
        }
        return ParsedLine::Finished(DecodeFinished {
            nsynced: 0,
            ndecoded: 0,
            navg0: 0,
        });
    }

    // Extract the first 5 whitespace-delimited tokens along with byte span after token 5.
    let bytes = trimmed_end.as_bytes();
    let mut i = 0usize;
    let mut tokens: [(&str, usize); 5] = [("", 0); 5];
    for slot in &mut tokens {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            return ParsedLine::Ignored;
        }
        *slot = (&trimmed_end[start..i], i);
    }

    let utc = tokens[0].0;
    if !(utc.len() == 4 || utc.len() == 6) || !utc.bytes().all(|b| b.is_ascii_digit()) {
        return ParsedLine::Ignored;
    }
    let Ok(snr_db) = tokens[1].0.parse::<i8>() else {
        return ParsedLine::Ignored;
    };
    let Ok(mut dt_sec) = tokens[2].0.parse::<f32>() else {
        return ParsedLine::Ignored;
    };
    if dt_sec == 0.0 {
        dt_sec = 0.0; // Normalize Fortran `-0.0` to `+0.0`.
    }
    let Ok(af_hz) = tokens[3].0.parse::<f32>() else {
        return ParsedLine::Ignored;
    };
    let mode = match tokens[4].0 {
        "~" => Mode::Ft8,
        "+" => Mode::Ft4,
        _ => return ParsedLine::Ignored,
    };

    let rest = trimmed_end[tokens[4].1..].trim();
    if rest.is_empty() {
        return ParsedLine::Ignored;
    }

    let (payload, ap_tag, ap_uncertain) = split_message_and_ap(rest);
    let normalized_msg = payload.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized_msg.is_empty() {
        return ParsedLine::Ignored;
    }

    ParsedLine::Decode(Jt9Decode {
        utc: utc.to_string(),
        mode,
        snr_db,
        dt_sec,
        af_hz,
        message: normalized_msg,
        ap_tag,
        ap_uncertain,
    })
}

fn split_message_and_ap(rest: &str) -> (&str, OracleApTag, bool) {
    let trimmed = rest.trim();
    if let Some((prefix, last_tok)) = trimmed.rsplit_once(char::is_whitespace) {
        if let Some(tag) = OracleApTag::from_token(last_tok) {
            let before_tag = prefix.trim_end();
            // AP hypotheses only decode structured callsign messages (never free text ending in '?'),
            // so a trailing '?' paired with `a1`..`a8` is always WSJT-X's `qual < 0.17` uncertainty marker.
            if let Some(stripped) = before_tag.strip_suffix('?') {
                return (stripped.trim_end(), tag, true);
            }
            return (before_tag, tag, false);
        }
    }
    (trimmed, OracleApTag::None, false)
}

/// Parses complete `jt9` stdout text.
pub fn parse_jt9_stdout(stdout: &str) -> ParsedJt9Output {
    let mut decodes = Vec::new();
    let mut finished = None;
    let mut eof_noticed = false;

    for line in stdout.lines() {
        match parse_jt9_line(line) {
            ParsedLine::Decode(d) => decodes.push(d),
            ParsedLine::Finished(f) => finished = Some(f),
            ParsedLine::EofNotice => eof_noticed = true,
            ParsedLine::Ignored => {}
        }
    }

    ParsedJt9Output {
        decodes,
        finished,
        eof_noticed,
    }
}

/// Tallies AP tag counts (`none`, `a1`..`a8`) in stable JSON key order.
pub fn count_ap_tags(decodes: &[Jt9Decode]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for key in ["none", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8"] {
        counts.insert(key.to_string(), 0);
    }
    for d in decodes {
        *counts.entry(d.ap_tag.as_str().to_string()).or_insert(0) += 1;
    }
    counts
}

/// Process-isolated, concurrency-safe `jt9` subprocess runner.
#[derive(Debug, Clone)]
pub struct Jt9Runner {
    bin_dir: PathBuf,
    cache_dir: PathBuf,
}

impl Jt9Runner {
    pub fn new(bin_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self { bin_dir, cache_dir }
    }

    pub fn jt9_path(&self) -> PathBuf {
        self.bin_dir.join("jt9")
    }

    /// Pre-warms FFTW wisdom once per patience level (for both FT8 and FT4 plans) into
    /// `<cache_dir>/wisdom/jt9_wisdom_w<patience>.dat` so timed runs incur zero wisdom stall.
    pub fn ensure_warm_wisdom(
        &self,
        patience: u8,
        ft8_warmup_wav: &Path,
        ft4_warmup_wav: &Path,
    ) -> Result<Option<PathBuf>, Jt9Error> {
        if patience == 0 {
            return Ok(None);
        }

        let wisdom_dir = self.cache_dir.join("wisdom");
        fs::create_dir_all(&wisdom_dir)?;
        let wisdom_file = wisdom_dir.join(format!("jt9_wisdom_w{patience}.dat"));
        if wisdom_file.exists() && fs::metadata(&wisdom_file)?.len() > 0 {
            return Ok(Some(wisdom_file));
        }

        let jt9_bin = self.jt9_path();
        if !jt9_bin.exists() {
            return Err(Jt9Error::BinaryMissing(jt9_bin));
        }

        let tmp = tempfile::tempdir()?;
        let data_dir = tmp.path().join("data");
        let temp_dir = tmp.path().join("temp");
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(&temp_dir)?;

        for (mode_flag, wav_path) in [("-8", ft8_warmup_wav), ("-5", ft4_warmup_wav)] {
            let output = Command::new(&jt9_bin)
                .env("OMP_NUM_THREADS", "1")
                .args([
                    mode_flag,
                    "-d",
                    "3",
                    "-w",
                    &patience.to_string(),
                    "-m",
                    "1",
                    "-L",
                    "200",
                    "-H",
                    "4000",
                    "-c",
                    "b",
                    "-x",
                    "b",
                    "-Q",
                    "1",
                    "-a",
                ])
                .arg(&data_dir)
                .arg("-t")
                .arg(&temp_dir)
                .arg(wav_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map_err(Jt9Error::Spawn)?;

            if !output.status.success() {
                return Err(Jt9Error::NonZeroExit {
                    wav: wav_path.display().to_string(),
                    status: output.status.code(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
        }

        let produced = data_dir.join("jt9_wisdom.dat");
        if produced.exists() {
            let tmp_dest = wisdom_dir.join(format!("jt9_wisdom_w{patience}.dat.tmp"));
            fs::copy(&produced, &tmp_dest)?;
            fs::rename(&tmp_dest, &wisdom_file)?;
            Ok(Some(wisdom_file))
        } else {
            Ok(None)
        }
    }

    /// Runs a single isolated `jt9` subprocess on one WAV file.
    pub fn run_wav(
        &self,
        wav_path: &Path,
        cfg: &Jt9RunConfig,
        wisdom_path: Option<&Path>,
    ) -> Result<Jt9RunOutput, Jt9Error> {
        self.run_wavs(&[wav_path], cfg, wisdom_path)
    }

    /// Runs a single isolated `jt9` subprocess sequentially over multiple WAV files
    /// (preserving in-process hash-table state across files).
    pub fn run_wavs(
        &self,
        wav_paths: &[&Path],
        cfg: &Jt9RunConfig,
        wisdom_path: Option<&Path>,
    ) -> Result<Jt9RunOutput, Jt9Error> {
        let jt9_bin = self.jt9_path();
        if !jt9_bin.exists() {
            return Err(Jt9Error::BinaryMissing(jt9_bin));
        }
        if wav_paths.is_empty() {
            return Err(Jt9Error::InvalidConfig("empty wav_paths".into()));
        }

        let tmp = tempfile::tempdir()?;
        let data_dir = tmp.path().join("data");
        let temp_dir = tmp.path().join("temp");
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(&temp_dir)?;

        if let Some(w) = wisdom_path {
            if w.exists() {
                fs::copy(w, data_dir.join("jt9_wisdom.dat"))?;
            }
        }

        let mut args = cfg.base_args()?;
        args.push("-a".to_string());
        args.push(data_dir.display().to_string());
        args.push("-t".to_string());
        args.push(temp_dir.display().to_string());

        for (idx, wav_path) in wav_paths.iter().enumerate() {
            // Guard against Release `-fbounds-check` crashes on short filenames (<6 digits before `.wav`).
            let effective_wav = if wav::is_jt9_safe_filename(wav_path) {
                wav_path.to_path_buf()
            } else {
                let safe_link = tmp.path().join(format!("000000_{:06}.wav", idx + 1));
                let abs = fs::canonicalize(wav_path)?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&abs, &safe_link)?;
                #[cfg(not(unix))]
                fs::copy(&abs, &safe_link)?;
                safe_link
            };
            args.push(effective_wav.display().to_string());
        }

        let wav_label = wav_paths[0].display().to_string();
        let started = Instant::now();
        let mut child = Command::new(&jt9_bin)
            .env("OMP_NUM_THREADS", "1")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Jt9Error::Spawn)?;

        let deadline = Duration::from_secs(cfg.timeout_secs.max(1));
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if started.elapsed() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Jt9Error::Timeout {
                            wav: wav_label,
                            timeout_secs: cfg.timeout_secs,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(Jt9Error::Spawn(e)),
            }
        }

        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let output = child.wait_with_output().map_err(Jt9Error::Spawn)?;

        if !output.status.success() {
            return Err(Jt9Error::NonZeroExit {
                wav: wav_label,
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = parse_jt9_stdout(&stdout);
        let finished = parsed.finished.ok_or_else(|| Jt9Error::MissingDecodeFinished {
            wav: wav_label.clone(),
        })?;

        Ok(Jt9RunOutput {
            wav_path: wav_label,
            cli_args: args,
            elapsed_ms,
            decodes: parsed.decodes,
            finished,
            eof_noticed: parsed.eof_noticed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_golden_jt9_line_cases() {
        // 1. FT8 verified
        let l1 = "133430  -4  0.7  683 ~  CQ TA6CQ KN70                         ";
        match parse_jt9_line(l1) {
            ParsedLine::Decode(d) => {
                assert_eq!(d.utc, "133430");
                assert_eq!(d.mode, Mode::Ft8);
                assert_eq!(d.snr_db, -4);
                assert!((d.dt_sec - 0.7).abs() < 1e-5);
                assert!((d.af_hz - 683.0).abs() < 1e-5);
                assert_eq!(d.message, "CQ TA6CQ KN70");
                assert_eq!(d.ap_tag, OracleApTag::None);
                assert!(!d.ap_uncertain);
                assert_eq!(d.wsjtx_metadata(), None);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 2. FT8 certain AP
        let l2 = "110200 -14  0.6 1030 ~  CQ LZ1JZ KN22                         a1";
        match parse_jt9_line(l2) {
            ParsedLine::Decode(d) => {
                assert_eq!(d.message, "CQ LZ1JZ KN22");
                assert_eq!(d.ap_tag, OracleApTag::A1);
                assert!(!d.ap_uncertain);
                assert_eq!(d.wsjtx_metadata().as_deref(), Some("a1"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 3. FT8 uncertain AP
        let l3 = "110115 -22  0.3 1993 ~  CQ PY7XC HI21                       ? a1";
        match parse_jt9_line(l3) {
            ParsedLine::Decode(d) => {
                assert_eq!(d.message, "CQ PY7XC HI21");
                assert_eq!(d.ap_tag, OracleApTag::A1);
                assert!(d.ap_uncertain);
                assert_eq!(d.wsjtx_metadata().as_deref(), Some("? a1"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 4. FT8 deferred a7/a8
        let l4 = "110115 -24  0.2 1500 ~  N0AAA K9BBB DM79                    ? a8";
        match parse_jt9_line(l4) {
            ParsedLine::Decode(d) => {
                assert_eq!(d.message, "N0AAA K9BBB DM79");
                assert_eq!(d.ap_tag, OracleApTag::A8);
                assert!(d.ap_tag.is_deferred_a7_a8());
                assert!(d.ap_uncertain);
                assert_eq!(d.wsjtx_metadata().as_deref(), Some("? a8"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 5. FT8 free text with literal '?'
        let l5 = "110115  -5  0.1 1500 ~  PSE QSL?                              ";
        match parse_jt9_line(l5) {
            ParsedLine::Decode(d) => {
                assert_eq!(d.message, "PSE QSL?");
                assert_eq!(d.ap_tag, OracleApTag::None);
                assert!(!d.ap_uncertain);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 6. FT4 verified with '+' in report (signed +3 and unsigned 3)
        for l6 in [
            "000002  +3  0.1 1240 +  K1ABC W9XYZ R+05                      ",
            "000002   3  0.1 1240 +  K1ABC W9XYZ R+05                      ",
        ] {
            match parse_jt9_line(l6) {
                ParsedLine::Decode(d) => {
                    assert_eq!(d.mode, Mode::Ft4);
                    assert_eq!(d.snr_db, 3);
                    assert_eq!(d.message, "K1ABC W9XYZ R+05");
                    assert_eq!(d.ap_tag, OracleApTag::None);
                    assert!(!d.ap_uncertain);
                }
                other => panic!("unexpected: {other:?}"),
            }
        }

        // 7. FT4 uncertain AP
        let l7 = "000002 -18  0.2 1500 +  N0AAA K9BBB RR73                    ? a6";
        match parse_jt9_line(l7) {
            ParsedLine::Decode(d) => {
                assert_eq!(d.mode, Mode::Ft4);
                assert_eq!(d.message, "N0AAA K9BBB RR73");
                assert_eq!(d.ap_tag, OracleApTag::A6);
                assert!(d.ap_uncertain);
                assert_eq!(d.wsjtx_metadata().as_deref(), Some("? a6"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 8. Negative zero DT normalization
        let l8 = "000001 -11 -0.0 1500 +  CQ N0AAA EM10                           ";
        match parse_jt9_line(l8) {
            ParsedLine::Decode(d) => {
                assert!(d.dt_sec.is_sign_positive());
                assert_eq!(d.dt_sec, 0.0);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 9. Non-decode lines
        assert_eq!(
            parse_jt9_line("<DecodeFinished>   0   5        0"),
            ParsedLine::Finished(DecodeFinished {
                nsynced: 0,
                ndecoded: 5,
                navg0: 0
            })
        );
        assert_eq!(
            parse_jt9_line("EOF on input file 000000_000001.wav"),
            ParsedLine::EofNotice
        );
        assert_eq!(parse_jt9_line("   "), ParsedLine::Ignored);
    }

    #[test]
    fn profile_flags_enforce_blind_and_a8_hygiene() {
        let blind = Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::BlindNoAp, 1);
        let args = blind.base_args().unwrap();
        assert!(args.windows(2).any(|w| w == ["-c", "b"]));
        assert!(args.windows(2).any(|w| w == ["-x", "b"]));
        assert!(args.windows(2).any(|w| w == ["-Q", "1"]));
        assert!(!args.iter().any(|a| a == "-M"));

        let cq_ap = Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::BlindCqAp, 1);
        let args = cq_ap.base_args().unwrap();
        assert!(args.windows(2).any(|w| w == ["-Q", "0"]));

        let mut qso = Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::QsoAp, 1);
        qso.my_call = Some("N0AAA".into());
        qso.his_call = Some("K9BBB".into());
        qso.his_grid = Some("DM79".into());
        qso.qso_progress = Some(2);
        let args = qso.base_args().unwrap();
        assert!(args.windows(2).any(|w| w == ["-g", "."]));
        assert!(args.windows(2).any(|w| w == ["-Q", "2"]));

        qso.his_call = Some("KH1/KH7Z".into());
        let h_args = qso.base_args().unwrap();
        assert!(h_args.windows(2).any(|w| w == ["-x", "'KH1/KH7Z'"]));
    }
}
