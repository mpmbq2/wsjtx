// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Committed golden fixtures manifest and deterministic synthetic suite definitions.
//!
//! All synthetic callsigns use neutral identifiers (`N0AAA`, `K9BBB`, `AA1AA`, `BB2BB`,
//! `VE3CCC`, `DL4DDD`, etc.) — never `K1ABC`/`W9XYZ`/`EN37` — to prevent any accidental
//! overlap with `jt9.f90` defaults.

use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use ftxr_core::Mode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::sim::{SimError, SimRunner};
use crate::synth::{derive_seed, synthesize_slot, SignalSpec, SlotSpec};
use crate::wav::{self, WavError};

#[derive(Debug, Error)]
pub enum CorpusError {
    #[error("simulator error: {0}")]
    Sim(#[from] SimError),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("committed fixture manifest not found at {0}")]
    ManifestMissing(PathBuf),
}

/// Selectable synthetic generation suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthSuite {
    All,
    Ft8Awgn,
    Ft4Awgn,
    Ft8Crowded,
    Ft4Crowded,
    Ft8Offsets,
    Ft4Offsets,
    Ft8LateTx,
    Ft4LateTx,
    Ft8Noise,
    Ft4Noise,
    Ft8ApQso,
    Ft4ApQso,
}

impl SynthSuite {
    pub fn expand(self) -> Vec<SynthSuite> {
        match self {
            Self::All => vec![
                Self::Ft8Awgn,
                Self::Ft4Awgn,
                Self::Ft8Crowded,
                Self::Ft4Crowded,
                Self::Ft8Offsets,
                Self::Ft4Offsets,
                Self::Ft8LateTx,
                Self::Ft4LateTx,
                Self::Ft8Noise,
                Self::Ft4Noise,
                Self::Ft8ApQso,
                Self::Ft4ApQso,
            ],
            other => vec![other],
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Ft8Awgn => "ft8_awgn",
            Self::Ft4Awgn => "ft4_awgn",
            Self::Ft8Crowded => "ft8_crowded",
            Self::Ft4Crowded => "ft4_crowded",
            Self::Ft8Offsets => "ft8_offsets",
            Self::Ft4Offsets => "ft4_offsets",
            Self::Ft8LateTx => "ft8_late_tx",
            Self::Ft4LateTx => "ft4_late_tx",
            Self::Ft8Noise => "ft8_noise",
            Self::Ft4Noise => "ft4_noise",
            Self::Ft8ApQso => "ft8_ap_qso",
            Self::Ft4ApQso => "ft4_ap_qso",
        }
    }
}

/// Optional QSO context attached to AP corpus cases (`ft8_ap_qso` / `ft4_ap_qso`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApCaseContext {
    pub my_call: String,
    pub his_call: String,
    pub his_grid: String,
    pub qso_progress: u8,
    pub rx_freq_hz: u16,
    pub expected_ap_kind: u8,
}

/// One entry in `fixtures/committed/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommittedFixture {
    pub fixture_id: String,
    pub category: String,
    pub mode: Mode,
    /// Path relative to `wsjtx/rust-ftx/`.
    pub relative_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub samples: usize,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_txt_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ground_truth: Vec<SignalSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommittedManifest {
    pub schema_version: u32,
    pub fixtures: Vec<CommittedFixture>,
}

/// One generated synthetic WAV case in `.oracle-cache/corpus/<seed_version>/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedCase {
    pub suite: String,
    pub case_id: String,
    pub trial_idx: usize,
    pub mode: Mode,
    /// Relative path from the corpus version directory (always named `000000_<6digits>.wav`).
    pub relative_path: String,
    pub sha256: String,
    pub slot: SlotSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snr_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ap_context: Option<ApCaseContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedCorpusManifest {
    pub schema_version: u32,
    pub seed_version: String,
    pub trials_per_snr: usize,
    pub noise_trials: usize,
    pub cases: Vec<GeneratedCase>,
}

/// Loads `fixtures/committed/manifest.json` from `rust_ftx_root`.
pub fn load_committed_manifest(rust_ftx_root: &Path) -> Result<CommittedManifest, CorpusError> {
    let path = rust_ftx_root.join("fixtures/committed/manifest.json");
    if !path.exists() {
        return Err(CorpusError::ManifestMissing(path));
    }
    let text = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Initializes the 4 compact frozen regression WAVs and writes `fixtures/committed/manifest.json`.
///
/// Expects `fixtures/committed/ft8/191111_110200.wav` and `.txt` to already be copied from
/// read-only `digi-dx/tests/fixtures/`.
pub fn init_committed_fixtures(
    rust_ftx_root: &Path,
    sim: &SimRunner,
    overwrite_frozen: bool,
) -> Result<CommittedManifest, CorpusError> {
    let ft8_dir = rust_ftx_root.join("fixtures/committed/ft8");
    let ft4_dir = rust_ftx_root.join("fixtures/committed/ft4");
    fs::create_dir_all(&ft8_dir)?;
    fs::create_dir_all(&ft4_dir)?;

    // 1. ft8_crowded_20db (000000_000101.wav): 10 signals, +2 to -18 dB (20 dB DR).
    let ft8_crowded_signals = vec![
        SignalSpec { message: "CQ N0AAA EM10".into(), af_hz: 620.0, dt_sec: 0.1, snr_db: 2.0, late_start_ms: None },
        SignalSpec { message: "K9BBB AA1AA -04".into(), af_hz: 690.0, dt_sec: -0.2, snr_db: -16.0, late_start_ms: None },
        SignalSpec { message: "CQ DX BB2BB FN20".into(), af_hz: 940.0, dt_sec: 0.3, snr_db: -3.0, late_start_ms: None },
        SignalSpec { message: "VE3CCC DL4DDD R-11".into(), af_hz: 1010.0, dt_sec: 0.0, snr_db: -18.0, late_start_ms: None },
        SignalSpec { message: "JA5EEE VK6FFF RR73".into(), af_hz: 1320.0, dt_sec: -0.3, snr_db: 0.0, late_start_ms: None },
        SignalSpec { message: "CQ G7GGG IO91".into(), af_hz: 1385.0, dt_sec: 0.2, snr_db: -14.0, late_start_ms: None },
        SignalSpec { message: "F8HHH EA9III 73".into(), af_hz: 1750.0, dt_sec: 0.4, snr_db: -8.0, late_start_ms: None },
        SignalSpec { message: "PY0JJJ N0AAA -12".into(), af_hz: 1815.0, dt_sec: -0.1, snr_db: -17.0, late_start_ms: None },
        SignalSpec { message: "CQ K9BBB DM79".into(), af_hz: 2210.0, dt_sec: 0.0, snr_db: -6.0, late_start_ms: None },
        SignalSpec { message: "AA1AA VE3CCC RRR".into(), af_hz: 2280.0, dt_sec: 0.5, snr_db: -12.0, late_start_ms: None },
    ];
    let ft8_crowded_path = ft8_dir.join("000000_000101.wav");
    if overwrite_frozen || !ft8_crowded_path.exists() {
        let seed = derive_seed("v1", "committed_reg", Mode::Ft8, "ft8_crowded_20db", 0);
        let slot = SlotSpec {
            mode: Mode::Ft8,
            signals: ft8_crowded_signals.clone(),
            truncate_ms: None,
        };
        let res = synthesize_slot(sim, seed, &slot)?;
        wav::write_wav_12k_mono(&ft8_crowded_path, &res.samples)?;
    }

    // 2. ft8_fading_mm (000000_000102.wav): Watterson Mid-Latitude Moderate (fspread=1.0 Hz, delay=1.0 ms).
    // Generated once via ft8sim and frozen in git (Watterson channel is stochastic per run).
    let ft8_fading_signals = vec![SignalSpec {
        message: "CQ N0AAA EM10".into(),
        af_hz: 1500.0,
        dt_sec: 0.0,
        snr_db: -10.0,
        late_start_ms: None,
    }];
    let ft8_fading_path = ft8_dir.join("000000_000102.wav");
    if overwrite_frozen || !ft8_fading_path.exists() {
        let (samples, _) = sim.run_ft8sim("CQ N0AAA EM10", 1500.0, 0.0, 1.0, 1.0, -10.0)?;
        wav::write_wav_12k_mono(&ft8_fading_path, &samples)?;
    }

    // 3. ft8_late_tx_5s (000000_000103.wav): +5.0 s UTC-aligned late-start FT8 at -5 dB.
    let ft8_late_signals = vec![SignalSpec {
        message: "CQ N0AAA EM10".into(),
        af_hz: 1500.0,
        dt_sec: 0.0,
        snr_db: -5.0,
        late_start_ms: Some(5_000),
    }];
    let ft8_late_path = ft8_dir.join("000000_000103.wav");
    if overwrite_frozen || !ft8_late_path.exists() {
        let seed = derive_seed("v1", "committed_reg", Mode::Ft8, "ft8_late_tx_5s", 0);
        let slot = SlotSpec {
            mode: Mode::Ft8,
            signals: ft8_late_signals.clone(),
            truncate_ms: None,
        };
        let res = synthesize_slot(sim, seed, &slot)?;
        wav::write_wav_12k_mono(&ft8_late_path, &res.samples)?;
    }

    // 4. ft4_crowded_20db (000000_000201.wav): 10 FT4 signals spanning +2 to -18 dB (20 dB DR).
    let ft4_crowded_signals = vec![
        SignalSpec { message: "CQ N0AAA EM10".into(), af_hz: 450.0, dt_sec: 0.05, snr_db: 2.0, late_start_ms: None },
        SignalSpec { message: "K9BBB AA1AA -05".into(), af_hz: 560.0, dt_sec: -0.10, snr_db: -14.0, late_start_ms: None },
        SignalSpec { message: "CQ DX BB2BB FN20".into(), af_hz: 820.0, dt_sec: 0.15, snr_db: -2.0, late_start_ms: None },
        SignalSpec { message: "VE3CCC DL4DDD R-11".into(), af_hz: 940.0, dt_sec: 0.00, snr_db: -18.0, late_start_ms: None },
        SignalSpec { message: "JA5EEE VK6FFF RR73".into(), af_hz: 1260.0, dt_sec: -0.15, snr_db: 0.0, late_start_ms: None },
        SignalSpec { message: "CQ G7GGG IO91".into(), af_hz: 1380.0, dt_sec: 0.10, snr_db: -13.0, late_start_ms: None },
        SignalSpec { message: "F8HHH EA9III 73".into(), af_hz: 1740.0, dt_sec: 0.20, snr_db: -7.0, late_start_ms: None },
        SignalSpec { message: "PY0JJJ N0AAA -10".into(), af_hz: 1860.0, dt_sec: -0.05, snr_db: -16.0, late_start_ms: None },
        SignalSpec { message: "CQ K9BBB DM79".into(), af_hz: 2300.0, dt_sec: 0.00, snr_db: -5.0, late_start_ms: None },
        SignalSpec { message: "AA1AA VE3CCC RRR".into(), af_hz: 2420.0, dt_sec: 0.10, snr_db: -11.0, late_start_ms: None },
    ];
    let ft4_crowded_path = ft4_dir.join("000000_000201.wav");
    if overwrite_frozen || !ft4_crowded_path.exists() {
        let seed = derive_seed("v1", "committed_reg", Mode::Ft4, "ft4_crowded_20db", 0);
        let slot = SlotSpec {
            mode: Mode::Ft4,
            signals: ft4_crowded_signals.clone(),
            truncate_ms: None,
        };
        let res = synthesize_slot(sim, seed, &slot)?;
        wav::write_wav_12k_mono(&ft4_crowded_path, &res.samples)?;
    }

    let entries = vec![
        build_committed_entry(
            rust_ftx_root,
            "wsjtx_sample_ft8_210703_133430",
            "committed_real",
            Mode::Ft8,
            "../samples/FT8/210703_133430.wav",
            "Real crowded HF off-air FT8 recording from WSJT-X samples (GPLv3)",
            None,
            Vec::new(),
        )?,
        build_committed_entry(
            rust_ftx_root,
            "wsjtx_sample_ft4_000002",
            "committed_real",
            Mode::Ft4,
            "../samples/FT4/000000_000002.wav",
            "Real/reference multi-signal FT4 recording from WSJT-X samples (GPLv3)",
            None,
            Vec::new(),
        )?,
        build_committed_entry(
            rust_ftx_root,
            "digidx_fixture_ft8_191111_110200",
            "committed_real",
            Mode::Ft8,
            "fixtures/committed/ft8/191111_110200.wav",
            "Real HF FT8 reference fixture (5 known decodes; originally kgoba/ft8_lib MIT)",
            Some("fixtures/committed/ft8/191111_110200.txt".to_string()),
            Vec::new(),
        )?,
        build_committed_entry(
            rust_ftx_root,
            "ft8_crowded_20db",
            "committed_reg",
            Mode::Ft8,
            "fixtures/committed/ft8/000000_000101.wav",
            "Deterministic 10-signal FT8 slot spanning +2 to -18 dB (20 dB dynamic range)",
            None,
            ft8_crowded_signals,
        )?,
        build_committed_entry(
            rust_ftx_root,
            "ft8_fading_mm",
            "committed_reg",
            Mode::Ft8,
            "fixtures/committed/ft8/000000_000102.wav",
            "Frozen Watterson Mid-Latitude Moderate fading FT8 slot (fspread=1.0 Hz, delay=1.0 ms, -10 dB)",
            None,
            ft8_fading_signals,
        )?,
        build_committed_entry(
            rust_ftx_root,
            "ft8_late_tx_5s",
            "committed_reg",
            Mode::Ft8,
            "fixtures/committed/ft8/000000_000103.wav",
            "Deterministic +5.0 s UTC-aligned late-start FT8 slot at -5 dB",
            None,
            ft8_late_signals,
        )?,
        build_committed_entry(
            rust_ftx_root,
            "ft4_crowded_20db",
            "committed_reg",
            Mode::Ft4,
            "fixtures/committed/ft4/000000_000201.wav",
            "Deterministic 10-signal FT4 slot spanning +2 to -18 dB (20 dB dynamic range)",
            None,
            ft4_crowded_signals,
        )?,
    ];

    let manifest = CommittedManifest {
        schema_version: 1,
        fixtures: entries,
    };
    let manifest_path = rust_ftx_root.join("fixtures/committed/manifest.json");
    let json = serde_json::to_string_pretty(&manifest)?;
    fs::write(&manifest_path, format!("{json}\n"))?;
    Ok(manifest)
}

#[allow(clippy::too_many_arguments)]
fn build_committed_entry(
    rust_ftx_root: &Path,
    fixture_id: &str,
    category: &str,
    mode: Mode,
    relative_path: &str,
    description: &str,
    reference_txt_path: Option<String>,
    ground_truth: Vec<SignalSpec>,
) -> Result<CommittedFixture, CorpusError> {
    let full_path = rust_ftx_root.join(relative_path);
    let samples = wav::read_wav_12k_mono(&full_path)?;
    let sha256 = wav::sha256_file(&full_path)?;
    let size_bytes = fs::metadata(&full_path)?.len();
    Ok(CommittedFixture {
        fixture_id: fixture_id.to_string(),
        category: category.to_string(),
        mode,
        relative_path: relative_path.to_string(),
        sha256,
        size_bytes,
        samples: samples.len(),
        description: description.to_string(),
        reference_txt_path,
        ground_truth,
    })
}

/// Generates one or more deterministic synthetic suites into `<out_dir>` and writes `manifest.json`.
pub fn generate_synthetic_corpus(
    sim: &SimRunner,
    out_dir: &Path,
    suite: SynthSuite,
    trials_per_snr: usize,
    noise_trials: usize,
    seed_version: &str,
) -> Result<GeneratedCorpusManifest, CorpusError> {
    fs::create_dir_all(out_dir)?;

    let mut cases = Vec::new();
    let mut file_seq = 1usize;

    for s in suite.expand() {
        let suite_name = s.as_str();
        let suite_dir = out_dir.join(suite_name);
        fs::create_dir_all(&suite_dir)?;

        let specs = build_suite_cases(s, trials_per_snr, noise_trials);
        for spec in specs {
            let fname = format!("000000_{file_seq:06}.wav");
            file_seq += 1;
            let rel_path = format!("{suite_name}/{fname}");
            let abs_path = out_dir.join(&rel_path);

            let seed = derive_seed(
                seed_version,
                suite_name,
                spec.slot.mode,
                &spec.case_id,
                spec.trial_idx,
            );
            let synth = synthesize_slot(sim, seed, &spec.slot)?;
            debug_assert!(synth.gain > 0.0);
            if spec.slot.truncate_ms.is_none() {
                assert!(
                    synth.rms_scaled >= 15.0,
                    "generated slot {suite_name}/{} rms {:.2} too close to jt9 silence gate",
                    spec.case_id,
                    synth.rms_scaled
                );
            }
            wav::write_wav_12k_mono(&abs_path, &synth.samples)?;
            let sha256 = wav::sha256_file(&abs_path)?;

            cases.push(GeneratedCase {
                suite: suite_name.to_string(),
                case_id: spec.case_id,
                trial_idx: spec.trial_idx,
                mode: spec.slot.mode,
                relative_path: rel_path,
                sha256,
                slot: spec.slot,
                label: spec.label,
                snr_db: spec.snr_db,
                ap_context: spec.ap_context,
            });
        }
    }

    let manifest = GeneratedCorpusManifest {
        schema_version: 1,
        seed_version: seed_version.to_string(),
        trials_per_snr,
        noise_trials,
        cases,
    };
    let manifest_path = out_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest)?;
    fs::write(&manifest_path, format!("{json}\n"))?;
    Ok(manifest)
}

/// Loads a previously generated synthetic corpus manifest from `<corpus_dir>/manifest.json`.
pub fn load_generated_manifest(corpus_dir: &Path) -> Result<GeneratedCorpusManifest, CorpusError> {
    let path = corpus_dir.join("manifest.json");
    if !path.exists() {
        return Err(CorpusError::ManifestMissing(path));
    }
    let text = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text)?)
}

struct PlannedCase {
    case_id: String,
    trial_idx: usize,
    slot: SlotSpec,
    label: Option<String>,
    snr_db: Option<f64>,
    ap_context: Option<ApCaseContext>,
}

fn build_suite_cases(
    suite: SynthSuite,
    trials_per_snr: usize,
    noise_trials: usize,
) -> Vec<PlannedCase> {
    match suite {
        SynthSuite::All => unreachable!("All must be expanded"),
        SynthSuite::Ft8Awgn => build_awgn_sweep(Mode::Ft8, 0, -24, trials_per_snr),
        SynthSuite::Ft4Awgn => build_awgn_sweep(Mode::Ft4, 0, -21, trials_per_snr),
        SynthSuite::Ft8Crowded => build_crowded_suite(Mode::Ft8),
        SynthSuite::Ft4Crowded => build_crowded_suite(Mode::Ft4),
        SynthSuite::Ft8Offsets => build_offsets_suite(Mode::Ft8),
        SynthSuite::Ft4Offsets => build_offsets_suite(Mode::Ft4),
        SynthSuite::Ft8LateTx => build_late_tx_suite(Mode::Ft8),
        SynthSuite::Ft4LateTx => build_late_tx_suite(Mode::Ft4),
        SynthSuite::Ft8Noise => build_noise_suite(Mode::Ft8, noise_trials),
        SynthSuite::Ft4Noise => build_noise_suite(Mode::Ft4, noise_trials),
        SynthSuite::Ft8ApQso => build_ap_qso_suite(Mode::Ft8),
        SynthSuite::Ft4ApQso => build_ap_qso_suite(Mode::Ft4),
    }
}

fn build_awgn_sweep(
    mode: Mode,
    snr_max: i32,
    snr_min: i32,
    trials_per_snr: usize,
) -> Vec<PlannedCase> {
    let mut out = Vec::new();
    let mut snr = snr_max;
    while snr >= snr_min {
        for trial in 0..trials_per_snr {
            let af_hz = 1500.0;
            out.push(PlannedCase {
                case_id: format!("snr_{snr}"),
                trial_idx: trial,
                slot: SlotSpec {
                    mode,
                    signals: vec![SignalSpec {
                        message: "CQ N0AAA EM10".into(),
                        af_hz,
                        dt_sec: 0.0,
                        snr_db: f64::from(snr),
                        late_start_ms: None,
                    }],
                    truncate_ms: None,
                },
                label: Some(format!("{snr} dB")),
                snr_db: Some(f64::from(snr)),
                ap_context: None,
            });
        }
        snr -= 1;
    }
    out
}

fn build_crowded_suite(mode: Mode) -> Vec<PlannedCase> {
    let message_pool = [
        "CQ N0AAA EM10",
        "K9BBB AA1AA -04",
        "CQ DX BB2BB FN20",
        "VE3CCC DL4DDD R-11",
        "JA5EEE VK6FFF RR73",
        "CQ G7GGG IO91",
        "F8HHH EA9III 73",
        "PY0JJJ N0AAA -12",
        "CQ K9BBB DM79",
        "AA1AA VE3CCC RRR",
    ];
    let snrs_ft8 = [2.0, -18.0, -2.0, -20.0, 0.0, -14.0, -6.0, -17.0, -4.0, -11.0];
    let snrs_ft4 = [2.0, -15.0, -2.0, -18.0, 0.0, -12.0, -6.0, -16.0, -4.0, -10.0];
    let snrs = match mode {
        Mode::Ft8 => &snrs_ft8,
        Mode::Ft4 => &snrs_ft4,
    };
    let dts_ft8 = [0.1, -0.2, 0.3, 0.0, -0.3, 0.2, 0.4, -0.1, 0.0, 0.5];
    let dts_ft4 = [0.05, -0.1, 0.15, 0.0, -0.15, 0.1, 0.2, -0.05, 0.0, 0.1];
    let dts = match mode {
        Mode::Ft8 => &dts_ft8,
        Mode::Ft4 => &dts_ft4,
    };

    let mut out = Vec::with_capacity(5);
    for slot_idx in 0..5usize {
        let base_shift = (slot_idx as f64) * 35.0;
        let pair_step = match mode {
            Mode::Ft8 => 65.0,  // Tight 65 Hz spacing inside pairs (50 Hz BW)
            Mode::Ft4 => 115.0, // Tight 115 Hz spacing inside pairs (83 Hz BW)
        };
        let group_step = match mode {
            Mode::Ft8 => 360.0,
            Mode::Ft4 => 480.0,
        };

        let mut signals = Vec::with_capacity(10);
        for k in 0..10usize {
            let group = k / 2;
            let in_pair = k % 2;
            let af_hz = 400.0 + base_shift + (group as f64) * group_step + (in_pair as f64) * pair_step;
            let msg_idx = (k + slot_idx * 2) % message_pool.len();
            signals.push(SignalSpec {
                message: message_pool[msg_idx].to_string(),
                af_hz,
                dt_sec: dts[(k + slot_idx) % dts.len()],
                snr_db: snrs[(k + slot_idx) % snrs.len()],
                late_start_ms: None,
            });
        }

        out.push(PlannedCase {
            case_id: format!("crowded_{slot_idx}"),
            trial_idx: slot_idx,
            slot: SlotSpec {
                mode,
                signals,
                truncate_ms: None,
            },
            label: Some(format!("crowded_slot_{slot_idx}")),
            snr_db: None,
            ap_context: None,
        });
    }
    out
}

fn build_offsets_suite(mode: Mode) -> Vec<PlannedCase> {
    let dts: &[f64] = match mode {
        Mode::Ft8 => &[-2.0, -1.0, 0.0, 1.0, 2.0],
        Mode::Ft4 => &[-0.5, -0.25, 0.0, 0.25, 0.5, 0.75],
    };
    let afs = [250.0, 1500.0, 3500.0];
    let snrs = [-10.0, -16.0];
    let mut out = Vec::new();
    for &snr in &snrs {
        for &af in &afs {
            for &dt in dts {
                let cid = format!("af{}_dt{:.2}_snr{}", af as i32, dt, snr as i32);
                out.push(PlannedCase {
                    case_id: cid.clone(),
                    trial_idx: 0,
                    slot: SlotSpec {
                        mode,
                        signals: vec![SignalSpec {
                            message: "CQ N0AAA EM10".into(),
                            af_hz: af,
                            dt_sec: dt,
                            snr_db: snr,
                            late_start_ms: None,
                        }],
                        truncate_ms: None,
                    },
                    label: Some(cid),
                    snr_db: Some(snr),
                    ap_context: None,
                });
            }
        }
    }
    out
}

fn build_late_tx_suite(mode: Mode) -> Vec<PlannedCase> {
    let mut out = Vec::new();
    match mode {
        Mode::Ft8 => {
            for late_ms in [1_000u32, 2_000, 3_000, 4_000, 5_000] {
                let cid = format!("late_tx_{late_ms}ms");
                out.push(PlannedCase {
                    case_id: cid.clone(),
                    trial_idx: 0,
                    slot: SlotSpec {
                        mode,
                        signals: vec![SignalSpec {
                            message: "CQ N0AAA EM10".into(),
                            af_hz: 1500.0,
                            dt_sec: 0.0,
                            snr_db: -5.0,
                            late_start_ms: Some(late_ms),
                        }],
                        truncate_ms: None,
                    },
                    label: Some(cid),
                    snr_db: Some(-5.0),
                    ap_context: None,
                });
            }
            for trunc_ms in [11_800u32, 13_500, 14_400, 14_800] {
                let cid = format!("partial_rx_{trunc_ms}ms");
                out.push(PlannedCase {
                    case_id: cid.clone(),
                    trial_idx: 0,
                    slot: SlotSpec {
                        mode,
                        signals: vec![SignalSpec {
                            message: "CQ N0AAA EM10".into(),
                            af_hz: 1500.0,
                            dt_sec: 0.0,
                            snr_db: -10.0,
                            late_start_ms: None,
                        }],
                        truncate_ms: Some(trunc_ms),
                    },
                    label: Some(cid),
                    snr_db: Some(-10.0),
                    ap_context: None,
                });
            }
        }
        Mode::Ft4 => {
            for late_ms in [500u32, 1_000, 1_500] {
                let cid = format!("late_tx_{late_ms}ms");
                out.push(PlannedCase {
                    case_id: cid.clone(),
                    trial_idx: 0,
                    slot: SlotSpec {
                        mode,
                        signals: vec![SignalSpec {
                            message: "CQ N0AAA EM10".into(),
                            af_hz: 1500.0,
                            dt_sec: 0.0,
                            snr_db: -5.0,
                            late_start_ms: Some(late_ms),
                        }],
                        truncate_ms: None,
                    },
                    label: Some(cid),
                    snr_db: Some(-5.0),
                    ap_context: None,
                });
            }
            for trunc_ms in [5_600u32, 6_048] {
                let cid = format!("partial_rx_{trunc_ms}ms");
                out.push(PlannedCase {
                    case_id: cid.clone(),
                    trial_idx: 0,
                    slot: SlotSpec {
                        mode,
                        signals: vec![SignalSpec {
                            message: "CQ N0AAA EM10".into(),
                            af_hz: 1500.0,
                            dt_sec: 0.0,
                            snr_db: -10.0,
                            late_start_ms: None,
                        }],
                        truncate_ms: Some(trunc_ms),
                    },
                    label: Some(cid),
                    snr_db: Some(-10.0),
                    ap_context: None,
                });
            }
        }
    }
    out
}

fn build_noise_suite(mode: Mode, noise_trials: usize) -> Vec<PlannedCase> {
    (0..noise_trials)
        .map(|trial| PlannedCase {
            case_id: format!("noise_{trial:04}"),
            trial_idx: trial,
            slot: SlotSpec {
                mode,
                signals: Vec::new(),
                truncate_ms: None,
            },
            label: Some(format!("noise_{trial:04}")),
            snr_db: None,
            ap_context: None,
        })
        .collect()
}

fn build_ap_qso_suite(mode: Mode) -> Vec<PlannedCase> {
    // Representative QSOProgress 0..=5 messages exercising a1..a6 for N0AAA / K9BBB / DM79.
    // Note: in FT4, QSOProgress 3/4 activate [3, 6] (a4/a5 are FT8-only).
    let templates: &[(u8, u8, &str)] = match mode {
        Mode::Ft8 => &[
            (0, 1, "CQ K9BBB DM79"),     // a1
            (1, 2, "N0AAA VE3CCC FN31"), // a2
            (2, 3, "N0AAA K9BBB R-12"),  // a3
            (3, 4, "N0AAA K9BBB RRR"),   // a4
            (3, 5, "N0AAA K9BBB 73"),    // a5
            (4, 6, "N0AAA K9BBB RR73"),  // a6
            (5, 3, "N0AAA K9BBB -15"),   // QSOProgress=5 -> a3/a1/a2
        ],
        Mode::Ft4 => &[
            (0, 1, "CQ K9BBB DM79"),     // a1
            (1, 2, "N0AAA VE3CCC FN31"), // a2
            (2, 3, "N0AAA K9BBB R-12"),  // a3
            (3, 6, "N0AAA K9BBB RR73"),  // a6
            (4, 6, "N0AAA K9BBB RR73"),  // a6
            (5, 3, "N0AAA K9BBB -15"),   // QSOProgress=5 -> a3/a1/a2
        ],
    };

    let snrs: &[i32] = match mode {
        Mode::Ft8 => &[-18, -20, -21, -22, -23, -24],
        Mode::Ft4 => &[-15, -17, -18, -19, -20, -21],
    };

    let mut out = Vec::new();
    for &(prog, ap_kind, msg) in templates {
        for &snr in snrs {
            for trial in 0..3usize {
                let cid = format!("q{prog}_a{ap_kind}_snr{snr}");
                out.push(PlannedCase {
                    case_id: cid.clone(),
                    trial_idx: trial,
                    slot: SlotSpec {
                        mode,
                        signals: vec![SignalSpec {
                            message: msg.to_string(),
                            af_hz: 1500.0,
                            dt_sec: 0.0,
                            snr_db: f64::from(snr),
                            late_start_ms: None,
                        }],
                        truncate_ms: None,
                    },
                    label: Some(cid),
                    snr_db: Some(f64::from(snr)),
                    ap_context: Some(ApCaseContext {
                        my_call: "N0AAA".into(),
                        his_call: "K9BBB".into(),
                        his_grid: "DM79".into(),
                        qso_progress: prog,
                        rx_freq_hz: 1500,
                        expected_ap_kind: ap_kind,
                    }),
                });
            }
        }
    }
    out
}
