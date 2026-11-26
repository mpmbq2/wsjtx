// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Top-level FT8 multi-pass and 3-stage receive pipeline (`wsjtx/lib/ft8_decode.f90`).

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
use crate::dsp::fft::Ft8FftPlans;
use crate::dsp::pool::{
    run_ordered_candidates, CandidateScratchPool, PassScratchPool, PrecomputedTables,
};
use crate::protocol::hashtable::{sanitize_hash_call, CallsignHashTable};
use crate::protocol::types::{UnpackContext, UnpackMode};
use crate::protocol::unpack77::unpack77_readonly;
use crate::{DecodeConfig, DecodeTelemetry, DecodedMessage, FtxrError, Mode};

use super::ap::{compute_ap_seed, ApSeed};
use super::ft8b::{ft8b_decode_candidate_with_options, Ft8bOutcome, Ft8bScratch};
use super::params::{NH1, NMAX, NN};
use super::subtract::{Ft8Subtractor, SubtractScratch};
use super::sync8::{sync8, BitMetricMode, Ft8Candidate, Sync8Scratch};
use super::sync8d::Sync8dTables;

const MAX_CAND: usize = 1000;
const MAX_DECODES_PER_SLOT: usize = 200;
const KSTEP: usize = 3456;
pub const NZHSYM_41_SAMPLES: usize = 41 * KSTEP; // 141,696 (11.808 s)
pub const NZHSYM_47_SAMPLES: usize = 47 * KSTEP; // 162,432 (13.536 s)
pub const NZHSYM_50_SAMPLES: usize = 50 * KSTEP; // 172,800 (14.400 s)

/// Controls whether `Ft8Decoder` runs a single complete-buffer pass sequence (`SingleShot`)
/// or mirrors classic `jt9 -8` disk-WAV 3-stage execution (`nzhsym = 41 → 47 → 50`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ft8StagedMode {
    /// Decodes the provided buffer once with `nzhsym = 50` (AP active if enabled).
    #[default]
    SingleShot,
    /// Mirrors `jt9 -8` disk-WAV 3-stage execution (`nzhsym = 41 → 47 → 50`) with early
    /// subtraction and `lrefinedt` DT refinement.
    WsjtxThreeStage,
}

/// Advanced internal/oracle decoding options for `Ft8Decoder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ft8DecodeOptions {
    /// Maximum input samples consumed (`NMAX = 180_000` by default; `172_800` matches
    /// classic `jt9 -8` disk-WAV truncation).
    pub max_input_samples: usize,
    /// Add +2.0 s EME delay offset to reported `dt_sec`.
    pub eme_delay: bool,
    /// Execution mode (`SingleShot` vs `WsjtxThreeStage`).
    pub staged_mode: Ft8StagedMode,
    /// Optional DT-refined subtraction (`lrefinedt = true`) inside `SingleShot` multi-pass
    /// subtraction when `depth >= 3`. Default `false` matches `ft8b.f90:462`.
    pub refine_dt_subtract: bool,
}

impl Default for Ft8DecodeOptions {
    fn default() -> Self {
        Self {
            max_input_samples: NMAX,
            eme_delay: false,
            staged_mode: Ft8StagedMode::SingleShot,
            refine_dt_subtract: false,
        }
    }
}

impl Ft8DecodeOptions {
    /// Preset matching classic `jt9 -8` disk-WAV execution (`nzhsym = 41 → 47 → 50`, `172,800` cap).
    pub fn wsjtx_three_stage() -> Self {
        Self {
            max_input_samples: NZHSYM_50_SAMPLES,
            eme_delay: false,
            staged_mode: Ft8StagedMode::WsjtxThreeStage,
            refine_dt_subtract: false,
        }
    }
}

#[derive(Debug, Clone)]
struct EarlyDecodeRecord {
    itone: [u8; NN],
    freq_hz: f32,
    dt_raw_sec: f32,
    subtracted: bool,
}

/// Incremental multi-stage FT8 slot state supporting WSJT-X `nzhsym = 41 / 47 / 50`
/// and streaming `digi-dx` pass cadence (`11.8 s / 13.5 s / 14.8 s`).
#[derive(Debug, Clone)]
pub struct Ft8SlotSession {
    dd1: Box<[f32; NMAX]>,
    seen_messages: HashSet<String>,
    early_decodes: Vec<EarlyDecodeRecord>,
    ndec_early: usize,
    ndecodes: usize,
    all_decoded: Vec<DecodedMessage>,
}

impl Default for Ft8SlotSession {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft8SlotSession {
    pub fn new() -> Self {
        Self {
            dd1: vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap(),
            seen_messages: HashSet::new(),
            early_decodes: Vec::new(),
            ndec_early: 0,
            ndecodes: 0,
            all_decoded: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.dd1.fill(0.0);
        self.seen_messages.clear();
        self.early_decodes.clear();
        self.ndec_early = 0;
        self.ndecodes = 0;
        self.all_decoded.clear();
    }

    pub fn all_decoded(&self) -> &[DecodedMessage] {
        &self.all_decoded
    }

    /// Executes one stage (`nzhsym = 41`, `47`, or `50`) over `i16` 12 kHz PCM,
    /// returning newly discovered decodes from this stage.
    #[allow(clippy::too_many_arguments)]
    pub fn step_i16<F>(
        &mut self,
        decoder: &mut Ft8Decoder,
        nzhsym: u8,
        samples_12k: &[i16],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        mut on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        let limit = opts.max_input_samples.min(NMAX);
        let stage_limit = match nzhsym {
            41 => NZHSYM_41_SAMPLES.min(limit),
            47 => NZHSYM_47_SAMPLES.min(limit),
            _ => limit,
        };
        let copy_len = samples_12k.len().min(stage_limit);
        for (dst, &src) in decoder.dd[..copy_len]
            .iter_mut()
            .zip(samples_12k[..copy_len].iter())
        {
            *dst = f32::from(src);
        }
        decoder.dd[copy_len..].fill(0.0);
        self.run_stage_with_dd(decoder, nzhsym, config, hash_table, cancel, opts, &mut on_decode)
    }

    /// Executes one stage (`nzhsym = 41`, `47`, or `50`) over normalized `f32` 12 kHz PCM.
    #[allow(clippy::too_many_arguments)]
    pub fn step_f32<F>(
        &mut self,
        decoder: &mut Ft8Decoder,
        nzhsym: u8,
        samples_12k: &[f32],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        mut on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        let limit = opts.max_input_samples.min(NMAX);
        let stage_limit = match nzhsym {
            41 => NZHSYM_41_SAMPLES.min(limit),
            47 => NZHSYM_47_SAMPLES.min(limit),
            _ => limit,
        };
        let copy_len = samples_12k.len().min(stage_limit);
        for (dst, &src) in decoder.dd[..copy_len]
            .iter_mut()
            .zip(samples_12k[..copy_len].iter())
        {
            *dst = if src.is_finite() { src * 32_768.0 } else { 0.0 };
        }
        decoder.dd[copy_len..].fill(0.0);
        self.run_stage_with_dd(decoder, nzhsym, config, hash_table, cancel, opts, &mut on_decode)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_stage_with_dd(
        &mut self,
        decoder: &mut Ft8Decoder,
        nzhsym: u8,
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        if config.mode != Mode::Ft8 {
            return Err(FtxrError::InvalidConfig(format!(
                "Ft8Decoder received mode {:?}",
                config.mode
            )));
        }

        let depth = config.depth.clamp(1, 3);
        if depth == 1 && nzhsym < 50 {
            self.ndec_early = 0;
            return Ok(Vec::new());
        }

        if nzhsym < 47 {
            self.dd1.copy_from_slice(&*decoder.dd);
        }

        if nzhsym == 41 {
            self.seen_messages.clear();
            self.early_decodes.clear();
            self.ndec_early = 0;
            self.ndecodes = 0;
            self.all_decoded.clear();
        } else {
            self.ndecodes = self.ndec_early;
        }

        if nzhsym == 47 {
            if self.ndec_early == 0 {
                self.dd1.copy_from_slice(&*decoder.dd);
                return Ok(Vec::new());
            }
            let refine = depth >= 3;
            for rec in self.early_decodes.iter_mut().take(self.ndec_early) {
                rec.subtracted = false;
                if rec.dt_raw_sec - 0.5 < 0.396 {
                    decoder.subtractor.subtract(
                        &mut decoder.dd,
                        &rec.itone,
                        rec.freq_hz,
                        rec.dt_raw_sec,
                        refine,
                        &decoder.plans,
                        &mut decoder.sub_scratch,
                    );
                    rec.subtracted = true;
                }
            }
            self.dd1.copy_from_slice(&*decoder.dd);
            return Ok(Vec::new());
        }

        if nzhsym == 50 && self.ndec_early > 0 {
            let n = NZHSYM_47_SAMPLES.min(NMAX);
            decoder.dd[..n].copy_from_slice(&self.dd1[..n]);
            for rec in self.early_decodes.iter().take(self.ndec_early) {
                if rec.subtracted {
                    continue;
                }
                decoder.subtractor.subtract(
                    &mut decoder.dd,
                    &rec.itone,
                    rec.freq_hz,
                    rec.dt_raw_sec,
                    true,
                    &decoder.plans,
                    &mut decoder.sub_scratch,
                );
            }
        }

        let stage_decodes = decoder.decode_passes(
            nzhsym,
            config,
            hash_table,
            cancel,
            opts,
            &mut self.seen_messages,
            &mut self.early_decodes,
            &mut self.ndecodes,
            on_decode,
        )?;
        self.all_decoded.extend(stage_decodes.iter().cloned());

        if nzhsym < 50 {
            self.ndec_early = self.ndecodes;
        } else {
            self.ndec_early = 0;
        }

        Ok(stage_decodes)
    }
}

/// Reusable, pre-allocated classic FT8 decoder owning all FFT plans and scratch buffers.
pub struct Ft8Decoder {
    plans: Ft8FftPlans,
    downsampler: Ft8Downsampler,
    sync8d: Sync8dTables,
    subtractor: Ft8Subtractor,
    dd: Box<[f32; NMAX]>,
    dd_raw: Box<[f32; NMAX]>,
    sync8_scratch: Sync8Scratch,
    snap_scratch: DownsampleSnapshotScratch,
    snapshot: Ft8SpectrumSnapshot,
    ft8b_scratch: Ft8bScratch,
    sub_scratch: SubtractScratch,
}

impl fmt::Debug for Ft8Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ft8Decoder")
            .field("plans", &self.plans)
            .finish_non_exhaustive()
    }
}

impl Default for Ft8Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft8Decoder {
    pub fn new() -> Self {
        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let sync8d = Sync8dTables::new();
        let subtractor = Ft8Subtractor::new(&plans);
        let sync8_scratch = Sync8Scratch::new(&plans);
        let snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let snapshot = Ft8SpectrumSnapshot::new();
        let ft8b_scratch = Ft8bScratch::new(&plans);
        let sub_scratch = SubtractScratch::new(&plans);
        let dd = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        let dd_raw = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();

        Self {
            plans,
            downsampler,
            sync8d,
            subtractor,
            dd,
            dd_raw,
            sync8_scratch,
            snap_scratch,
            snapshot,
            ft8b_scratch,
            sub_scratch,
        }
    }

    pub fn plans(&self) -> &Ft8FftPlans {
        &self.plans
    }

    /// Primary entry point for `i16` 12 kHz PCM (exact `jt9` scaling: `s as f32`).
    pub fn decode_i16<F>(
        &mut self,
        samples_12k: &[i16],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        mut on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        match opts.staged_mode {
            Ft8StagedMode::SingleShot => {
                let limit = opts.max_input_samples.min(NMAX);
                let copy_len = samples_12k.len().min(limit);
                for (dst, &src) in self.dd[..copy_len]
                    .iter_mut()
                    .zip(samples_12k[..copy_len].iter())
                {
                    *dst = f32::from(src);
                }
                self.dd[copy_len..].fill(0.0);
                self.decode_dd_single_shot(config, hash_table, cancel, opts, &mut on_decode)
            }
            Ft8StagedMode::WsjtxThreeStage => {
                self.decode_three_stage_i16(samples_12k, config, hash_table, cancel, opts, &mut on_decode)
            }
        }
    }

    /// Entry point for normalized `f32` `[-1.0, 1.0]` 12 kHz PCM (scales by `32768.0`).
    pub fn decode_f32<F>(
        &mut self,
        samples_12k: &[f32],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        mut on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        match opts.staged_mode {
            Ft8StagedMode::SingleShot => {
                let limit = opts.max_input_samples.min(NMAX);
                let copy_len = samples_12k.len().min(limit);
                for (dst, &src) in self.dd[..copy_len]
                    .iter_mut()
                    .zip(samples_12k[..copy_len].iter())
                {
                    *dst = if src.is_finite() { src * 32_768.0 } else { 0.0 };
                }
                self.dd[copy_len..].fill(0.0);
                self.decode_dd_single_shot(config, hash_table, cancel, opts, &mut on_decode)
            }
            Ft8StagedMode::WsjtxThreeStage => {
                self.decode_three_stage_f32(samples_12k, config, hash_table, cancel, opts, &mut on_decode)
            }
        }
    }

    fn decode_three_stage_i16(
        &mut self,
        samples_12k: &[i16],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let limit = opts.max_input_samples.min(NMAX);
        let copy_len = samples_12k.len().min(limit);
        for (dst, &src) in self.dd_raw[..copy_len]
            .iter_mut()
            .zip(samples_12k[..copy_len].iter())
        {
            *dst = f32::from(src);
        }
        self.dd_raw[copy_len..].fill(0.0);
        self.decode_three_stage_from_dd_raw(config, hash_table, cancel, opts, on_decode)
    }

    fn decode_three_stage_f32(
        &mut self,
        samples_12k: &[f32],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let limit = opts.max_input_samples.min(NMAX);
        let copy_len = samples_12k.len().min(limit);
        for (dst, &src) in self.dd_raw[..copy_len]
            .iter_mut()
            .zip(samples_12k[..copy_len].iter())
        {
            *dst = if src.is_finite() { src * 32_768.0 } else { 0.0 };
        }
        self.dd_raw[copy_len..].fill(0.0);
        self.decode_three_stage_from_dd_raw(config, hash_table, cancel, opts, on_decode)
    }

    fn decode_three_stage_from_dd_raw(
        &mut self,
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        if config.mode != Mode::Ft8 {
            return Err(FtxrError::InvalidConfig(format!(
                "Ft8Decoder received mode {:?}",
                config.mode
            )));
        }

        let depth = config.depth.clamp(1, 3);
        let mut seen_messages: HashSet<String> = HashSet::new();
        let mut early_decodes: Vec<EarlyDecodeRecord> = Vec::new();
        let mut ndecodes = 0usize;
        let mut results: Vec<DecodedMessage> = Vec::new();

        if depth == 1 {
            self.dd.copy_from_slice(&*self.dd_raw);
            let d = self.decode_passes(
                50,
                config,
                hash_table,
                cancel,
                opts,
                &mut seen_messages,
                &mut early_decodes,
                &mut ndecodes,
                on_decode,
            )?;
            return Ok(d);
        }

        // Stage 1: nzhsym = 41 (first 41*3456 = 141,696 samples, tail zeroed, no AP).
        let n41 = NZHSYM_41_SAMPLES.min(NMAX);
        self.dd[..n41].copy_from_slice(&self.dd_raw[..n41]);
        self.dd[n41..].fill(0.0);
        let d41 = self.decode_passes(
            41,
            config,
            hash_table,
            cancel,
            opts,
            &mut seen_messages,
            &mut early_decodes,
            &mut ndecodes,
            on_decode,
        )?;
        results.extend(d41);
        let ndec_early = ndecodes;

        // Stage 2: nzhsym = 47 (first 47*3456 = 162,432 samples): subtract early decodes with dt < 0.396.
        let n47 = NZHSYM_47_SAMPLES.min(NMAX);
        self.dd[..n47].copy_from_slice(&self.dd_raw[..n47]);
        self.dd[n47..].fill(0.0);
        let refine_47 = depth >= 3;
        for rec in early_decodes.iter_mut().take(ndec_early) {
            if rec.dt_raw_sec - 0.5 < 0.396 {
                self.subtractor.subtract(
                    &mut self.dd,
                    &rec.itone,
                    rec.freq_hz,
                    rec.dt_raw_sec,
                    refine_47,
                    &self.plans,
                    &mut self.sub_scratch,
                );
                rec.subtracted = true;
            }
        }

        // Stage 3: nzhsym = 50: preserve dd1[..n47], append dd_raw[n47..], subtract remaining early decodes
        // with lrefinedt = true, then run 3 passes with AP enabled.
        self.dd[n47..].copy_from_slice(&self.dd_raw[n47..]);
        for rec in early_decodes.iter().take(ndec_early) {
            if rec.subtracted {
                continue;
            }
            self.subtractor.subtract(
                &mut self.dd,
                &rec.itone,
                rec.freq_hz,
                rec.dt_raw_sec,
                true,
                &self.plans,
                &mut self.sub_scratch,
            );
        }

        let d50 = self.decode_passes(
            50,
            config,
            hash_table,
            cancel,
            opts,
            &mut seen_messages,
            &mut early_decodes,
            &mut ndecodes,
            on_decode,
        )?;
        results.extend(d50);

        Ok(results)
    }

    fn decode_dd_single_shot(
        &mut self,
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        if config.mode != Mode::Ft8 {
            return Err(FtxrError::InvalidConfig(format!(
                "Ft8Decoder received mode {:?}",
                config.mode
            )));
        }
        let mut seen_messages = HashSet::new();
        let mut early_decodes = Vec::new();
        let mut ndecodes = 0usize;
        self.decode_passes(
            50,
            config,
            hash_table,
            cancel,
            opts,
            &mut seen_messages,
            &mut early_decodes,
            &mut ndecodes,
            on_decode,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_passes(
        &mut self,
        nzhsym: u8,
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: &AtomicBool,
        opts: Ft8DecodeOptions,
        seen_messages: &mut HashSet<String>,
        early_decodes: &mut Vec<EarlyDecodeRecord>,
        ndecodes: &mut usize,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let mut hash_snapshot = crate::protocol::hashtable::PassHashSnapshot::new();
        let active_samples_12k = self
            .dd
            .iter()
            .rposition(|&x| x != 0.0)
            .map(|i| i + 1)
            .unwrap_or(0);
        decode_ft8_passes_core(
            &self.plans,
            &self.downsampler,
            &self.sync8d,
            &self.subtractor,
            &mut self.dd,
            &mut self.sync8_scratch,
            &mut self.snap_scratch,
            &mut self.snapshot,
            &mut self.sub_scratch,
            &mut hash_snapshot,
            Some(&mut self.ft8b_scratch),
            None,
            nzhsym,
            active_samples_12k,
            config,
            hash_table,
            cancel,
            opts,
            seen_messages,
            early_decodes,
            ndecodes,
            on_decode,
        )
    }
}

/// Shared parallel FT8 RX entry point used by `DecoderEngine` with pooled scratch and `rayon::ThreadPool`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_ft8_shared_f32<F>(
    samples_12k: &[f32],
    config: &DecodeConfig,
    tables: &PrecomputedTables,
    pass_pool: &PassScratchPool,
    cand_pool: &CandidateScratchPool,
    rx_pool: Option<&rayon::ThreadPool>,
    hash_table: &CallsignHashTable,
    cancel: &AtomicBool,
    opts: Ft8DecodeOptions,
    mut on_decode: F,
) -> Result<Vec<DecodedMessage>, FtxrError>
where
    F: FnMut(DecodedMessage),
{
    if cancel.load(Ordering::Relaxed) {
        return Err(FtxrError::Cancelled);
    }
    let mut pass = pass_pool.checkout();
    let active_len = samples_12k.len().min(NMAX);
    match opts.staged_mode {
        Ft8StagedMode::SingleShot => {
            for (dst, &src) in pass.dd[..active_len]
                .iter_mut()
                .zip(samples_12k[..active_len].iter())
            {
                *dst = src * 32768.0;
            }
            pass.dd[active_len..].fill(0.0);
            let mut seen_messages = HashSet::new();
            let mut early_decodes = Vec::new();
            let mut ndecodes = 0usize;
            let p = &mut *pass;
            decode_ft8_passes_core(
                &tables.fft.ft8,
                &tables.ft8_downsampler,
                &tables.sync8d,
                &tables.ft8_subtractor,
                &mut p.dd,
                &mut p.sync8_scratch,
                &mut p.ft8_snap_scratch,
                &mut p.ft8_snapshot,
                &mut p.ft8_sub_scratch,
                &mut p.hash_snapshot,
                None,
                rx_pool.map(|pool| (cand_pool, pool)),
                50,
                active_len,
                config,
                hash_table,
                cancel,
                opts,
                &mut seen_messages,
                &mut early_decodes,
                &mut ndecodes,
                &mut on_decode,
            )
        }
        Ft8StagedMode::WsjtxThreeStage => {
            for (dst, &src) in pass.dd_raw[..active_len]
                .iter_mut()
                .zip(samples_12k[..active_len].iter())
            {
                *dst = src * 32768.0;
            }
            pass.dd_raw[active_len..].fill(0.0);
            decode_ft8_three_stage_from_pass(
                &mut pass,
                active_len,
                config,
                tables,
                cand_pool,
                rx_pool,
                hash_table,
                cancel,
                opts,
                &mut on_decode,
            )
        }
    }
}

/// Shared parallel FT8 RX entry point over `i16` PCM using pooled scratch and `rayon::ThreadPool`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_ft8_shared_i16<F>(
    samples_12k: &[i16],
    config: &DecodeConfig,
    tables: &PrecomputedTables,
    pass_pool: &PassScratchPool,
    cand_pool: &CandidateScratchPool,
    rx_pool: Option<&rayon::ThreadPool>,
    hash_table: &CallsignHashTable,
    cancel: &AtomicBool,
    opts: Ft8DecodeOptions,
    mut on_decode: F,
) -> Result<Vec<DecodedMessage>, FtxrError>
where
    F: FnMut(DecodedMessage),
{
    if cancel.load(Ordering::Relaxed) {
        return Err(FtxrError::Cancelled);
    }
    let mut pass = pass_pool.checkout();
    let active_len = samples_12k.len().min(NMAX);
    match opts.staged_mode {
        Ft8StagedMode::SingleShot => {
            for (dst, &src) in pass.dd[..active_len]
                .iter_mut()
                .zip(samples_12k[..active_len].iter())
            {
                *dst = f32::from(src);
            }
            pass.dd[active_len..].fill(0.0);
            let mut seen_messages = HashSet::new();
            let mut early_decodes = Vec::new();
            let mut ndecodes = 0usize;
            let p = &mut *pass;
            decode_ft8_passes_core(
                &tables.fft.ft8,
                &tables.ft8_downsampler,
                &tables.sync8d,
                &tables.ft8_subtractor,
                &mut p.dd,
                &mut p.sync8_scratch,
                &mut p.ft8_snap_scratch,
                &mut p.ft8_snapshot,
                &mut p.ft8_sub_scratch,
                &mut p.hash_snapshot,
                None,
                rx_pool.map(|pool| (cand_pool, pool)),
                50,
                active_len,
                config,
                hash_table,
                cancel,
                opts,
                &mut seen_messages,
                &mut early_decodes,
                &mut ndecodes,
                &mut on_decode,
            )
        }
        Ft8StagedMode::WsjtxThreeStage => {
            for (dst, &src) in pass.dd_raw[..active_len]
                .iter_mut()
                .zip(samples_12k[..active_len].iter())
            {
                *dst = f32::from(src);
            }
            pass.dd_raw[active_len..].fill(0.0);
            decode_ft8_three_stage_from_pass(
                &mut pass,
                active_len,
                config,
                tables,
                cand_pool,
                rx_pool,
                hash_table,
                cancel,
                opts,
                &mut on_decode,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_ft8_three_stage_from_pass(
    pass: &mut crate::dsp::pool::PassScratch,
    active_len: usize,
    config: &DecodeConfig,
    tables: &PrecomputedTables,
    cand_pool: &CandidateScratchPool,
    rx_pool: Option<&rayon::ThreadPool>,
    hash_table: &CallsignHashTable,
    cancel: &AtomicBool,
    opts: Ft8DecodeOptions,
    on_decode: &mut dyn FnMut(DecodedMessage),
) -> Result<Vec<DecodedMessage>, FtxrError> {
    let depth = config.depth.clamp(1, 3);
    let refine_early = depth >= 3;
    let parallel = rx_pool.map(|pool| (cand_pool, pool));

    let n41 = NZHSYM_41_SAMPLES.min(NMAX);
    pass.dd[..n41].copy_from_slice(&pass.dd_raw[..n41]);
    pass.dd[n41..].fill(0.0);
    let mut seen_messages = HashSet::new();
    let mut early_decodes = Vec::new();
    let mut ndecodes = 0usize;
    let mut all_results = decode_ft8_passes_core(
        &tables.fft.ft8,
        &tables.ft8_downsampler,
        &tables.sync8d,
        &tables.ft8_subtractor,
        &mut pass.dd,
        &mut pass.sync8_scratch,
        &mut pass.ft8_snap_scratch,
        &mut pass.ft8_snapshot,
        &mut pass.ft8_sub_scratch,
        &mut pass.hash_snapshot,
        None,
        parallel,
        41,
        active_len.min(n41),
        config,
        hash_table,
        cancel,
        opts,
        &mut seen_messages,
        &mut early_decodes,
        &mut ndecodes,
        on_decode,
    )?;

    let n47 = NZHSYM_47_SAMPLES.min(NMAX);
    pass.dd[..n47].copy_from_slice(&pass.dd_raw[..n47]);
    pass.dd[n47..].fill(0.0);
    for rec in early_decodes.iter_mut() {
        if cancel.load(Ordering::Relaxed) {
            return Err(FtxrError::Cancelled);
        }
        if rec.dt_raw_sec - 0.5 < 0.396 {
            tables.ft8_subtractor.subtract(
                &mut pass.dd,
                &rec.itone,
                rec.freq_hz,
                rec.dt_raw_sec,
                refine_early,
                &tables.fft.ft8,
                &mut pass.ft8_sub_scratch,
            );
            rec.subtracted = true;
        }
    }

    let ndec_before_47 = early_decodes.len();
    let stage2 = decode_ft8_passes_core(
        &tables.fft.ft8,
        &tables.ft8_downsampler,
        &tables.sync8d,
        &tables.ft8_subtractor,
        &mut pass.dd,
        &mut pass.sync8_scratch,
        &mut pass.ft8_snap_scratch,
        &mut pass.ft8_snapshot,
        &mut pass.ft8_sub_scratch,
        &mut pass.hash_snapshot,
        None,
        parallel,
        47,
        active_len.min(n47),
        config,
        hash_table,
        cancel,
        opts,
        &mut seen_messages,
        &mut early_decodes,
        &mut ndecodes,
        on_decode,
    )?;
    all_results.extend(stage2);

    pass.dd[n47..].copy_from_slice(&pass.dd_raw[n47..]);
    for rec in early_decodes.iter().take(ndec_before_47) {
        if cancel.load(Ordering::Relaxed) {
            return Err(FtxrError::Cancelled);
        }
        if !rec.subtracted {
            tables.ft8_subtractor.subtract(
                &mut pass.dd,
                &rec.itone,
                rec.freq_hz,
                rec.dt_raw_sec,
                refine_early,
                &tables.fft.ft8,
                &mut pass.ft8_sub_scratch,
            );
        }
    }
    for rec in early_decodes.iter().skip(ndec_before_47) {
        if cancel.load(Ordering::Relaxed) {
            return Err(FtxrError::Cancelled);
        }
        tables.ft8_subtractor.subtract(
            &mut pass.dd,
            &rec.itone,
            rec.freq_hz,
            rec.dt_raw_sec,
            refine_early,
            &tables.fft.ft8,
            &mut pass.ft8_sub_scratch,
        );
    }

    let stage3 = decode_ft8_passes_core(
        &tables.fft.ft8,
        &tables.ft8_downsampler,
        &tables.sync8d,
        &tables.ft8_subtractor,
        &mut pass.dd,
        &mut pass.sync8_scratch,
        &mut pass.ft8_snap_scratch,
        &mut pass.ft8_snapshot,
        &mut pass.ft8_sub_scratch,
        &mut pass.hash_snapshot,
        None,
        parallel,
        50,
        active_len,
        config,
        hash_table,
        cancel,
        opts,
        &mut seen_messages,
        &mut early_decodes,
        &mut ndecodes,
        on_decode,
    )?;
    all_results.extend(stage3);
    Ok(all_results)
}

#[allow(clippy::too_many_arguments)]
fn decode_ft8_passes_core(
    plans: &Ft8FftPlans,
    downsampler: &Ft8Downsampler,
    sync8d: &Sync8dTables,
    subtractor: &Ft8Subtractor,
    dd: &mut [f32; NMAX],
    sync8_scratch: &mut Sync8Scratch,
    snap_scratch: &mut DownsampleSnapshotScratch,
    snapshot: &mut Ft8SpectrumSnapshot,
    sub_scratch: &mut SubtractScratch,
    hash_snapshot: &mut crate::protocol::hashtable::PassHashSnapshot,
    mut inline_ft8b: Option<&mut Ft8bScratch>,
    parallel: Option<(&CandidateScratchPool, &rayon::ThreadPool)>,
    nzhsym: u8,
    active_samples_12k: usize,
    config: &DecodeConfig,
    hash_table: &CallsignHashTable,
    cancel: &AtomicBool,
    opts: Ft8DecodeOptions,
    seen_messages: &mut HashSet<String>,
    early_decodes: &mut Vec<EarlyDecodeRecord>,
    ndecodes: &mut usize,
    on_decode: &mut dyn FnMut(DecodedMessage),
) -> Result<Vec<DecodedMessage>, FtxrError> {
    let depth = config.depth.clamp(1, 3);
    let mut npass = if depth == 1 { 2 } else { 3 };
    if config.early_pass && npass > 1 {
        npass = 1;
    }
    let syncmin = if depth <= 2 { 2.1_f32 } else { 1.3_f32 };
    let refine_in_pass = opts.refine_dt_subtract && depth >= 3;

    let effective_rx_freq_hz = match config.qso.as_ref() {
        Some(q) if (q.rx_freq_hz - 1500.0).abs() > 1e-3 => q.rx_freq_hz,
        _ => config.rx_freq_hz,
    };

    let (unpack_ctx, my_call_clean, ncontest, ap_seed) = match &config.qso {
        Some(qso) => {
            let my_clean = qso
                .my_call
                .as_deref()
                .and_then(sanitize_hash_call)
                .filter(|c| c.len() >= 3);
            let ctx = UnpackContext {
                my_call: qso.my_call.clone(),
                dx_call: qso.his_call.clone(),
                dx_base: None,
            };
            let seed = compute_ap_seed(qso, hash_table);
            (ctx, my_clean, qso.contest as u8, seed)
        }
        None => (UnpackContext::default(), None, 0, ApSeed::default()),
    };

    let mut results: Vec<DecodedMessage> = Vec::new();
    let mut tier0: Vec<Ft8Candidate> = Vec::new();
    let mut tier1: Vec<Ft8Candidate> = Vec::new();
    let mut to_subtract: Vec<([u8; NN], f32, f32)> = Vec::new();

    for ipass in 1..=npass {
        if cancel.load(Ordering::Relaxed) {
            return Err(FtxrError::Cancelled);
        }
        if ipass == 3 && *ndecodes == 0 {
            continue;
        }

        let metric_mode = if ipass == 1 {
            BitMetricMode::Amplitude
        } else {
            BitMetricMode::Power
        };

        let cands = sync8(
            dd,
            config.freq_min_hz,
            config.freq_max_hz,
            syncmin,
            effective_rx_freq_hz,
            MAX_CAND,
            plans,
            sync8_scratch,
        );
        if cancel.load(Ordering::Relaxed) {
            return Err(FtxrError::Cancelled);
        }
        if cands.is_empty() {
            continue;
        }

        downsampler.compute_snapshot(dd, plans, snap_scratch, snapshot);
        if cancel.load(Ordering::Relaxed) {
            return Err(FtxrError::Cancelled);
        }

        let sbase_db = sync8_scratch.sbase_db();
        let tier0_tol = config.freq_tol_hz.max(10.0);
        tier0.clear();
        tier1.clear();
        if config.qso_first {
            for cand in cands {
                if (cand.freq_hz - effective_rx_freq_hz).abs() <= tier0_tol {
                    tier0.push(cand);
                } else {
                    tier1.push(cand);
                }
            }
        } else {
            tier1 = cands;
        }

        for tier in [&tier0, &tier1] {
            if tier.is_empty() {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(FtxrError::Cancelled);
            }
            hash_table.snapshot_into(hash_snapshot);
            let hash_snap_ref = &*hash_snapshot;
            to_subtract.clear();

            let mut accept_outcome = |cand: &Ft8Candidate, mut outcome: Ft8bOutcome| {
                // If worker unpacked `<...>` against pre-pass snapshot, re-unpack now that all
                // earlier canonical candidates in this tier have already committed to `hash_table`.
                if outcome.message.contains("<...>") {
                    if let Ok(re_unpacked) =
                        unpack77_readonly(&outcome.message77, UnpackMode::Rx, &unpack_ctx, hash_table)
                    {
                        if !(ncontest == 0
                            && (1..=3).contains(&outcome.i3)
                            && (re_unpacked.text.contains("/R")
                                || re_unpacked.text.starts_with("TU; ")))
                        {
                            outcome.message = re_unpacked.text;
                            outcome.pending_hashes = re_unpacked.pending_hashes;
                        }
                    }
                }

                if seen_messages.contains(&outcome.message) || *ndecodes >= MAX_DECODES_PER_SLOT {
                    return;
                }

                *ndecodes += 1;
                seen_messages.insert(outcome.message.clone());
                hash_table.apply_updates(&outcome.pending_hashes, my_call_clean.as_deref());

                if nzhsym < 50 {
                    early_decodes.push(EarlyDecodeRecord {
                        itone: outcome.itone,
                        freq_hz: outcome.freq_hz,
                        dt_raw_sec: outcome.dt_raw_sec,
                        subtracted: false,
                    });
                }

                to_subtract.push((outcome.itone, outcome.freq_hz, outcome.dt_raw_sec));

                let mut dt_sec = outcome.dt_raw_sec - 0.5;
                if opts.eme_delay {
                    dt_sec += 2.0;
                }
                if dt_sec == 0.0 {
                    dt_sec = 0.0;
                }

                let telemetry = if config.enable_telemetry {
                    Some(DecodeTelemetry {
                        fec_method: outcome.fec_method,
                        sync_score: cand.sync,
                        subtraction_pass: ipass as u8,
                        hard_errors: outcome.nhard_errors.max(0) as u16,
                        osd_distance: outcome.dmin,
                        quality: outcome.qual,
                        freq_drift_hz: outcome.freq_drift_hz,
                        qsb_envelope: outcome.qsb_envelope.clone(),
                    })
                } else {
                    None
                };

                let decoded = DecodedMessage {
                    mode: Mode::Ft8,
                    snr_db: outcome.snr_db.round().clamp(-128.0, 127.0) as i8,
                    snr_raw: outcome.snr_db,
                    dt_sec,
                    af_hz: outcome.freq_hz,
                    message: outcome.message,
                    ap_tag: outcome.ap_tag,
                    telemetry,
                };
                on_decode(decoded.clone());
                results.push(decoded);
            };

            if let Some(pool_pair) = parallel {
                run_ordered_candidates(
                    tier,
                    Some(pool_pair),
                    cancel,
                    |cand, cand_scratch| {
                        let bin = ((cand.freq_hz / 3.125).round() as usize).clamp(1, NH1);
                        let xbase = 10.0_f32.powf(0.1 * (sbase_db[bin] - 40.0));
                        ft8b_decode_candidate_with_options(
                            snapshot,
                            cand,
                            xbase,
                            depth,
                            nzhsym,
                            metric_mode,
                            false,
                            ncontest,
                            config.qso.as_ref(),
                            &ap_seed,
                            plans,
                            downsampler,
                            sync8d,
                            &unpack_ctx,
                            hash_snap_ref,
                            &mut cand_scratch.ft8b,
                            cancel,
                            config.enable_telemetry,
                            active_samples_12k,
                            config.early_pass,
                        )
                    },
                    |idx, outcome| {
                        accept_outcome(&tier[idx], outcome);
                    },
                );
            } else if let Some(ft8b_scratch) = inline_ft8b.as_deref_mut() {
                for cand in tier.iter() {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(FtxrError::Cancelled);
                    }
                    let bin = ((cand.freq_hz / 3.125).round() as usize).clamp(1, NH1);
                    let xbase = 10.0_f32.powf(0.1 * (sbase_db[bin] - 40.0));
                    if let Some(outcome) = ft8b_decode_candidate_with_options(
                        snapshot,
                        cand,
                        xbase,
                        depth,
                        nzhsym,
                        metric_mode,
                        false,
                        ncontest,
                        config.qso.as_ref(),
                        &ap_seed,
                        plans,
                        downsampler,
                        sync8d,
                        &unpack_ctx,
                        hash_snap_ref,
                        ft8b_scratch,
                        cancel,
                        config.enable_telemetry,
                        active_samples_12k,
                        config.early_pass,
                    ) {
                        accept_outcome(cand, outcome);
                    }
                }
            }

            if cancel.load(Ordering::Relaxed) {
                return Err(FtxrError::Cancelled);
            }

            for (itone, freq_hz, dt_raw_sec) in to_subtract.drain(..) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(FtxrError::Cancelled);
                }
                subtractor.subtract(
                    dd,
                    &itone,
                    freq_hz,
                    dt_raw_sec,
                    refine_in_pass,
                    plans,
                    sub_scratch,
                );
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulate::modulate_slot_pcm;
    use crate::{ApTag, ModulateConfig, QsoContext};

    #[test]
    fn scale_invariance_holds_across_100x_amplitude_range() {
        let engine = crate::DecoderEngine::new();
        let mcfg = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let (slot, _) = modulate_slot_pcm("CQ N0AAA EM10", &mcfg, NMAX, &ctx, &ht).unwrap();

        let mut base = slot.clone();
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for s in &mut base {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let u = ((state >> 32) as f32) / (u32::MAX as f32) - 0.5;
            *s = (*s * 0.25 + u * 0.05).clamp(-1.0, 1.0);
        }

        let mut cfg = DecodeConfig::for_mode(Mode::Ft8);
        cfg.depth = 1;
        cfg.enable_telemetry = true;

        let mut snrs = Vec::new();
        for scale in [0.1_f32, 1.0, 10.0] {
            let scaled: Vec<f32> = base.iter().map(|&x| x * scale).collect();
            let res = engine.decode(&scaled, &cfg).unwrap();
            assert_eq!(res.len(), 1, "failed at scale {scale}");
            assert_eq!(res[0].message, "CQ N0AAA EM10");
            assert!(res[0].telemetry.is_some());
            snrs.push(res[0].snr_raw);
        }
        assert!(
            (snrs[0] - snrs[1]).abs() < 0.1 && (snrs[1] - snrs[2]).abs() < 0.1,
            "SNR not scale-invariant: {snrs:?}"
        );
    }

    #[test]
    fn partial_empty_and_oversized_buffers_do_not_panic() {
        let mut decoder = Ft8Decoder::new();
        let ht = CallsignHashTable::new();
        let cfg = DecodeConfig {
            depth: 1,
            ..DecodeConfig::for_mode(Mode::Ft8)
        };
        let cancel = AtomicBool::new(false);

        let empty_res = decoder
            .decode_i16(&[], &cfg, &ht, &cancel, Ft8DecodeOptions::default(), |_| {})
            .unwrap();
        assert!(empty_res.is_empty());

        let short_res = decoder
            .decode_f32(&[0.0; 12_000], &cfg, &ht, &cancel, Ft8DecodeOptions::default(), |_| {})
            .unwrap();
        assert!(short_res.is_empty());

        let long_res = decoder
            .decode_i16(&vec![0i16; 200_000], &cfg, &ht, &cancel, Ft8DecodeOptions::default(), |_| {})
            .unwrap();
        assert!(long_res.is_empty());
    }

    #[test]
    fn late_tx_5s_and_three_stage_session_decode_cleanly() {
        let mut decoder = Ft8Decoder::new();
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();

        // 5.0 s raw-active late-TX previously failed BP-only (73 erasures); Stage 3 OSD must decode it.
        let mut mcfg_5s = ModulateConfig::raw_active(Mode::Ft8, 1500.0, 12_000);
        mcfg_5s.offset_ms = 5_000;
        let (slot_5s, _) = modulate_slot_pcm("N0AAA K9BBB RR73", &mcfg_5s, NMAX, &ctx, &ht).unwrap();

        let mut cfg = DecodeConfig::for_mode(Mode::Ft8);
        cfg.depth = 3;
        cfg.enable_telemetry = true;
        cfg.qso = Some(QsoContext {
            my_call: Some("N0AAA".into()),
            his_call: Some("K9BBB".into()),
            qso_progress: 3,
            rx_freq_hz: 1500.0,
            tx_freq_hz: 0.0,
            ap_enabled: true,
            ..Default::default()
        });
        let cancel = AtomicBool::new(false);
        let decs = decoder
            .decode_f32(&slot_5s, &cfg, &ht, &cancel, Ft8DecodeOptions::default(), |_| {})
            .unwrap();
        assert_eq!(decs.len(), 1);
        assert_eq!(decs[0].message, "N0AAA K9BBB RR73");

        // Verify WsjtxThreeStage + incremental Ft8SlotSession produce identical decodes.
        let mcfg = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        let (slot_full, _) = modulate_slot_pcm("CQ N0AAA EM10", &mcfg, NMAX, &ctx, &ht).unwrap();
        let res_3stage = decoder
            .decode_f32(
                &slot_full,
                &cfg,
                &ht,
                &cancel,
                Ft8DecodeOptions::wsjtx_three_stage(),
                |_| {},
            )
            .unwrap();
        assert_eq!(res_3stage.len(), 1);
        assert_eq!(res_3stage[0].message, "CQ N0AAA EM10");
        assert_eq!(res_3stage[0].ap_tag, ApTag::None);

        let mut sess = Ft8SlotSession::new();
        let opts = Ft8DecodeOptions::wsjtx_three_stage();
        let s41 = sess
            .step_f32(&mut decoder, 41, &slot_full, &cfg, &ht, &cancel, opts, |_| {})
            .unwrap();
        let s47 = sess
            .step_f32(&mut decoder, 47, &slot_full, &cfg, &ht, &cancel, opts, |_| {})
            .unwrap();
        let s50 = sess
            .step_f32(&mut decoder, 50, &slot_full, &cfg, &ht, &cancel, opts, |_| {})
            .unwrap();
        assert_eq!(s41.len(), 1);
        assert!(s47.is_empty());
        assert!(s50.is_empty());
        assert_eq!(sess.all_decoded().len(), 1);
        assert_eq!(sess.all_decoded()[0].message, "CQ N0AAA EM10");
    }
}
