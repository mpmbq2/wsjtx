// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Top-level FT8 multi-pass receive pipeline (`wsjtx/lib/ft8_decode.f90`).

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
use crate::dsp::fft::Ft8FftPlans;
use crate::protocol::hashtable::{sanitize_hash_call, CallsignHashTable};
use crate::protocol::types::UnpackContext;
use crate::{ApTag, DecodeConfig, DecodeTelemetry, DecodedMessage, FtxrError, Mode};

use super::ft8b::{ft8b_decode_candidate, Ft8bScratch};
use super::params::{NH1, NMAX};
use super::subtract::{Ft8Subtractor, SubtractScratch};
use super::sync8::{sync8, BitMetricMode, Sync8Scratch};
use super::sync8d::Sync8dTables;

const MAX_CAND: usize = 1000;
const MAX_DECODES_PER_SLOT: usize = 200;

/// Advanced internal/oracle decoding options for `Ft8Decoder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ft8DecodeOptions {
    /// Maximum input samples consumed (`NMAX = 180_000` by default; `172_800` matches
    /// classic `jt9 -8` disk-WAV truncation for intermediate array parity tests).
    pub max_input_samples: usize,
    /// Add +2.0 s EME delay offset to reported `dt_sec`.
    pub eme_delay: bool,
}

impl Default for Ft8DecodeOptions {
    fn default() -> Self {
        Self {
            max_input_samples: NMAX,
            eme_delay: false,
        }
    }
}

/// Reusable, pre-allocated classic FT8 decoder owning all FFT plans and scratch buffers.
pub struct Ft8Decoder {
    plans: Ft8FftPlans,
    downsampler: Ft8Downsampler,
    sync8d: Sync8dTables,
    subtractor: Ft8Subtractor,
    dd: Box<[f32; NMAX]>,
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

        Self {
            plans,
            downsampler,
            sync8d,
            subtractor,
            dd,
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
        let limit = opts.max_input_samples.min(NMAX);
        let copy_len = samples_12k.len().min(limit);
        for (dst, &src) in self.dd[..copy_len].iter_mut().zip(samples_12k[..copy_len].iter()) {
            *dst = f32::from(src);
        }
        self.dd[copy_len..].fill(0.0);
        self.decode_dd_buffer(config, hash_table, cancel, opts, &mut on_decode)
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
        let limit = opts.max_input_samples.min(NMAX);
        let copy_len = samples_12k.len().min(limit);
        for (dst, &src) in self.dd[..copy_len].iter_mut().zip(samples_12k[..copy_len].iter()) {
            *dst = if src.is_finite() { src * 32_768.0 } else { 0.0 };
        }
        self.dd[copy_len..].fill(0.0);
        self.decode_dd_buffer(config, hash_table, cancel, opts, &mut on_decode)
    }

    fn decode_dd_buffer(
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
        let npass = if depth == 1 { 2 } else { 3 };
        let syncmin = if depth <= 2 { 2.1_f32 } else { 1.3_f32 };

        let (unpack_ctx, my_call_clean, ncontest) = match &config.qso {
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
                (ctx, my_clean, qso.contest as u8)
            }
            None => (UnpackContext::default(), None, 0),
        };

        let mut seen_messages: HashSet<String> = HashSet::new();
        let mut results: Vec<DecodedMessage> = Vec::new();
        let mut ndecodes = 0usize;

        for ipass in 1..=npass {
            if cancel.load(Ordering::Relaxed) {
                return Err(FtxrError::Cancelled);
            }
            if ipass == 3 && ndecodes == 0 {
                continue;
            }

            let metric_mode = if ipass == 1 {
                BitMetricMode::Amplitude
            } else {
                BitMetricMode::Power
            };

            let cands = sync8(
                &self.dd,
                config.freq_min_hz,
                config.freq_max_hz,
                syncmin,
                config.rx_freq_hz,
                MAX_CAND,
                &self.plans,
                &mut self.sync8_scratch,
            );
            if cands.is_empty() {
                continue;
            }

            self.downsampler.compute_snapshot(
                &self.dd,
                &self.plans,
                &mut self.snap_scratch,
                &mut self.snapshot,
            );

            for cand in cands {
                if cancel.load(Ordering::Relaxed) {
                    return Err(FtxrError::Cancelled);
                }

                let bin = ((cand.freq_hz / 3.125).round() as usize).clamp(1, NH1);
                let xbase = 10.0_f32.powf(0.1 * (self.sync8_scratch.sbase_db()[bin] - 40.0));

                let Some(outcome) = ft8b_decode_candidate(
                    &self.snapshot,
                    &cand,
                    xbase,
                    depth,
                    metric_mode,
                    false,
                    ncontest,
                    &self.plans,
                    &self.downsampler,
                    &self.sync8d,
                    &unpack_ctx,
                    hash_table,
                    &mut self.ft8b_scratch,
                ) else {
                    continue;
                };

                if seen_messages.contains(&outcome.message) {
                    continue;
                }
                if ndecodes >= MAX_DECODES_PER_SLOT {
                    continue;
                }

                ndecodes += 1;
                seen_messages.insert(outcome.message.clone());
                hash_table.apply_updates(&outcome.pending_hashes, my_call_clean.as_deref());

                self.subtractor.subtract(
                    &mut self.dd,
                    &outcome.itone,
                    outcome.freq_hz,
                    outcome.dt_raw_sec,
                    false,
                    &self.plans,
                    &mut self.sub_scratch,
                );

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
                        freq_drift_hz: None,
                        qsb_envelope: None,
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
                    ap_tag: ApTag::None,
                    telemetry,
                };
                on_decode(decoded.clone());
                results.push(decoded);
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulate::modulate_slot_pcm;
    use crate::ModulateConfig;

    #[test]
    fn scale_invariance_holds_across_100x_amplitude_range() {
        let mut engine = crate::DecoderEngine::new();
        let mcfg = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let (slot, _) = modulate_slot_pcm("CQ N0AAA EM10", &mcfg, NMAX, &ctx, &ht).unwrap();

        // Add tiny deterministic noise floor so xbase is finite and non-degenerate.
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
}
