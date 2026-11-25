// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Pure-Rust FT8 and FT4 modem core library.
//!
//! Stage 0 provides the stable public API contract, verified protocol constants,
//! and module skeleton for Stages 1–5.

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod dsp;
pub mod fec;
pub mod ft4;
pub mod ft8;
pub mod modulate;
pub mod protocol;
pub mod telemetry;

pub use telemetry::{DecodeTelemetry, FecMethod};

/// Digital mode supported by `ftxr-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ft8,
    Ft4,
}

impl Mode {
    /// Standard internal RX baseband sample rate (Hz), matching WSJT-X `jt9`.
    pub const SAMPLE_RATE_HZ: u32 = 12_000;

    /// Nominal UTC transmit/receive slot cadence in milliseconds.
    pub const fn slot_duration_ms(self) -> u32 {
        match self {
            Self::Ft8 => 15_000,
            Self::Ft4 => 7_500,
        }
    }

    /// Number of 12 kHz PCM samples consumed by the classic WSJT-X decoder (`NMAX`).
    pub const fn rx_window_samples(self) -> usize {
        match self {
            Self::Ft8 => ft8::params::NMAX,
            Self::Ft4 => ft4::params::NMAX,
        }
    }

    /// Nominal TX start delay from the UTC slot boundary (ms), matching WSJT-X `Modulator.cpp`.
    pub const fn nominal_tx_delay_ms(self) -> u32 {
        match self {
            Self::Ft8 => ft8::params::NOMINAL_TX_DELAY_MS,
            Self::Ft4 => ft4::params::NOMINAL_TX_DELAY_MS,
        }
    }

    /// Single-character mode delimiter emitted by `jt9` stdout (`~` for FT8, `+` for FT4).
    pub const fn jt9_mode_marker(self) -> char {
        match self {
            Self::Ft8 => '~',
            Self::Ft4 => '+',
        }
    }

    /// Default AP frequency window half-width (Hz) around `rx_freq_hz`/`tx_freq_hz`.
    pub const fn default_ap_width_hz(self) -> f32 {
        match self {
            Self::Ft8 => 75.0,
            Self::Ft4 => 50.0,
        }
    }
}

/// WSJT-X contest mode selector (`ncontest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum ContestMode {
    #[default]
    None = 0,
    NaVhf = 1,
    EuVhf = 2,
    FieldDay = 3,
    RttyRoundup = 4,
    WwDigi = 5,
    Fox = 6,
    Hound = 7,
    ArrlDigi = 8,
}

/// Optional QSO context driving single-slot A Priori (`a1`–`a6`) hypotheses and callsign hashing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QsoContext {
    pub my_call: Option<String>,
    pub his_call: Option<String>,
    pub his_grid: Option<String>,
    /// WSJT-X `nQSOProgress` (`0..=5`: 0=CALLING/CQ, 1=REPORT, 2=ROGER_REPORT, 3=ROGERS, 4=SIGNOFF, 5=CALLING_73).
    pub qso_progress: u8,
    pub rx_freq_hz: f32,
    pub tx_freq_hz: f32,
    pub ap_enabled: bool,
    pub ap_cq_only: bool,
    /// AP frequency window around `rx_freq_hz`/`tx_freq_hz` (`75.0` Hz FT8, `50.0` Hz FT4).
    pub ap_width_hz: f32,
    pub contest: ContestMode,
}

impl Default for QsoContext {
    fn default() -> Self {
        Self {
            my_call: None,
            his_call: None,
            his_grid: None,
            qso_progress: 0,
            rx_freq_hz: 1500.0,
            tx_freq_hz: 1500.0,
            ap_enabled: false,
            ap_cq_only: false,
            ap_width_hz: 75.0,
            contest: ContestMode::None,
        }
    }
}

/// Configuration for a single-slot RX decode pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeConfig {
    pub mode: Mode,
    /// Decoding depth: `1` (Fast/BP), `2` (Normal), `3` (Deep/BP+OSD).
    pub depth: u8,
    pub freq_min_hz: f32,
    pub freq_max_hz: f32,
    pub rx_freq_hz: f32,
    pub freq_tol_hz: f32,
    pub qso: Option<QsoContext>,
    pub enable_telemetry: bool,
    pub max_threads: Option<usize>,
}

impl DecodeConfig {
    /// Creates a standard deep-decode configuration matching classic `jt9` defaults (`200..=4000 Hz`).
    pub fn for_mode(mode: Mode) -> Self {
        Self {
            mode,
            depth: 3,
            freq_min_hz: 200.0,
            freq_max_hz: 4000.0,
            rx_freq_hz: 1500.0,
            freq_tol_hz: 20.0,
            qso: None,
            enable_telemetry: false,
            max_threads: None,
        }
    }
}

/// Canonical single-slot A Priori (`AP`) tag emitted by `ftxr-core` (`a1`–`a6`).
///
/// Cross-slot `a7` and RX-frequency list `a8` are explicitly out of scope for `ftxr-core`
/// and are tracked separately inside `ftxr-oracle` for apples-to-apples comparisons against `jt9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ApTag {
    #[default]
    None,
    /// Single-slot AP decode `a1`..`a6`; `uncertain == true` iff `qual < 0.17`.
    Ap { kind: u8, uncertain: bool },
}

impl ApTag {
    /// WSJT-X AP decode quality threshold below which `? aN` uncertainty is marked (`decoder.f90`).
    pub const UNCERTAIN_QUAL_THRESHOLD: f32 = 0.17;

    /// Constructs an `ApTag` from WSJT-X `iaptype` (`0..=6`) and decode quality `qual`.
    pub fn from_wsjtx(iaptype: u8, qual: f32) -> Self {
        match iaptype {
            1..=6 => Self::Ap {
                kind: iaptype,
                uncertain: qual < Self::UNCERTAIN_QUAL_THRESHOLD,
            },
            _ => Self::None,
        }
    }

    /// Returns the AP hypothesis number (`1..=6`) if AP was used.
    pub const fn kind(self) -> Option<u8> {
        match self {
            Self::None => None,
            Self::Ap { kind, .. } => Some(kind),
        }
    }

    /// Returns `true` iff this decode is an uncertain AP decode (`? aN`).
    pub const fn is_uncertain(self) -> bool {
        match self {
            Self::None => false,
            Self::Ap { uncertain, .. } => uncertain,
        }
    }

    /// Formats WSJT-X / `digi-dx` `SlotDecodeReconciler`-compatible `raw_metadata`
    /// (`None`, `Some("a1")`..`Some("a6")`, or `Some("? a1")`..`Some("? a6")`).
    pub fn wsjtx_metadata(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Ap {
                kind,
                uncertain: false,
            } => Some(format!("a{kind}")),
            Self::Ap {
                kind,
                uncertain: true,
            } => Some(format!("? a{kind}")),
        }
    }
}

/// A single decoded FT8 or FT4 message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedMessage {
    pub mode: Mode,
    pub snr_db: i8,
    pub snr_raw: f32,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub message: String,
    pub ap_tag: ApTag,
    pub telemetry: Option<DecodeTelemetry>,
}

impl DecodedMessage {
    /// Returns `true` iff this decode is flagged as an uncertain AP decode (`? aN`).
    pub const fn ap_uncertain(&self) -> bool {
        self.ap_tag.is_uncertain()
    }

    /// Formats WSJT-X / `digi-dx` `SlotDecodeReconciler`-compatible `raw_metadata`
    /// (`None`, `Some("a1")`, `Some("? a1")`, ...).
    pub fn wsjtx_metadata(&self) -> Option<String> {
        self.ap_tag.wsjtx_metadata()
    }
}

/// Canonical TX modulation configuration for FT8 and FT4 GFSK waveform generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModulateConfig {
    pub mode: Mode,
    /// Base audio tone frequency in Hz.
    pub af_hz: f32,
    /// Output PCM sample rate in Hz (e.g., `12_000` or `48_000`).
    pub sample_rate: u32,
    /// Nominal TX start delay from UTC slot boundary in ms (`500` FT8, `300` FT4, or `0` raw active).
    pub start_delay_ms: u32,
    /// UTC-aligned late-TX / mid-slot hot-swap offset into the slot in milliseconds.
    pub offset_ms: u64,
    /// Output peak amplitude in `(0.0, 1.0]`.
    pub amplitude: f32,
}

impl ModulateConfig {
    /// Standard WSJT-X TX timing (`500 ms` FT8 / `300 ms` FT4 start delay, `0 ms` offset, amplitude `1.0`).
    pub const fn default_for(mode: Mode, af_hz: f32, sample_rate: u32) -> Self {
        Self {
            mode,
            af_hz,
            sample_rate,
            start_delay_ms: mode.nominal_tx_delay_ms(),
            offset_ms: 0,
            amplitude: 1.0,
        }
    }

    /// Raw active waveform starting immediately (`start_delay_ms = 0`, matching `digi-dx` Phase 7A TX).
    pub const fn raw_active(mode: Mode, af_hz: f32, sample_rate: u32) -> Self {
        Self {
            mode,
            af_hz,
            sample_rate,
            start_delay_ms: 0,
            offset_ms: 0,
            amplitude: 1.0,
        }
    }
}

/// Errors returned by `ftxr-core` operations.
#[derive(Debug, Error)]
pub enum FtxrError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
    #[error("invalid message: {0}")]
    InvalidMessage(String),
    #[error("failed to unpack 77-bit payload")]
    UnpackFailed,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid audio frequency {af_hz} Hz (expected {min_hz}..={max_hz} Hz)")]
    InvalidFrequency { af_hz: f32, min_hz: f32, max_hz: f32 },
    #[error("unsupported sample rate: {0} Hz")]
    InvalidSampleRate(u32),
    #[error("invalid tone value {tone} for mode {mode:?}")]
    InvalidTone { mode: Mode, tone: u8 },
    #[error("invalid audio length: got {actual} samples, expected at least {expected_min}")]
    InvalidAudioLength { expected_min: usize, actual: usize },
    #[error("operation cancelled")]
    Cancelled,
    #[error("internal modem error: {0}")]
    Internal(String),
}

/// Stateful, thread-safe FT8/FT4 modem engine (TX implemented in Stage 1; RX in Stages 2–5).
#[derive(Debug, Default)]
pub struct DecoderEngine {
    hash_table: protocol::hashtable::CallsignHashTable,
}

impl DecoderEngine {
    /// Creates a new modem engine instance with an empty session callsign hash table.
    pub fn new() -> Self {
        Self {
            hash_table: protocol::hashtable::CallsignHashTable::new(),
        }
    }

    /// Returns a reference to the session-scoped callsign hash table.
    pub fn hash_table(&self) -> &protocol::hashtable::CallsignHashTable {
        &self.hash_table
    }

    /// Clears session-scoped state (callsign hash tables) between bands or operating sessions.
    pub fn reset_session(&mut self) {
        self.hash_table.clear();
    }

    /// Decodes a 12 kHz mono baseband PCM buffer for the configured mode.
    pub fn decode(
        &mut self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let cancel = AtomicBool::new(false);
        self.decode_streaming(samples_12k, cfg, &cancel, |_| {})
    }

    /// Decodes a 12 kHz mono baseband PCM buffer, streaming decodes via `on_decode` and honoring `cancel`.
    pub fn decode_streaming<F>(
        &mut self,
        _samples_12k: &[f32],
        _cfg: &DecodeConfig,
        _cancel: &AtomicBool,
        _on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        Err(FtxrError::Unimplemented(
            "DecoderEngine::decode_streaming (Stage 2/4)",
        ))
    }

    /// Encodes and GFSK-modulates an FT8 or FT4 message into mono `f32` PCM samples.
    pub fn modulate(
        &mut self,
        message: &str,
        cfg: &ModulateConfig,
    ) -> Result<Vec<f32>, FtxrError> {
        let ctx = protocol::types::UnpackContext::default();
        let wave = modulate::modulate_message(message, cfg, &ctx, &self.hash_table)?;
        Ok(wave.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_constants_match_wsjtx() {
        assert_eq!(Mode::SAMPLE_RATE_HZ, 12_000);
        assert_eq!(Mode::Ft8.slot_duration_ms(), 15_000);
        assert_eq!(Mode::Ft4.slot_duration_ms(), 7_500);
        assert_eq!(Mode::Ft8.rx_window_samples(), 180_000);
        assert_eq!(Mode::Ft4.rx_window_samples(), 72_576);
        assert_eq!(Mode::Ft8.nominal_tx_delay_ms(), 500);
        assert_eq!(Mode::Ft4.nominal_tx_delay_ms(), 300);
        assert_eq!(Mode::Ft8.jt9_mode_marker(), '~');
        assert_eq!(Mode::Ft4.jt9_mode_marker(), '+');
        assert_eq!(Mode::Ft8.default_ap_width_hz(), 75.0);
        assert_eq!(Mode::Ft4.default_ap_width_hz(), 50.0);
    }

    #[test]
    fn protocol_param_invariants() {
        assert_eq!(ft8::params::NN, ft8::params::NS + ft8::params::ND);
        assert_eq!(ft8::params::NZ, ft8::params::NN * ft8::params::NSPS);
        assert_eq!(ft8::params::NMAX, 180_000);
        assert_eq!(ft8::params::NHSYM, 372);
        assert_eq!(ft8::params::ICOS7, [3, 1, 4, 0, 6, 5, 2]);
        assert_eq!(ft8::params::GRAYMAP, [0, 1, 3, 2, 5, 6, 4, 7]);

        assert_eq!(ft4::params::NN, ft4::params::NS + ft4::params::ND);
        assert_eq!(ft4::params::NN2, ft4::params::NN + 2);
        assert_eq!(ft4::params::NZ, ft4::params::NN * ft4::params::NSPS);
        assert_eq!(ft4::params::NZ2, ft4::params::NN2 * ft4::params::NSPS);
        assert_eq!(ft4::params::NMAX, 72_576);
        assert_eq!(ft4::params::NHSYM, 122);

        assert_eq!(protocol::rvec::FT4_RVEC.len(), 77);
        assert!(protocol::rvec::FT4_RVEC.iter().all(|&b| b <= 1));
    }

    #[test]
    fn ap_tag_and_wsjtx_metadata_formatting() {
        assert_eq!(ApTag::None.wsjtx_metadata(), None);
        assert_eq!(ApTag::from_wsjtx(0, 0.0), ApTag::None);
        assert_eq!(ApTag::from_wsjtx(7, 0.5), ApTag::None);

        let a1 = ApTag::from_wsjtx(1, 0.50);
        assert_eq!(
            a1,
            ApTag::Ap {
                kind: 1,
                uncertain: false
            }
        );
        assert_eq!(a1.wsjtx_metadata().as_deref(), Some("a1"));
        assert!(!a1.is_uncertain());

        let a3_uncertain = ApTag::from_wsjtx(3, 0.16);
        assert_eq!(
            a3_uncertain,
            ApTag::Ap {
                kind: 3,
                uncertain: true
            }
        );
        assert_eq!(a3_uncertain.wsjtx_metadata().as_deref(), Some("? a3"));
        assert!(a3_uncertain.is_uncertain());

        let a6_boundary = ApTag::from_wsjtx(6, 0.17);
        assert_eq!(a6_boundary.wsjtx_metadata().as_deref(), Some("a6"));

        let msg = DecodedMessage {
            mode: Mode::Ft8,
            snr_db: -21,
            snr_raw: -21.2,
            dt_sec: 0.2,
            af_hz: 1500.0,
            message: "N0AAA K9BBB R-12".to_string(),
            ap_tag: a3_uncertain,
            telemetry: None,
        };
        assert_eq!(msg.wsjtx_metadata().as_deref(), Some("? a3"));
        assert!(msg.ap_uncertain());
    }

    #[test]
    fn config_and_modulate_defaults() {
        let qso = QsoContext::default();
        assert!(!qso.ap_enabled);
        assert!(!qso.ap_cq_only);
        assert_eq!(qso.qso_progress, 0);
        assert_eq!(qso.rx_freq_hz, 1500.0);
        assert_eq!(qso.tx_freq_hz, 1500.0);
        assert_eq!(qso.ap_width_hz, 75.0);
        assert_eq!(qso.contest, ContestMode::None);

        let cfg = DecodeConfig::for_mode(Mode::Ft8);
        assert_eq!(cfg.mode, Mode::Ft8);
        assert_eq!(cfg.depth, 3);
        assert_eq!(cfg.freq_min_hz, 200.0);
        assert_eq!(cfg.freq_max_hz, 4000.0);
        assert_eq!(cfg.rx_freq_hz, 1500.0);
        assert_eq!(cfg.freq_tol_hz, 20.0);
        assert!(cfg.qso.is_none());
        assert!(!cfg.enable_telemetry);

        let mod_ft8 = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        assert_eq!(mod_ft8.start_delay_ms, 500);
        assert_eq!(mod_ft8.offset_ms, 0);
        assert_eq!(mod_ft8.amplitude, 1.0);

        let mod_ft4 = ModulateConfig::default_for(Mode::Ft4, 1500.0, 48_000);
        assert_eq!(mod_ft4.start_delay_ms, 300);

        let mod_raw = ModulateConfig::raw_active(Mode::Ft8, 1500.0, 12_000);
        assert_eq!(mod_raw.start_delay_ms, 0);
    }

    #[test]
    fn stub_decode_returns_unimplemented_and_modulate_succeeds() {
        let mut engine = DecoderEngine::new();
        let cfg = DecodeConfig::for_mode(Mode::Ft8);
        assert!(matches!(
            engine.decode(&[], &cfg),
            Err(FtxrError::Unimplemented(_))
        ));
        let mcfg = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        let pcm = engine.modulate("CQ N0AAA EM10", &mcfg).unwrap();
        assert_eq!(pcm.len(), 6_000 + 151_680);
    }
}
