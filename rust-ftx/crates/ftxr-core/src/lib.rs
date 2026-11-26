// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

#![forbid(unsafe_code)]

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

pub use ft8::ap::QsoProgress;
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
    /// True during live partial passes (e.g. 11.8s / 13.5s) to bound pass depth for low latency.
    #[serde(default)]
    pub early_pass: bool,
    /// True (default) to decode Tier 0 (`|f - rx_freq_hz| <= freq_tol_hz`) before Tier 1.
    #[serde(default = "default_true")]
    pub qso_first: bool,
}

const fn default_true() -> bool {
    true
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
            early_pass: false,
            qso_first: true,
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

/// Recommended worker thread count on the host machine, reserving I/O headroom when available.
pub fn recommended_worker_threads() -> usize {
    let avail = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    if avail <= 2 {
        avail
    } else if avail <= 4 {
        avail - 1
    } else {
        (avail - 2).clamp(1, 8)
    }
}

/// Stateful, thread-safe FT8/FT4 modem engine holding warm FFT plans, precomputed DSP/FEC tables,
/// RAII scratch pools, a session callsign hash table, and a private Rayon RX worker pool.
pub struct DecoderEngine {
    hash_table: protocol::hashtable::CallsignHashTable,
    fft_plans: std::sync::Arc<dsp::FftPlanCache>,
    tables: std::sync::Arc<dsp::PrecomputedTables>,
    pass_pool: dsp::PassScratchPool,
    cand_pool: dsp::CandidateScratchPool,
    rx_pool: std::sync::Arc<rayon::ThreadPool>,
    rx_threads: usize,
    override_pool: std::sync::Mutex<Option<(usize, std::sync::Arc<rayon::ThreadPool>)>>,
    legacy_ft8: Option<Box<ft8::decoder::Ft8Decoder>>,
    legacy_ft4: Option<Box<ft4::decoder::Ft4Decoder>>,
}

impl std::fmt::Debug for DecoderEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderEngine")
            .field("rx_threads", &self.rx_threads)
            .field("hash_entries", &self.hash_table.len())
            .field("pass_pool", &self.pass_pool)
            .field("cand_pool", &self.cand_pool)
            .finish_non_exhaustive()
    }
}

impl Default for DecoderEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn build_rx_thread_pool(threads: usize) -> Result<std::sync::Arc<rayon::ThreadPool>, FtxrError> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|idx| format!("ftxr-rx-{idx}"))
        .build()
        .map_err(|e| FtxrError::Internal(format!("failed to create rayon pool: {e}")))?;
    Ok(std::sync::Arc::new(pool))
}

impl DecoderEngine {
    /// Creates a new modem engine with warm FFT plans, auto-sized worker pool, and empty hash table.
    pub fn new() -> Self {
        Self::with_threads(None).expect("auto rayon thread pool must initialize")
    }

    /// Creates a new modem engine configured for `threads` RX worker threads (`None` uses auto heuristic).
    pub fn with_threads(threads: Option<usize>) -> Result<Self, FtxrError> {
        let rx_threads = match threads {
            Some(0) => {
                return Err(FtxrError::InvalidConfig(
                    "threads must be >= 1".to_string(),
                ));
            }
            Some(n) => n.clamp(1, 32),
            None => recommended_worker_threads(),
        };

        let fft_plans = std::sync::Arc::new(dsp::FftPlanCache::new());
        let tables = std::sync::Arc::new(dsp::PrecomputedTables::new(std::sync::Arc::clone(
            &fft_plans,
        )));
        let pass_pool = dsp::PassScratchPool::new(std::sync::Arc::clone(&fft_plans));
        let cand_pool = dsp::CandidateScratchPool::new(std::sync::Arc::clone(&fft_plans));
        pass_pool.prewarm(1);
        cand_pool.prewarm(rx_threads);
        let rx_pool = build_rx_thread_pool(rx_threads)?;

        Ok(Self {
            hash_table: protocol::hashtable::CallsignHashTable::new(),
            fft_plans,
            tables,
            pass_pool,
            cand_pool,
            rx_pool,
            rx_threads,
            override_pool: std::sync::Mutex::new(None),
            legacy_ft8: None,
            legacy_ft4: None,
        })
    }

    /// Returns the default number of RX worker threads in this engine's pool.
    pub fn num_threads(&self) -> usize {
        self.rx_threads
    }

    /// Returns the number of pass-level scratch workspaces allocated in `PassScratchPool`.
    pub fn pass_pool_created(&self) -> usize {
        self.pass_pool.created_count()
    }

    /// Returns the number of candidate-level scratch workspaces allocated in `CandidateScratchPool`.
    pub fn cand_pool_created(&self) -> usize {
        self.cand_pool.created_count()
    }

    /// Returns a reference to the shared warm FFT plan cache.
    pub fn fft_plans(&self) -> &dsp::FftPlanCache {
        &self.fft_plans
    }

    /// Returns a reference to the session-scoped callsign hash table.
    pub fn hash_table(&self) -> &protocol::hashtable::CallsignHashTable {
        &self.hash_table
    }

    /// Returns a reference to a lazy-initialized legacy FT8 decoder (prefer `&self` methods on `DecoderEngine`).
    pub fn ft8_decoder_mut(&mut self) -> &mut ft8::decoder::Ft8Decoder {
        self.legacy_ft8
            .get_or_insert_with(|| Box::new(ft8::decoder::Ft8Decoder::new()))
    }

    /// Returns a reference to a lazy-initialized legacy FT4 decoder (prefer `&self` methods on `DecoderEngine`).
    pub fn ft4_decoder_mut(&mut self) -> &mut ft4::decoder::Ft4Decoder {
        self.legacy_ft4
            .get_or_insert_with(|| Box::new(ft4::decoder::Ft4Decoder::new()))
    }

    /// Clears session-scoped state (callsign hash tables) between bands or operating sessions,
    /// returning the number of callsign hash entries cleared.
    pub fn reset_session(&self) -> usize {
        self.hash_table.clear()
    }

    fn resolve_pool(
        &self,
        override_threads: Option<usize>,
    ) -> Result<std::sync::Arc<rayon::ThreadPool>, FtxrError> {
        let Some(n) = override_threads else {
            return Ok(std::sync::Arc::clone(&self.rx_pool));
        };
        if n == 0 {
            return Err(FtxrError::InvalidConfig(
                "max_threads must be >= 1".to_string(),
            ));
        }
        let n = n.clamp(1, 32);
        if n == self.rx_threads {
            return Ok(std::sync::Arc::clone(&self.rx_pool));
        }
        self.cand_pool.prewarm(n);
        let mut guard = self
            .override_pool
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((cached_n, ref cached_pool)) = *guard {
            if cached_n == n {
                return Ok(std::sync::Arc::clone(cached_pool));
            }
        }
        let pool = build_rx_thread_pool(n)?;
        *guard = Some((n, std::sync::Arc::clone(&pool)));
        Ok(pool)
    }

    /// Decodes a 12 kHz mono normalized `f32` (`[-1.0, 1.0]`) baseband PCM buffer.
    pub fn decode(
        &self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let cancel = AtomicBool::new(false);
        self.decode_streaming(samples_12k, cfg, &cancel, |_| {})
    }

    /// Decodes a 12 kHz mono normalized `f32` (`[-1.0, 1.0]`) baseband PCM buffer,
    /// streaming decodes via `on_decode` and honoring `cancel`.
    pub fn decode_streaming<F>(
        &self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
        cancel: &AtomicBool,
        on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        let pool = self.resolve_pool(cfg.max_threads)?;
        match cfg.mode {
            Mode::Ft8 => ft8::decoder::decode_ft8_shared_f32(
                samples_12k,
                cfg,
                &self.tables,
                &self.pass_pool,
                &self.cand_pool,
                Some(pool.as_ref()),
                &self.hash_table,
                cancel,
                ft8::decoder::Ft8DecodeOptions::default(),
                on_decode,
            ),
            Mode::Ft4 => ft4::decoder::decode_ft4_shared_f32(
                samples_12k,
                cfg,
                &self.tables,
                &self.pass_pool,
                &self.cand_pool,
                Some(pool.as_ref()),
                &self.hash_table,
                cancel,
                ft4::decoder::Ft4DecodeOptions::default(),
                on_decode,
            ),
        }
    }

    /// Decodes a 12 kHz mono normalized `f32` PCM buffer with explicit `Ft8DecodeOptions`
    /// (e.g., `wsjtx_three_stage` for crowded disk WAVs vs single-shot live partial passes).
    pub fn decode_f32_with_ft8_options<F>(
        &self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
        cancel: &AtomicBool,
        ft8_opts: ft8::decoder::Ft8DecodeOptions,
        on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        let pool = self.resolve_pool(cfg.max_threads)?;
        match cfg.mode {
            Mode::Ft8 => ft8::decoder::decode_ft8_shared_f32(
                samples_12k,
                cfg,
                &self.tables,
                &self.pass_pool,
                &self.cand_pool,
                Some(pool.as_ref()),
                &self.hash_table,
                cancel,
                ft8_opts,
                on_decode,
            ),
            Mode::Ft4 => ft4::decoder::decode_ft4_shared_f32(
                samples_12k,
                cfg,
                &self.tables,
                &self.pass_pool,
                &self.cand_pool,
                Some(pool.as_ref()),
                &self.hash_table,
                cancel,
                ft4::decoder::Ft4DecodeOptions::default(),
                on_decode,
            ),
        }
    }

    /// Decodes a 12 kHz mono signed 16-bit (`i16`) PCM buffer with exact `jt9` scaling (`s as f32`).
    pub fn decode_i16(
        &self,
        samples_12k: &[i16],
        cfg: &DecodeConfig,
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let cancel = AtomicBool::new(false);
        self.decode_i16_streaming(samples_12k, cfg, &cancel, |_| {})
    }

    /// Decodes a 12 kHz mono signed 16-bit (`i16`) PCM buffer, streaming decodes via `on_decode`.
    pub fn decode_i16_streaming<F>(
        &self,
        samples_12k: &[i16],
        cfg: &DecodeConfig,
        cancel: &AtomicBool,
        on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        let pool = self.resolve_pool(cfg.max_threads)?;
        match cfg.mode {
            Mode::Ft8 => ft8::decoder::decode_ft8_shared_i16(
                samples_12k,
                cfg,
                &self.tables,
                &self.pass_pool,
                &self.cand_pool,
                Some(pool.as_ref()),
                &self.hash_table,
                cancel,
                ft8::decoder::Ft8DecodeOptions::default(),
                on_decode,
            ),
            Mode::Ft4 => ft4::decoder::decode_ft4_shared_i16(
                samples_12k,
                cfg,
                &self.tables,
                &self.pass_pool,
                &self.cand_pool,
                Some(pool.as_ref()),
                &self.hash_table,
                cancel,
                ft4::decoder::Ft4DecodeOptions::default(),
                on_decode,
            ),
        }
    }

    /// Encodes and GFSK-modulates an FT8 or FT4 message into mono `f32` PCM samples.
    ///
    /// Thread-safe and lock-free with respect to active parallel RX candidate decoding.
    pub fn modulate(
        &self,
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
    fn ft8_and_ft4_modulate_and_decode_roundtrip() {
        let engine = DecoderEngine::new();
        let mut cfg_ft4 = DecodeConfig::for_mode(Mode::Ft4);
        cfg_ft4.depth = 1;
        assert!(engine.decode(&[], &cfg_ft4).unwrap().is_empty());

        let mcfg_ft4 = ModulateConfig::default_for(Mode::Ft4, 1500.0, 12_000);
        let pcm_ft4 = engine.modulate("CQ K1ABC FN42", &mcfg_ft4).unwrap();
        assert_eq!(pcm_ft4.len(), 3_600 + 60_480);

        let decoded_ft4 = engine.decode(&pcm_ft4, &cfg_ft4).unwrap();
        assert_eq!(decoded_ft4.len(), 1);
        assert_eq!(decoded_ft4[0].message, "CQ K1ABC FN42");
        assert_eq!(decoded_ft4[0].mode, Mode::Ft4);
        assert!((decoded_ft4[0].af_hz - 1500.0).abs() < 1.0);

        let mcfg = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        let pcm = engine.modulate("CQ N0AAA EM10", &mcfg).unwrap();
        assert_eq!(pcm.len(), 6_000 + 151_680);

        let mut cfg_ft8 = DecodeConfig::for_mode(Mode::Ft8);
        cfg_ft8.depth = 1;
        assert!(engine.decode(&[], &cfg_ft8).unwrap().is_empty());

        let decoded = engine.decode(&pcm, &cfg_ft8).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].message, "CQ N0AAA EM10");
        assert!((decoded[0].af_hz - 1500.0).abs() < 1.0);
        assert!(decoded[0].dt_sec.abs() < 0.05);
    }

    #[test]
    fn parallel_determinism_pool_reuse_concurrent_tx_and_fast_cancel() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let engine_synth = DecoderEngine::with_threads(Some(1)).unwrap();

        // Build multi-signal crowded FT8 and FT4 slots cleanly.
        let mut ft8_mix = vec![0.0_f32; ft8::params::NMAX];
        for (idx, (msg, af)) in [
            ("CQ K1ABC FN42", 1500.0),
            ("CQ W9XYZ EN52", 1620.0),
            ("K1ABC W9XYZ -10", 1850.0),
            ("CQ PJ4/N0AAA FK52", 1200.0),
        ]
        .into_iter()
        .enumerate()
        {
            let mut mcfg = ModulateConfig::default_for(Mode::Ft8, af, 12_000);
            mcfg.offset_ms = (idx as u64) * 40;
            let wave = engine_synth.modulate(msg, &mcfg).unwrap();
            let scale = if idx == 0 { 0.35 } else { 0.25 };
            for (dst, &s) in ft8_mix.iter_mut().zip(wave.iter()) {
                *dst += s * scale;
            }
        }

        let mut ft4_mix = vec![0.0_f32; ft4::params::NMAX];
        for (idx, (msg, af)) in [
            ("CQ K1ABC FN42", 1500.0),
            ("CQ W9XYZ EN52", 1740.0),
            ("K1ABC W9XYZ RR73", 1310.0),
        ]
        .into_iter()
        .enumerate()
        {
            let mut mcfg = ModulateConfig::default_for(Mode::Ft4, af, 12_000);
            mcfg.offset_ms = (idx as u64) * 20;
            let wave = engine_synth.modulate(msg, &mcfg).unwrap();
            for (dst, &s) in ft4_mix.iter_mut().zip(wave.iter()) {
                *dst += s * 0.25;
            }
        }

        // 1. Determinism across thread counts 1, 2, 4, 8 (with telemetry enabled!)
        let mut ref_ft8 = None;
        let mut ref_ft4 = None;
        for &threads in &[1usize, 2, 4, 8] {
            let eng = DecoderEngine::with_threads(Some(threads)).unwrap();
            let mut cfg8 = DecodeConfig::for_mode(Mode::Ft8);
            cfg8.depth = 3;
            cfg8.enable_telemetry = true;
            let d8 = eng.decode(&ft8_mix, &cfg8).unwrap();
            assert_eq!(d8.len(), 4, "threads={threads} ft8={d8:?}");
            match &ref_ft8 {
                None => ref_ft8 = Some(d8),
                Some(prev) => assert_eq!(prev, &d8, "FT8 determinism mismatch for threads={threads}"),
            }

            let mut cfg4 = DecodeConfig::for_mode(Mode::Ft4);
            cfg4.depth = 3;
            cfg4.enable_telemetry = true;
            let d4 = eng.decode(&ft4_mix, &cfg4).unwrap();
            assert_eq!(d4.len(), 3, "threads={threads} ft4={d4:?}");
            match &ref_ft4 {
                None => ref_ft4 = Some(d4),
                Some(prev) => assert_eq!(prev, &d4, "FT4 determinism mismatch for threads={threads}"),
            }
        }

        // 2. Steady-state scratch pool reuse across 8 consecutive slots (0 pool growth)
        let eng4 = Arc::new(DecoderEngine::with_threads(Some(2)).unwrap());
        assert_eq!(eng4.pass_pool_created(), 1);
        assert_eq!(eng4.cand_pool_created(), 2);
        let cfg8_notelem = DecodeConfig::for_mode(Mode::Ft8);
        for _ in 0..4 {
            let _ = eng4.decode(&ft8_mix, &cfg8_notelem).unwrap();
            let _ = eng4.decode(&ft4_mix, &DecodeConfig::for_mode(Mode::Ft4)).unwrap();
        }
        assert_eq!(eng4.pass_pool_created(), 1);
        assert_eq!(eng4.cand_pool_created(), 2);

        // 3. Concurrent TX modulate + RX decode sharing Arc<DecoderEngine>, plus <50ms cancel response
        let mut noisy = vec![0.0_f32; ft8::params::NMAX];
        let mut rng: u64 = 0xDEAD_BEEF_CAFE_BABE;
        for s in &mut noisy {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            *s = (((rng >> 33) as f32) / (u32::MAX as f32) - 0.5) * 0.4;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let eng_rx = Arc::clone(&eng4);
        let cancel_rx = Arc::clone(&cancel);
        let rx_handle = std::thread::spawn(move || {
            let cfg = DecodeConfig::for_mode(Mode::Ft8);
            eng_rx.decode_streaming(&noisy, &cfg, &cancel_rx, |_| {})
        });

        std::thread::sleep(Duration::from_millis(3));
        let tx_t0 = Instant::now();
        let tx_wave = eng4
            .modulate(
                "CQ N0AAA EM10",
                &ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000),
            )
            .unwrap();
        assert!(!tx_wave.is_empty());
        assert!(tx_t0.elapsed() < Duration::from_secs(1));

        let c_t0 = Instant::now();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let rx_res = rx_handle.join().unwrap();
        let c_elapsed = c_t0.elapsed();
        assert!(matches!(rx_res, Err(FtxrError::Cancelled)));
        let cancel_limit = if cfg!(debug_assertions) {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(50)
        };
        assert!(
            c_elapsed < cancel_limit,
            "cancel took {c_elapsed:?} (limit {cancel_limit:?})"
        );
    }
}
