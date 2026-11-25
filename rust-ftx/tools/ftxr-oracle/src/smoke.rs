// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Automated smoke verification of all 6 WSJT-X reference oracle binaries
//! (`jt9`, `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`).

use std::path::Path;

use ftxr_core::Mode;
use thiserror::Error;

use crate::jt9::{Jt9Error, Jt9Profile, Jt9RunConfig, Jt9Runner};
use crate::sim::{SimError, SimRunner};
use crate::wav::{self, WavError};

pub const ORACLE_BINARIES: [&str; 6] = [
    "jt9",
    "ft8code",
    "ft4code",
    "ft8sim",
    "ft4sim",
    "ft4sim_mult",
];

#[derive(Debug, Error)]
pub enum SmokeError {
    #[error("missing oracle binary: {0}")]
    MissingBinary(String),
    #[error("simulator error: {0}")]
    Sim(#[from] SimError),
    #[error("jt9 error: {0}")]
    Jt9(#[from] Jt9Error),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("smoke check failed: {0}")]
    Assertion(String),
}

pub fn run_smoke(
    wsjtx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
) -> Result<(), SmokeError> {
    // 1. Verify all 6 binaries exist and are executable.
    for name in ORACLE_BINARIES {
        let p = bin_dir.join(name);
        if !p.exists() {
            return Err(SmokeError::MissingBinary(p.display().to_string()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p)?.permissions().mode();
            if mode & 0o111 == 0 {
                return Err(SmokeError::Assertion(format!(
                    "{} is not executable",
                    p.display()
                )));
            }
        }
    }
    println!("[PASS] All 6 oracle binaries present in {}", bin_dir.display());

    let sim = SimRunner::new(bin_dir.to_path_buf(), cache_dir.to_path_buf());
    let jt9 = Jt9Runner::new(bin_dir.to_path_buf(), cache_dir.to_path_buf());

    // 2. ft8code -T and single message.
    let ft8_t = sim.run_ft8code(&["-T"])?;
    if !ft8_t.contains("Standard msg") || ft8_t.lines().count() < 40 {
        return Err(SmokeError::Assertion("ft8code -T output unexpectedly short".into()));
    }
    let ft8_single = sim.run_ft8code(&["CQ N0AAA EM10"])?;
    if !ft8_single.contains("CQ N0AAA EM10") || !ft8_single.contains("Channel symbols") {
        return Err(SmokeError::Assertion("ft8code single message output invalid".into()));
    }
    println!("[PASS] ft8code -T & single message");

    // 3. ft4code -t and single message.
    let ft4_t = sim.run_ft4code(&["-t"])?;
    if !ft4_t.contains("Standard msg") || ft4_t.lines().count() < 40 {
        return Err(SmokeError::Assertion("ft4code -t output unexpectedly short".into()));
    }
    let ft4_single = sim.run_ft4code(&["CQ N0AAA EM10"])?;
    if !ft4_single.contains("CQ N0AAA EM10") || !ft4_single.contains("Channel symbols") {
        return Err(SmokeError::Assertion("ft4code single message output invalid".into()));
    }
    println!("[PASS] ft4code -t & single message");

    // 4. ft8sim -> jt9 -8 round-trip.
    {
        let tmp = tempfile::tempdir()?;
        let wav_path = tmp.path().join("000000_000001.wav");
        let (samples, _) = sim.run_ft8sim("CQ N0AAA EM10", 1500.0, 0.0, 0.0, 0.0, -10.0)?;
        wav::write_wav_12k_mono(&wav_path, &samples)?;
        let cfg = Jt9RunConfig::new(Mode::Ft8, 1, Jt9Profile::BlindNoAp, 0);
        let res = jt9.run_wav(&wav_path, &cfg, None)?;
        let matched = res.decodes.iter().find(|d| {
            d.message == "CQ N0AAA EM10"
                && (d.af_hz - 1500.0).abs() <= 2.0
                && d.dt_sec.abs() <= 0.15
                && !d.ap_tag.is_ap()
        });
        if matched.is_none() {
            return Err(SmokeError::Assertion(format!(
                "ft8sim -> jt9 -8 did not recover expected decode; got {:?}",
                res.decodes
            )));
        }
        println!("[PASS] ft8sim -> jt9 -8 round-trip ({:.1} ms)", res.elapsed_ms);
    }

    // 5. ft4sim -> jt9 -5 round-trip.
    {
        let tmp = tempfile::tempdir()?;
        let wav_path = tmp.path().join("000000_000001.wav");
        let (samples, _) = sim.run_ft4sim("CQ N0AAA EM10", 1500.0, 0.0, 0.0, 0.0, -10.0)?;
        wav::write_wav_12k_mono(&wav_path, &samples)?;
        let cfg = Jt9RunConfig::new(Mode::Ft4, 1, Jt9Profile::BlindNoAp, 0);
        let res = jt9.run_wav(&wav_path, &cfg, None)?;
        let matched = res.decodes.iter().find(|d| {
            d.message == "CQ N0AAA EM10"
                && (d.af_hz - 1500.0).abs() <= 5.0
                && d.dt_sec.abs() <= 0.15
                && !d.ap_tag.is_ap()
        });
        if matched.is_none() {
            return Err(SmokeError::Assertion(format!(
                "ft4sim -> jt9 -5 did not recover expected decode; got {:?}",
                res.decodes
            )));
        }
        println!("[PASS] ft4sim -> jt9 -5 round-trip ({:.1} ms)", res.elapsed_ms);
    }

    // 6. ft4sim_mult -> jt9 -5 round-trip.
    {
        let tmp = tempfile::tempdir()?;
        let wav_path = tmp.path().join("000000_000001.wav");
        let messages_txt = wsjtx_root.join("lib/ft4/messages.txt");
        let (samples, _) = sim.run_ft4sim_mult(&messages_txt, 20, 1)?;
        wav::write_wav_12k_mono(&wav_path, &samples)?;
        let cfg = Jt9RunConfig::new(Mode::Ft4, 3, Jt9Profile::BlindNoAp, 0);
        let res = jt9.run_wav(&wav_path, &cfg, None)?;
        if res.decodes.len() < 10 || !res.decodes.iter().all(|d| d.mode == Mode::Ft4) {
            return Err(SmokeError::Assertion(format!(
                "ft4sim_mult -> jt9 -5 recovered only {} decodes",
                res.decodes.len()
            )));
        }
        println!(
            "[PASS] ft4sim_mult -> jt9 -5 ({} decodes, {:.1} ms)",
            res.decodes.len(),
            res.elapsed_ms
        );
    }

    // 7. Real WSJT-X FT8 & FT4 samples.
    {
        let ft8_wav = wsjtx_root.join("samples/FT8/210703_133430.wav");
        let cfg8 = Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::BlindNoAp, 0);
        let res8 = jt9.run_wav(&ft8_wav, &cfg8, None)?;
        if res8.decodes.len() < 20 {
            return Err(SmokeError::Assertion(format!(
                "210703_133430.wav expected >=20 FT8 decodes, got {}",
                res8.decodes.len()
            )));
        }
        println!(
            "[PASS] jt9 -8 on samples/FT8/210703_133430.wav ({} decodes, {:.1} ms)",
            res8.decodes.len(),
            res8.elapsed_ms
        );

        let ft4_wav = wsjtx_root.join("samples/FT4/000000_000002.wav");
        let cfg4 = Jt9RunConfig::new(Mode::Ft4, 3, Jt9Profile::BlindNoAp, 0);
        let res4 = jt9.run_wav(&ft4_wav, &cfg4, None)?;
        if res4.decodes.len() < 10 {
            return Err(SmokeError::Assertion(format!(
                "000000_000002.wav expected >=10 FT4 decodes, got {}",
                res4.decodes.len()
            )));
        }
        println!(
            "[PASS] jt9 -5 on samples/FT4/000000_000002.wav ({} decodes, {:.1} ms)",
            res4.decodes.len(),
            res4.elapsed_ms
        );
    }

    println!("All Stage 0 oracle smoke checks passed.");
    Ok(())
}
