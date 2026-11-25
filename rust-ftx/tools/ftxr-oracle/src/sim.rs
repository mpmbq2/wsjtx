// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! TempDir-isolated wrappers for WSJT-X reference utilities (`ft8code`, `ft4code`, `ft8sim`,
//! `ft4sim`, `ft4sim_mult`) and cached unit-power clean template extraction.

use std::f64::consts::FRAC_1_SQRT_2;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ftxr_core::{ft4, ft8, Mode};
use thiserror::Error;

use crate::wav::{self, WavError};

/// Peak scaling constant used by `ft8sim.f90:159` and `ft4sim.f90:110` for noiseless output (`snrdb > 90`).
const SIM_CLEAN_PEAK: f64 = 32_766.9;

#[derive(Debug, Error)]
pub enum SimError {
    #[error("oracle binary not found at {0}")]
    BinaryMissing(PathBuf),
    #[error("failed to execute {bin}: {source}")]
    Spawn {
        bin: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{bin} exited with status {status:?}; stderr: {stderr}")]
    NonZeroExit {
        bin: String,
        status: Option<i32>,
        stderr: String,
    },
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("clean template RMS calibration failed for {mode:?} ({msg}): rms={rms:.6} (expected ~{FRAC_1_SQRT_2:.6})")]
    BadTemplateRms {
        mode: Mode,
        msg: String,
        rms: f64,
    },
}

#[derive(Debug, Clone)]
pub struct SimRunner {
    bin_dir: PathBuf,
    cache_dir: PathBuf,
}

impl SimRunner {
    pub fn new(bin_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self { bin_dir, cache_dir }
    }

    pub fn bin_path(&self, name: &str) -> PathBuf {
        self.bin_dir.join(name)
    }

    pub fn run_ft8code(&self, args: &[&str]) -> Result<String, SimError> {
        self.run_code_tool("ft8code", args)
    }

    pub fn run_ft4code(&self, args: &[&str]) -> Result<String, SimError> {
        self.run_code_tool("ft4code", args)
    }

    fn run_code_tool(&self, name: &str, args: &[&str]) -> Result<String, SimError> {
        let bin = self.bin_path(name);
        if !bin.exists() {
            return Err(SimError::BinaryMissing(bin));
        }
        let output = Command::new(&bin)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|source| SimError::Spawn {
                bin: name.to_string(),
                source,
            })?;
        if !output.status.success() {
            return Err(SimError::NonZeroExit {
                bin: name.to_string(),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Runs `ft8sim` inside an isolated `TempDir` and returns the generated `000000_000001.wav` samples.
    #[allow(clippy::too_many_arguments)]
    pub fn run_ft8sim(
        &self,
        message: &str,
        af_hz: f64,
        dt_sec: f64,
        fspread_hz: f64,
        delay_ms: f64,
        snr_db: f64,
    ) -> Result<(Vec<i16>, String), SimError> {
        let bin = self.bin_path("ft8sim");
        if !bin.exists() {
            return Err(SimError::BinaryMissing(bin));
        }
        let tmp = tempfile::tempdir()?;
        let output = Command::new(&bin)
            .current_dir(tmp.path())
            .args([
                message,
                &format!("{af_hz:.3}"),
                &format!("{dt_sec:.3}"),
                &format!("{fspread_hz:.3}"),
                &format!("{delay_ms:.3}"),
                "1",
                &format!("{snr_db:.2}"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|source| SimError::Spawn {
                bin: "ft8sim".to_string(),
                source,
            })?;
        if !output.status.success() {
            return Err(SimError::NonZeroExit {
                bin: "ft8sim".to_string(),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let samples = wav::read_wav_12k_mono(&tmp.path().join("000000_000001.wav"))?;
        Ok((samples, String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    /// Runs `ft4sim` inside an isolated `TempDir` and returns the generated `000000_000001.wav` samples.
    #[allow(clippy::too_many_arguments)]
    pub fn run_ft4sim(
        &self,
        message: &str,
        af_hz: f64,
        dt_sec: f64,
        fspread_hz: f64,
        delay_ms: f64,
        snr_db: f64,
    ) -> Result<(Vec<i16>, String), SimError> {
        let bin = self.bin_path("ft4sim");
        if !bin.exists() {
            return Err(SimError::BinaryMissing(bin));
        }
        let tmp = tempfile::tempdir()?;
        let output = Command::new(&bin)
            .current_dir(tmp.path())
            .args([
                message,
                &format!("{af_hz:.3}"),
                &format!("{dt_sec:.3}"),
                &format!("{fspread_hz:.3}"),
                &format!("{delay_ms:.3}"),
                "1",
                &format!("{snr_db:.2}"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|source| SimError::Spawn {
                bin: "ft4sim".to_string(),
                source,
            })?;
        if !output.status.success() {
            return Err(SimError::NonZeroExit {
                bin: "ft4sim".to_string(),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let samples = wav::read_wav_12k_mono(&tmp.path().join("000000_000001.wav"))?;
        Ok((samples, String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    /// Runs `ft4sim_mult` inside an isolated `TempDir` with `messages.txt` copied from `wsjtx`.
    pub fn run_ft4sim_mult(
        &self,
        messages_txt: &Path,
        nsigs: u32,
        nfiles: u32,
    ) -> Result<(Vec<i16>, String), SimError> {
        let bin = self.bin_path("ft4sim_mult");
        if !bin.exists() {
            return Err(SimError::BinaryMissing(bin));
        }
        let tmp = tempfile::tempdir()?;
        fs::copy(messages_txt, tmp.path().join("messages.txt"))?;
        let output = Command::new(&bin)
            .current_dir(tmp.path())
            .args([nsigs.to_string(), nfiles.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|source| SimError::Spawn {
                bin: "ft4sim_mult".to_string(),
                source,
            })?;
        if !output.status.success() {
            return Err(SimError::NonZeroExit {
                bin: "ft4sim_mult".to_string(),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let samples = wav::read_wav_12k_mono(&tmp.path().join("000000_000001.wav"))?;
        Ok((samples, String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    /// Extracts a 100% deterministic, noiseless (`snrdb=99`), `DT=0.0`, unit-amplitude
    /// (`P_active = 0.5`, `RMS_active = 1/sqrt(2)`) signal template from `ft8sim`/`ft4sim`,
    /// cached under `.oracle-cache/templates/`.
    pub fn clean_template(&self, mode: Mode, message: &str, af_hz: f64) -> Result<Vec<f64>, SimError> {
        let af_millihz = (af_hz * 1000.0).round() as i64;
        let mode_str = match mode {
            Mode::Ft8 => "ft8",
            Mode::Ft4 => "ft4",
        };
        let key = wav::sha256_bytes(format!("tpl-v1|{mode_str}|{message}|{af_millihz}").as_bytes());
        let cache_dir = self.cache_dir.join("templates");
        let cache_path = cache_dir.join(format!("{key}.f64"));

        let expected_len = mode.rx_window_samples();
        if cache_path.exists() {
            let bytes = fs::read(&cache_path)?;
            if bytes.len() == expected_len * 8 {
                let mut out = Vec::with_capacity(expected_len);
                for chunk in bytes.chunks_exact(8) {
                    out.push(f64::from_le_bytes(chunk.try_into().unwrap()));
                }
                return Ok(out);
            }
        }

        let (raw_i16, _) = match mode {
            Mode::Ft8 => self.run_ft8sim(message, af_hz, 0.0, 0.0, 0.0, 99.0)?,
            Mode::Ft4 => self.run_ft4sim(message, af_hz, 0.0, 0.0, 0.0, 99.0)?,
        };

        let mut wave: Vec<f64> = raw_i16.iter().map(|&s| f64::from(s) / SIM_CLEAN_PEAK).collect();
        let (active_start, active_end) = active_non_ramp_bounds(mode);
        let active_slice = &wave[active_start..active_end];
        let rms = (active_slice.iter().map(|&x| x * x).sum::<f64>() / active_slice.len() as f64)
            .sqrt();

        if (rms - FRAC_1_SQRT_2).abs() > 0.01 {
            return Err(SimError::BadTemplateRms {
                mode,
                msg: message.to_string(),
                rms,
            });
        }

        let rescale = FRAC_1_SQRT_2 / rms;
        for sample in &mut wave {
            *sample *= rescale;
        }

        fs::create_dir_all(&cache_dir)?;
        let mut bytes = Vec::with_capacity(wave.len() * 8);
        for &v in &wave {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let tmp_path = cache_dir.join(format!("{key}.f64.tmp"));
        fs::write(&tmp_path, &bytes)?;
        fs::rename(&tmp_path, &cache_path)?;

        Ok(wave)
    }
}

/// Exact non-ramp active sample interval at `DT = 0.0` matching `ft8sim.f90` and `ft4sim.f90`.
pub const fn active_non_ramp_bounds(mode: Mode) -> (usize, usize) {
    match mode {
        // FT8: sync starts at 6000 (0.500 s), ramp is 240 samples (20 ms) on each end.
        Mode::Ft8 => (6_000 + 240, 6_000 + ft8::params::NZ - 240),
        // FT4: ramp symbol 0 is 5424..6000, sync+data is 6000..65328, trailing ramp is 65328..65904.
        Mode::Ft4 => (6_000, 6_000 + ft4::params::NZ),
    }
}

/// Exact nominal active waveform start sample index at `DT = 0.0` in `ft8sim`/`ft4sim`.
pub const fn active_start_sample(mode: Mode) -> usize {
    match mode {
        Mode::Ft8 => 6_000,
        Mode::Ft4 => 6_000 - ft4::params::NSPS, // 5424 (start of leading ramp symbol)
    }
}
