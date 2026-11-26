// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Single-slot FT4 RX decoder pipeline (`wsjtx/lib/ft4_decode.f90`).

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use num_complex::Complex32;

use crate::dsp::downsample::{Ft4DownsampleScratch, Ft4Downsampler, Ft4PassSpectrum};
use crate::dsp::fft::Ft4FftPlans;
use crate::fec::bp::{LdpcDecoded, LDPC_N};
use crate::fec::decode174_91::decode174_91;
use crate::fec::osd::OsdScratch;
use crate::modulate::ft4::encode_ft4_tones_from_77bits;
use crate::protocol::hashtable::CallsignHashTable;
use crate::protocol::rvec::descramble_ft4;
use crate::protocol::types::{UnpackContext, UnpackMode};
use crate::protocol::unpack77::unpack77_readonly;
use crate::telemetry::DecodeTelemetry;
use crate::{ApTag, DecodeConfig, DecodedMessage, FtxrError, Mode};

use super::ap::{build_ft4_ap_pass, ft4_ap_schedule, Ft4ApSet, Ft4ApTables};
use super::bitmetrics::{
    count_sync_bit_quality, extract_ft4_llrs, get_ft4_bitmetrics, Ft4BitMetricsScratch,
};
use super::candidates::{find_candidates, Ft4Candidate, Ft4CandidatePlan, Ft4CandidateScratch};
use super::params::{
    DT_OFFSET_S, DT_RATE_LITERAL, DT_SEGMENTS, NAPWID_HZ, NBITS, NCW, NDMAX, NFRAME_DOWN, NMAX,
    NN, NSYNC_QUAL_MIN, SAMPLE_RATE_HZ, SCALEFAC, SNR_FLOOR_DB, SNR_OFFSET_DB, SYNC4D_MIN,
};
use super::subtract::{Ft4SubtractScratch, Ft4Subtractor};
use super::sync4d::Ft4SyncTables;

/// Optional inspection hook for capturing FT4 intermediate stage arrays against Fortran dumps.
pub trait Ft4Probe {
    fn on_candidates(
        &mut self,
        _pass: usize,
        _savg: &[f32],
        _savsm_norm: &[f32],
        _sbase: &[f32],
        _candidates: &[Ft4Candidate],
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn on_downsample(
        &mut self,
        _pass: usize,
        _cand_idx: usize,
        _f0_hz: f32,
        _cd2: &[Complex32; NDMAX],
        _f1_hz: f32,
        _smax: f32,
        _ibest: i32,
        _idfbest: i32,
        _cb: &[Complex32; NDMAX],
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn on_bitmetrics(
        &mut self,
        _pass: usize,
        _cand_idx: usize,
        _iseg: usize,
        _nsync: usize,
        _nsync_qual: usize,
        _cd: &[Complex32; NFRAME_DOWN],
        _bitmetrics: &[[f32; 3]; NBITS],
        _llrs: &[[f32; NCW]; 3],
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn on_ldpc_attempt(
        &mut self,
        _pass: usize,
        _cand_idx: usize,
        _ap_pass: usize,
        _iaptype: u8,
        _max_osd: i32,
        _apmask: &[u8; LDPC_N],
        _llr: &[f32; LDPC_N],
        _decoded: Option<&LdpcDecoded>,
    ) {
    }

    fn on_subtract_residual(&mut self, _pass: usize, _cand_idx: usize, _dd: &[f32; NMAX]) {}
}

/// Decoder tuning options for FT4 (`Ft4Decoder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ft4DecodeOptions {
    /// Maximum subtraction passes (`wsjtx` default `3`).
    pub max_subtraction_passes: usize,
}

impl Default for Ft4DecodeOptions {
    fn default() -> Self {
        Self {
            max_subtraction_passes: 3,
        }
    }
}

/// Immutable, shareable precomputed FT4 plans and lookup tables.
#[derive(Clone)]
pub struct Ft4Plans {
    pub fft: Ft4FftPlans,
    pub cand: Ft4CandidatePlan,
    pub down: Ft4Downsampler,
    pub sync: Ft4SyncTables,
    pub ap: Ft4ApTables,
    pub sub: Ft4Subtractor,
}

impl fmt::Debug for Ft4Plans {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ft4Plans")
            .field("fft", &self.fft)
            .finish_non_exhaustive()
    }
}

impl Default for Ft4Plans {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4Plans {
    pub fn new() -> Self {
        let fft = Ft4FftPlans::new();
        let sub = Ft4Subtractor::new(&fft);
        Self {
            fft,
            cand: Ft4CandidatePlan::new(),
            down: Ft4Downsampler::new(),
            sync: Ft4SyncTables::new(),
            ap: Ft4ApTables::new(),
            sub,
        }
    }
}

/// Reusable scratch buffers for `Ft4Decoder` (zero per-slot allocations after construction).
pub struct Ft4Scratch {
    dd: Box<[f32; NMAX]>,
    cand: Ft4CandidateScratch,
    down: Ft4DownsampleScratch,
    pass_spectrum: Ft4PassSpectrum,
    cd0: [Complex32; NDMAX],
    cb: [Complex32; NDMAX],
    cd: [Complex32; NFRAME_DOWN],
    bm: Ft4BitMetricsScratch,
    sub: Ft4SubtractScratch,
    osd: OsdScratch,
    bitmetrics: [[f32; 3]; NBITS],
    pass_llr: [f32; LDPC_N],
    apmask: [u8; LDPC_N],
}

impl Ft4Scratch {
    pub fn new(plans: &Ft4Plans) -> Self {
        Self {
            dd: vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap(),
            cand: Ft4CandidateScratch::new(&plans.fft),
            down: Ft4DownsampleScratch::new(&plans.fft),
            pass_spectrum: Ft4PassSpectrum::new(),
            cd0: [Complex32::ZERO; NDMAX],
            cb: [Complex32::ZERO; NDMAX],
            cd: [Complex32::ZERO; NFRAME_DOWN],
            bm: Ft4BitMetricsScratch::new(&plans.fft),
            sub: Ft4SubtractScratch::new(&plans.fft),
            osd: OsdScratch::new(),
            bitmetrics: [[0.0_f32; 3]; NBITS],
            pass_llr: [0.0_f32; LDPC_N],
            apmask: [0; LDPC_N],
        }
    }
}

/// Pure-Rust single-slot FT4 RX decoder (`wsjtx/lib/ft4_decode.f90`).
pub struct Ft4Decoder {
    plans: Ft4Plans,
    scratch: Ft4Scratch,
}

impl fmt::Debug for Ft4Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ft4Decoder")
            .field("plans", &self.plans)
            .finish_non_exhaustive()
    }
}

impl Default for Ft4Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4Decoder {
    pub fn new() -> Self {
        let plans = Ft4Plans::new();
        let scratch = Ft4Scratch::new(&plans);
        Self { plans, scratch }
    }

    /// Returns shared immutable plans (useful for Stage 5 read-only worker access).
    pub fn plans(&self) -> &Ft4Plans {
        &self.plans
    }

    /// Decodes a normalized 12 kHz `f32` slot buffer (`[-1.0, 1.0]`).
    #[allow(clippy::too_many_arguments)]
    pub fn decode_f32<F>(
        &mut self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: Option<&AtomicBool>,
        options: Ft4DecodeOptions,
        mut on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        self.decode_f32_with_probe(
            samples_12k,
            cfg,
            hash_table,
            cancel,
            options,
            None,
            &mut on_decode,
        )
    }

    /// Decodes a 12 kHz `i16` slot buffer directly without normalized rescaling.
    #[allow(clippy::too_many_arguments)]
    pub fn decode_i16<F>(
        &mut self,
        samples_12k: &[i16],
        cfg: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: Option<&AtomicBool>,
        options: Ft4DecodeOptions,
        mut on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        self.decode_i16_with_probe(
            samples_12k,
            cfg,
            hash_table,
            cancel,
            options,
            None,
            &mut on_decode,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode_f32_with_probe(
        &mut self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: Option<&AtomicBool>,
        options: Ft4DecodeOptions,
        probe: Option<&mut dyn Ft4Probe>,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let n = samples_12k.len().min(NMAX);
        for (dst, &src) in self.scratch.dd[..n].iter_mut().zip(samples_12k[..n].iter()) {
            *dst = if src.is_finite() { src * 32_768.0 } else { 0.0 };
        }
        self.scratch.dd[n..].fill(0.0);
        self.decode_dd_loaded(cfg, hash_table, cancel, options, probe, on_decode)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode_i16_with_probe(
        &mut self,
        samples_12k: &[i16],
        cfg: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: Option<&AtomicBool>,
        options: Ft4DecodeOptions,
        probe: Option<&mut dyn Ft4Probe>,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let n = samples_12k.len().min(NMAX);
        for (dst, &src) in self.scratch.dd[..n].iter_mut().zip(samples_12k[..n].iter()) {
            *dst = f32::from(src);
        }
        self.scratch.dd[n..].fill(0.0);
        self.decode_dd_loaded(cfg, hash_table, cancel, options, probe, on_decode)
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_dd_loaded(
        &mut self,
        cfg: &DecodeConfig,
        hash_table: &CallsignHashTable,
        cancel: Option<&AtomicBool>,
        options: Ft4DecodeOptions,
        mut probe: Option<&mut dyn Ft4Probe>,
        on_decode: &mut dyn FnMut(DecodedMessage),
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                return Err(FtxrError::Cancelled);
            }
        }
        if self.scratch.dd.iter().all(|&v| v == 0.0) {
            return Ok(Vec::new());
        }

        let effective_rx_freq_hz = match cfg.qso.as_ref() {
            Some(q) if (q.rx_freq_hz - 1500.0).abs() > 1e-3 => q.rx_freq_hz,
            _ => cfg.rx_freq_hz,
        };
        let depth = cfg.depth.clamp(1, 3);
        let mut ap_set = Ft4ApSet::from_context(cfg.qso.as_ref(), hash_table);
        ap_set.rx_freq_hz = effective_rx_freq_hz;

        let schedule = ft4_ap_schedule(depth, &ap_set);
        let unpack_ctx = match cfg.qso.as_ref() {
            Some(q) => UnpackContext {
                my_call: q.my_call.clone(),
                dx_call: q.his_call.clone(),
                dx_base: None,
            },
            None => UnpackContext::default(),
        };

        let mut decodes = Vec::new();
        let mut seen_messages: Vec<String> = Vec::new();
        let max_passes = options.max_subtraction_passes.max(1);
        let mut prev_pass_decodes = 0usize;

        for sub_pass in 0..max_passes {
            if sub_pass > 0 && prev_pass_decodes == 0 {
                break;
            }
            if let Some(flag) = cancel {
                if flag.load(Ordering::Relaxed) {
                    return Err(FtxrError::Cancelled);
                }
            }

            let cands = find_candidates(
                &self.scratch.dd,
                cfg.freq_min_hz,
                cfg.freq_max_hz,
                effective_rx_freq_hz,
                &self.plans.fft,
                &self.plans.cand,
                &mut self.scratch.cand,
            );
            if let Some(ref mut p) = probe {
                p.on_candidates(
                    sub_pass,
                    self.scratch.cand.savg(),
                    self.scratch.cand.savsm_normalized(),
                    self.scratch.cand.sbase(),
                    &cands,
                );
            }
            if cands.is_empty() {
                break;
            }

            self.plans.down.compute_pass_spectrum(
                &self.scratch.dd,
                &self.plans.fft,
                &mut self.scratch.down,
                &mut self.scratch.pass_spectrum,
            );

            let mut pass_decodes = 0usize;
            for (cand_idx, cand) in cands.iter().enumerate() {
                if let Some(flag) = cancel {
                    if flag.load(Ordering::Relaxed) {
                        return Err(FtxrError::Cancelled);
                    }
                }

                let outcome = evaluate_ft4_candidate(
                    sub_pass,
                    cand_idx,
                    *cand,
                    effective_rx_freq_hz,
                    depth,
                    &schedule,
                    &ap_set,
                    &unpack_ctx,
                    hash_table,
                    &self.plans,
                    &self.scratch.pass_spectrum,
                    &mut self.scratch.down,
                    &mut self.scratch.cd0,
                    &mut self.scratch.cb,
                    &mut self.scratch.cd,
                    &mut self.scratch.bm,
                    &mut self.scratch.osd,
                    &mut self.scratch.bitmetrics,
                    &mut self.scratch.pass_llr,
                    &mut self.scratch.apmask,
                    &mut probe,
                );

                if let Some(eval) = outcome {
                    let dt_raw_sec = (eval.ibest as f32) / DT_RATE_LITERAL;
                    self.plans.sub.subtract(
                        &mut self.scratch.dd,
                        &eval.tones,
                        eval.f1_hz,
                        dt_raw_sec,
                        &self.plans.fft,
                        &mut self.scratch.sub,
                    );
                    if let Some(ref mut p) = probe {
                        p.on_subtract_residual(sub_pass, cand_idx, &self.scratch.dd);
                    }

                    // Subtraction runs before duplicate suppression (ft4_decode.f90:435-443),
                    // preventing residual harmonic/multipath copies of already-decoded messages.
                    if !seen_messages.contains(&eval.message) {
                        seen_messages.push(eval.message.clone());
                        let snr_linear = cand.snr_ratio - 1.0;
                        let snr_raw = if snr_linear > 0.0 {
                            (10.0 * snr_linear.log10() - SNR_OFFSET_DB).max(SNR_FLOOR_DB)
                        } else {
                            SNR_FLOOR_DB
                        };
                        let snr_db = snr_raw.round().clamp(SNR_FLOOR_DB, 99.0) as i8;
                        let dt_sec = dt_raw_sec - DT_OFFSET_S;
                        let telemetry = if cfg.enable_telemetry {
                            Some(DecodeTelemetry {
                                fec_method: eval.fec_method,
                                sync_score: eval.smax,
                                subtraction_pass: (sub_pass + 1) as u8,
                                hard_errors: eval.nhard_errors.max(0) as u16,
                                osd_distance: eval.dmin,
                                quality: eval.qual,
                                freq_drift_hz: Some(eval.idfbest as f32),
                                qsb_envelope: None,
                            })
                        } else {
                            None
                        };

                        let decoded = DecodedMessage {
                            mode: Mode::Ft4,
                            snr_db,
                            snr_raw,
                            dt_sec,
                            af_hz: eval.f1_hz,
                            message: eval.message,
                            ap_tag: eval.ap_tag,
                            telemetry,
                        };
                        on_decode(decoded.clone());
                        decodes.push(decoded);
                        pass_decodes += 1;
                    }
                }
            }

            prev_pass_decodes = pass_decodes;
        }

        Ok(decodes)
    }
}

struct CandidateDecodeSuccess {
    message: String,
    tones: [u8; NN],
    f1_hz: f32,
    ibest: i32,
    idfbest: i32,
    smax: f32,
    nhard_errors: i32,
    dmin: f32,
    qual: f32,
    ap_tag: ApTag,
    fec_method: crate::telemetry::FecMethod,
}

#[allow(clippy::too_many_arguments)]
fn evaluate_ft4_candidate(
    sub_pass: usize,
    cand_idx: usize,
    cand: Ft4Candidate,
    rx_freq_hz: f32,
    depth: u8,
    schedule: &[super::ap::Ft4PassSpec],
    ap_set: &Ft4ApSet,
    unpack_ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
    plans: &Ft4Plans,
    pass_spectrum: &Ft4PassSpectrum,
    down_scratch: &mut Ft4DownsampleScratch,
    cd0: &mut [Complex32; NDMAX],
    cb: &mut [Complex32; NDMAX],
    cd: &mut [Complex32; NFRAME_DOWN],
    bm_scratch: &mut Ft4BitMetricsScratch,
    osd_scratch: &mut OsdScratch,
    bitmetrics: &mut [[f32; 3]; NBITS],
    pass_llr: &mut [f32; LDPC_N],
    apmask: &mut [u8; LDPC_N],
    probe: &mut Option<&mut dyn Ft4Probe>,
) -> Option<CandidateDecodeSuccess> {
    let f0 = cand.freq_hz;
    plans
        .down
        .downsample_from_spectrum(pass_spectrum, f0, &plans.fft, down_scratch, cd0);
    let sum2: f32 = cd0.iter().map(|c| c.norm_sqr()).sum::<f32>() / (NDMAX as f32);
    if sum2 > 0.0 && sum2.is_finite() {
        let inv_rms = 1.0_f32 / sum2.sqrt();
        for c in cd0.iter_mut() {
            *c *= inv_rms;
        }
    }

    let mut smax1 = 0.0_f32;
    let mut best_cb_sampled = false;

    for (iseg, &(ib_start, ib_end)) in DT_SEGMENTS.iter().enumerate() {
        let mut smax_coarse = -99.0_f32;
        let mut ibest_coarse = ib_start;
        let mut idfbest_coarse = 0i32;

        for idf in (-12..=12).step_by(3) {
            for ib in (ib_start..=ib_end).step_by(4) {
                let s = plans.sync.sync4d(cd0, ib, idf);
                if s > smax_coarse {
                    smax_coarse = s;
                    ibest_coarse = ib;
                    idfbest_coarse = idf;
                }
            }
        }

        let mut smax = -99.0_f32;
        let mut ibest = ibest_coarse;
        let mut idfbest = idfbest_coarse;
        for idf in (idfbest_coarse - 4)..=(idfbest_coarse + 4) {
            for ib in (ibest_coarse - 5)..=(ibest_coarse + 5) {
                let s = plans.sync.sync4d(cd0, ib, idf);
                if s > smax {
                    smax = s;
                    ibest = ib;
                    idfbest = idf;
                }
            }
        }

        if iseg == 0 {
            smax1 = smax;
            if smax < SYNC4D_MIN {
                continue;
            }
        } else if smax < smax1 || smax < SYNC4D_MIN {
            continue;
        }

        let f1_hz = f0 + (idfbest as f32);
        if f1_hz <= 10.0 || f1_hz >= 4990.0 {
            continue;
        }

        plans
            .down
            .downsample_from_spectrum(pass_spectrum, f1_hz, &plans.fft, down_scratch, cb);
        let sum2_cb: f32 = cb.iter().map(|c| c.norm_sqr()).sum::<f32>() / (NFRAME_DOWN as f32);
        if sum2_cb > 0.0 && sum2_cb.is_finite() {
            let inv_rms = 1.0_f32 / sum2_cb.sqrt();
            for c in cb.iter_mut() {
                *c *= inv_rms;
            }
        }

        if !best_cb_sampled || probe.is_some() {
            if let Some(p) = probe.as_deref_mut() {
                p.on_downsample(
                    sub_pass, cand_idx, f0, cd0, f1_hz, smax, ibest, idfbest, cb,
                );
            }
            best_cb_sampled = true;
        }

        slice_frame_cd(cb, ibest, cd);

        let bm_status = get_ft4_bitmetrics(cd, &plans.fft, bm_scratch, bitmetrics);
        if bm_status.badsync {
            continue;
        }
        let nsync_qual = count_sync_bit_quality(bitmetrics);
        if nsync_qual < NSYNC_QUAL_MIN {
            continue;
        }

        let llrs = extract_ft4_llrs(bitmetrics, SCALEFAC);
        if let Some(p) = probe.as_deref_mut() {
            p.on_bitmetrics(
                sub_pass,
                cand_idx,
                iseg,
                bm_status.nsync,
                nsync_qual,
                cd,
                bitmetrics,
                &llrs,
            );
        }

        let maxosd = if depth <= 2 {
            -1
        } else if (rx_freq_hz - f1_hz).abs() <= NAPWID_HZ {
            3
        } else {
            2
        };

        let mut ldpc_won = false;
        let mut success: Option<CandidateDecodeSuccess> = None;

        for (ap_pass_idx, &pass_spec) in schedule.iter().enumerate() {
            if !build_ft4_ap_pass(
                pass_spec,
                &llrs,
                f1_hz,
                &plans.ap,
                ap_set,
                pass_llr,
                apmask,
            ) {
                continue;
            }

            let ldpc_opt = decode174_91(pass_llr, 91, maxosd, 2, apmask, osd_scratch);
            if let Some(p) = probe.as_deref_mut() {
                p.on_ldpc_attempt(
                    sub_pass,
                    cand_idx,
                    ap_pass_idx,
                    pass_spec.ap_type as u8,
                    maxosd,
                    apmask,
                    pass_llr,
                    ldpc_opt.as_ref(),
                );
            }

            if let Some(ldpc_dec) = ldpc_opt {
                let msg77_scrambled: [u8; 77] = ldpc_dec.message91[..77].try_into().unwrap();
                // Reject all-zero codeword BEFORE descrambling (matches ft4_decode.f90:425).
                if msg77_scrambled.iter().all(|&b| b == 0) {
                    continue;
                }
                let msg77 = descramble_ft4(&msg77_scrambled);

                ldpc_won = true;
                match unpack77_readonly(&msg77, UnpackMode::Rx, unpack_ctx, hash_table) {
                    Ok(unpacked) => {
                        hash_table.apply_updates(
                            &unpacked.pending_hashes,
                            unpack_ctx.my_call.as_deref(),
                        );
                        let tones = encode_ft4_tones_from_77bits(&msg77);
                        let qual =
                            1.0_f32 - (ldpc_dec.nhard_errors as f32 + ldpc_dec.dmin) / 60.0;
                        let ap_tag = ApTag::from_wsjtx(pass_spec.ap_type as u8, qual);
                        success = Some(CandidateDecodeSuccess {
                            message: unpacked.text,
                            tones,
                            f1_hz,
                            ibest,
                            idfbest,
                            smax,
                            nhard_errors: ldpc_dec.nhard_errors,
                            dmin: ldpc_dec.dmin,
                            qual,
                            ap_tag,
                            fec_method: ldpc_dec.fec_method,
                        });
                    }
                    Err(_) => {
                        // Valid CRC/LDPC codeword whose message fails unpack exits both loops
                        // without subtraction (`ft4_decode.f90:433,460`).
                        return None;
                    }
                }
                break;
            }
        }

        if ldpc_won {
            return success;
        }
    }

    let _ = SAMPLE_RATE_HZ;
    None
}

/// Exact port of `ft4_decode.f90:289-295` baseband slicing (`cb[0..4032]` -> `cd[0..3296]`),
/// preserving Fortran's `ibest < 0` slice behavior (`cd[-ibest..ibest+3296] = cb[..3296+2*ibest]`)
/// for parity against WSJT-X.
pub fn slice_frame_cd(cb: &[Complex32; NDMAX], ibest: i32, cd: &mut [Complex32; NFRAME_DOWN]) {
    cd.fill(Complex32::ZERO);
    if ibest >= 0 {
        let start = ibest as usize;
        if start < NDMAX {
            let avail = (NDMAX - start).min(NFRAME_DOWN);
            cd[..avail].copy_from_slice(&cb[start..start + avail]);
        }
    } else {
        let offset = (-ibest) as usize;
        let len = (NFRAME_DOWN as i32) + 2 * ibest;
        if len > 0 {
            let len_u = (len as usize).min(NDMAX).min(NFRAME_DOWN.saturating_sub(offset));
            cd[offset..offset + len_u].copy_from_slice(&cb[..len_u]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::pack77::pack77;

    #[test]
    fn ft4_decoder_handles_edge_lengths_and_silence_without_panic() {
        let mut decoder = Ft4Decoder::new();
        let cfg = DecodeConfig::for_mode(Mode::Ft4);
        let ht = CallsignHashTable::new();

        for len in [0usize, 1, 100, 10_000, 72_575, 72_576, 90_000] {
            let buf = vec![0.0_f32; len];
            let res = decoder
                .decode_f32(&buf, &cfg, &ht, None, Ft4DecodeOptions::default(), |_| {})
                .unwrap();
            assert!(res.is_empty(), "silent len={len} produced false decodes");

            let buf_i16 = vec![0_i16; len];
            let res_i16 = decoder
                .decode_i16(
                    &buf_i16,
                    &cfg,
                    &ht,
                    None,
                    Ft4DecodeOptions::default(),
                    |_| {},
                )
                .unwrap();
            assert!(res_i16.is_empty());
        }
    }

    #[test]
    fn ft4_decoder_roundtrips_shifted_and_truncated_slots() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let tones = encode_ft4_tones_from_77bits(&packed.bits);
        let wave = crate::modulate::ft4::gen_ft4_wave_real(&tones, 1500.0, SAMPLE_RATE_HZ);

        let mut decoder = Ft4Decoder::new();
        let mut cfg = DecodeConfig::for_mode(Mode::Ft4);
        cfg.enable_telemetry = true;

        // 1. Nominal 300 ms TX delay inside full 90,000-sample slot & 66,000 truncated slot
        let mut slot_90k = vec![0.0_f32; 90_000];
        let delay_300ms = 3600usize;
        for (i, &w) in wave.iter().enumerate() {
            slot_90k[delay_300ms + i] = w * 0.5;
        }
        for &len in &[90_000usize, NMAX, 66_000usize] {
            let decs = decoder
                .decode_f32(
                    &slot_90k[..len],
                    &cfg,
                    &ht,
                    None,
                    Ft4DecodeOptions::default(),
                    |_| {},
                )
                .unwrap();
            assert_eq!(decs.len(), 1, "failed on len={len}: {decs:?}");
            assert_eq!(decs[0].message, "CQ K1ABC FN42");
            assert_eq!(decs[0].mode, Mode::Ft4);
            assert!(decs[0].telemetry.is_some());
        }

        // 2. Late TX (+1.2 s) inside 90,000-sample slot (trailing end clipped slightly at 72,576)
        let mut slot_late = vec![0.0_f32; 90_000];
        let delay_late = 14_400usize; // 1.2 s
        for (i, &w) in wave.iter().enumerate() {
            slot_late[delay_late + i] = w * 0.5;
        }
        let decs_late = decoder
            .decode_f32(
                &slot_late,
                &cfg,
                &ht,
                None,
                Ft4DecodeOptions::default(),
                |_| {},
            )
            .unwrap();
        assert_eq!(decs_late.len(), 1, "late start failed: {decs_late:?}");
        assert_eq!(decs_late[0].message, "CQ K1ABC FN42");

        // 3. Cancellation check
        let cancel = AtomicBool::new(true);
        let err = decoder.decode_f32(
            &slot_90k,
            &cfg,
            &ht,
            Some(&cancel),
            Ft4DecodeOptions::default(),
            |_| {},
        );
        assert!(matches!(err, Err(FtxrError::Cancelled)));
    }
}
