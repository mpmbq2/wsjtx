// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 symbol FFT, multi-symbol coherent soft LLRs (`bmeta`–`bmete`), AP pass scheduling,
//! and candidate decode (`wsjtx/lib/ft8/ft8b.f90`, explicitly excluding unused NR `bessi0`).

use std::f32::consts::TAU;

use std::sync::atomic::{AtomicBool, Ordering};

use num_complex::Complex32;

use crate::dsp::downsample::{Ft8Downsampler, Ft8SpectrumSnapshot};
use crate::dsp::fft::Ft8FftPlans;
use crate::fec::bp::LDPC_N;
use crate::fec::decode174_91::{decode174_91_with_scratch, FecScratch};
use crate::modulate::ft8::encode_ft8_tones_from_77bits;
use crate::protocol::hashtable::{CallsignHashTable, CallsignLookup};
use crate::protocol::types::{PendingHashUpdates, UnpackContext, UnpackMode, PAYLOAD_BITS};
use crate::protocol::unpack77::unpack77_readonly;
use crate::telemetry::{
    compute_ft8_symbol_envelope, estimate_ft8_costas_drift, FecMethod, SymbolEnvelope,
};
use crate::{ApTag, QsoContext};

use super::ap::{apply_ap_hypothesis, ft8b_pass_count, ft8b_pass_plan, ApSeed, Ft8bPassPlan};
use super::params::{DOWN_NFFT2, GRAYMAP, ICOS7, ND, NDOWN, NN, NP2, SAMPLE_RATE_HZ};
use super::sync8::{BitMetricMode, Ft8Candidate};
use super::sync8d::Sync8dTables;

/// Reusable scratch buffers for `ft8b` candidate decoding (one instance per worker).
pub struct Ft8bScratch {
    pub cd0: [Complex32; DOWN_NFFT2],
    pub ifft_scratch: Vec<Complex32>,
    pub fft32_scratch: Vec<Complex32>,
    pub cs: [[Complex32; 8]; NN],
    pub s8: [[f32; 8]; NN],
    pub s2: [f32; 512],
    pub llrz: [f32; LDPC_N],
    pub apmask: [u8; LDPC_N],
    pub fec_scratch: FecScratch,
}

impl Ft8bScratch {
    pub fn new(plans: &Ft8FftPlans) -> Self {
        Self {
            cd0: [Complex32::ZERO; DOWN_NFFT2],
            ifft_scratch: vec![Complex32::ZERO; plans.c2c_inv_3200.get_inplace_scratch_len()],
            fft32_scratch: vec![Complex32::ZERO; plans.c2c_fwd_32.get_inplace_scratch_len()],
            cs: [[Complex32::ZERO; 8]; NN],
            s8: [[0.0; 8]; NN],
            s2: [0.0; 512],
            llrz: [0.0; LDPC_N],
            apmask: [0u8; LDPC_N],
            fec_scratch: FecScratch::new(),
        }
    }
}

/// Complete multi-symbol coherent bit metrics (`bmeta`–`bmete`, `llra`–`llre`) for Stage 3
/// decoding and intermediate verification.
#[derive(Debug, Clone)]
pub struct Ft8BitMetrics {
    /// Unnormalized 1-symbol metrics.
    pub bmeta: [f32; LDPC_N],
    /// Unnormalized 2-symbol coherent metrics.
    pub bmetb: [f32; LDPC_N],
    /// Unnormalized 3-symbol coherent metrics.
    pub bmetc: [f32; LDPC_N],
    /// Unnormalized bit-by-bit ratio-normalized 1-symbol metrics.
    pub bmetd: [f32; LDPC_N],
    /// Unnormalized signed max-abs across `bmeta`, `bmetb`, `bmetc`.
    pub bmete: [f32; LDPC_N],
    /// Normalized & scaled (`* 2.83`) LLRs: `[llra, llrb, llrc, llrd, llre]`.
    pub llrs: [[f32; LDPC_N]; 5],
}

/// Intermediate fine-sync and soft-demodulation products exposed for Stage 2/3 verification.
#[derive(Debug, Clone)]
pub struct Ft8bIntermediates {
    pub f1_refined_hz: f32,
    pub delf_best_hz: f32,
    pub ibest: isize,
    pub dt_raw_sec: f32,
    pub sync_fine: f32,
    pub sync8d_coarse: Vec<f32>,
    pub nsync_hard: u8,
    pub bmeta: [f32; LDPC_N],
    pub llra: [f32; LDPC_N],
}

/// Valid decoded candidate returned by `ft8b_decode_candidate`.
#[derive(Debug, Clone)]
pub struct Ft8bOutcome {
    pub freq_hz: f32,
    /// Raw DT before `- 0.5` offset (passed directly to `Ft8Subtractor::subtract`).
    pub dt_raw_sec: f32,
    pub sync_fine: f32,
    pub nsync_hard: u8,
    pub snr_db: f32,
    pub nhard_errors: i32,
    pub dmin: f32,
    pub qual: f32,
    pub iaptype: u8,
    pub ap_tag: ApTag,
    pub fec_method: FecMethod,
    pub message: String,
    pub message77: [u8; PAYLOAD_BITS],
    pub i3: u8,
    pub n3: u8,
    pub itone: [u8; NN],
    pub pending_hashes: PendingHashUpdates,
    pub freq_drift_hz: Option<f32>,
    pub qsb_envelope: Option<SymbolEnvelope>,
}

/// Computes unnormalized 1-symbol soft-bit LLRs `bmeta[0..174]` from symbol spectra `cs`.
pub fn compute_1sym_bmeta(cs: &[[Complex32; 8]; NN], metric_mode: BitMetricMode) -> [f32; LDPC_N] {
    compute_1sym_bmet_pair(cs, metric_mode).0
}

/// Computes unnormalized 1-symbol bit-normalized soft-bit LLRs `bmetd[0..174]` (`ft8b.f90:218-225`).
pub fn compute_1sym_bmetd(cs: &[[Complex32; 8]; NN], metric_mode: BitMetricMode) -> [f32; LDPC_N] {
    compute_1sym_bmet_pair(cs, metric_mode).1
}

/// Computes both unnormalized 1-symbol `bmeta` and bit-by-bit ratio-normalized `bmetd`.
pub fn compute_1sym_bmet_pair(
    cs: &[[Complex32; 8]; NN],
    metric_mode: BitMetricMode,
) -> ([f32; LDPC_N], [f32; LDPC_N]) {
    let mut bmeta = [0.0_f32; LDPC_N];
    let mut bmetd = [0.0_f32; LDPC_N];
    let mut s2 = [0.0_f32; 8];

    for ihalf in 0..2 {
        for k in 0..(ND / 2) {
            let ks = if ihalf == 0 { k + 7 } else { k + 43 };
            for (i, slot) in s2.iter_mut().enumerate() {
                let tone = GRAYMAP[i] as usize;
                let mag = cs[ks][tone].norm();
                *slot = match metric_mode {
                    BitMetricMode::Amplitude => mag,
                    BitMetricMode::Power => mag * mag,
                };
            }

            let base_bit = k * 3 + ihalf * 87;
            for ib in 0..3 {
                let bit_pos = 2 - ib;
                let mut m1 = f32::NEG_INFINITY;
                let mut m0 = f32::NEG_INFINITY;
                for (i, &val) in s2.iter().enumerate() {
                    if ((i >> bit_pos) & 1) != 0 {
                        if val > m1 {
                            m1 = val;
                        }
                    } else if val > m0 {
                        m0 = val;
                    }
                }
                let bm = m1 - m0;
                let den = m1.max(m0);
                bmeta[base_bit + ib] = bm;
                bmetd[base_bit + ib] = if den > 0.0 { bm / den } else { 0.0 };
            }
        }
    }

    (bmeta, bmetd)
}

/// Computes all 5 Stage 3 coherent multi-symbol bit-metric sets (`bmeta`–`bmete`, `llra`–`llre`)
/// matching `wsjtx/lib/ft8/ft8b.f90:186-254`, including exact half-boundary Costas symbol
/// overlap (`cs[36]` and `cs[72]`).
pub fn compute_all_bit_metrics(
    cs: &[[Complex32; 8]; NN],
    metric_mode: BitMetricMode,
    s2: &mut [f32; 512],
) -> Ft8BitMetrics {
    let mut bmeta = [0.0_f32; LDPC_N];
    let mut bmetb = [0.0_f32; LDPC_N];
    let mut bmetc = [0.0_f32; LDPC_N];
    let mut bmetd = [0.0_f32; LDPC_N];
    let mut bmete = [0.0_f32; LDPC_N];
    let power = metric_mode == BitMetricMode::Power;

    for nsym in 1..=3 {
        let nt = 1usize << (3 * nsym);
        let ibmax = 3 * nsym - 1;
        for ihalf in 0..2 {
            let mut k = 0usize;
            while k < 29 {
                let ks = if ihalf == 0 { k + 7 } else { k + 43 };
                match nsym {
                    1 => {
                        for i3 in 0..8 {
                            let mag = cs[ks][GRAYMAP[i3] as usize].norm();
                            s2[i3] = if power { mag * mag } else { mag };
                        }
                    }
                    2 => {
                        for i2 in 0..8 {
                            let z1 = cs[ks][GRAYMAP[i2] as usize];
                            let row = i2 << 3;
                            for i3 in 0..8 {
                                let mag = (z1 + cs[ks + 1][GRAYMAP[i3] as usize]).norm();
                                s2[row | i3] = if power { mag * mag } else { mag };
                            }
                        }
                    }
                    _ => {
                        for i1 in 0..8 {
                            let z1 = cs[ks][GRAYMAP[i1] as usize];
                            let blk = i1 << 6;
                            for i2 in 0..8 {
                                let z12 = z1 + cs[ks + 1][GRAYMAP[i2] as usize];
                                let row = blk | (i2 << 3);
                                for i3 in 0..8 {
                                    let mag = (z12 + cs[ks + 2][GRAYMAP[i3] as usize]).norm();
                                    s2[row | i3] = if power { mag * mag } else { mag };
                                }
                            }
                        }
                    }
                }

                let i32_base = k * 3 + ihalf * 87;
                for ib in 0..=ibmax {
                    let bit_idx = i32_base + ib;
                    if bit_idx >= LDPC_N {
                        continue;
                    }
                    let bit_pos = ibmax - ib;
                    let mut m1 = f32::NEG_INFINITY;
                    let mut m0 = f32::NEG_INFINITY;
                    for (i, &val) in s2[..nt].iter().enumerate() {
                        if ((i >> bit_pos) & 1) != 0 {
                            if val > m1 {
                                m1 = val;
                            }
                        } else if val > m0 {
                            m0 = val;
                        }
                    }
                    let bm = m1 - m0;
                    match nsym {
                        1 => {
                            bmeta[bit_idx] = bm;
                            let den = m1.max(m0);
                            bmetd[bit_idx] = if den > 0.0 { bm / den } else { 0.0 };
                        }
                        2 => bmetb[bit_idx] = bm,
                        _ => bmetc[bit_idx] = bm,
                    }
                }

                k += nsym;
            }
        }
    }

    for i in 0..LDPC_N {
        let candidates = [bmeta[i], bmetb[i], bmetc[i]];
        let mut best_val = candidates[0];
        let mut best_abs = best_val.abs();
        for &v in &candidates[1..] {
            let av = v.abs();
            if av > best_abs {
                best_abs = av;
                best_val = v;
            }
        }
        bmete[i] = best_val;
    }

    const SCALEFAC: f32 = 2.83;
    let raw_sets = [bmeta, bmetb, bmetc, bmetd, bmete];
    let mut llrs = [[0.0_f32; LDPC_N]; 5];
    for (dst, mut src) in llrs.iter_mut().zip(raw_sets) {
        normalize_bmet(&mut src);
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            *d = SCALEFAC * s;
        }
    }

    Ft8BitMetrics {
        bmeta,
        bmetb,
        bmetc,
        bmetd,
        bmete,
        llrs,
    }
}

/// Port of `normalizebmet` (`wsjtx/lib/ft8/ft8b.f90:493-506`) with zero-variance guard.
pub fn normalize_bmet(bmet: &mut [f32; LDPC_N]) {
    let n = LDPC_N as f32;
    let bmetav: f32 = bmet.iter().sum::<f32>() / n;
    let bmet2av: f32 = bmet.iter().map(|&x| x * x).sum::<f32>() / n;
    let var = bmet2av - bmetav * bmetav;
    let bmetsig = if var > 0.0 {
        var.sqrt()
    } else {
        bmet2av.sqrt()
    };
    if bmetsig > 0.0 && bmetsig.is_finite() {
        for x in bmet.iter_mut() {
            *x /= bmetsig;
        }
    }
}

/// Runs downsampling, fine sync, 32-pt symbol FFTs, and 1-symbol LLR extraction for one
/// candidate without mutating `snapshot`. Returns `None` if `nsync_hard <= syncmin`.
#[allow(clippy::too_many_arguments)]
pub fn ft8b_extract_stage2_intermediates(
    snapshot: &Ft8SpectrumSnapshot,
    cand: &Ft8Candidate,
    depth: u8,
    metric_mode: BitMetricMode,
    plans: &Ft8FftPlans,
    downsampler: &Ft8Downsampler,
    sync8d_tables: &Sync8dTables,
    scratch: &mut Ft8bScratch,
) -> Option<Ft8bIntermediates> {
    let fs2 = (SAMPLE_RATE_HZ as f32) / (NDOWN as f32); // 200.0 Hz
    let dt2 = 1.0_f32 / fs2; // 0.005 s

    let mut f1 = cand.freq_hz;
    downsampler.downsample_from_snapshot(
        snapshot,
        f1,
        plans,
        &mut scratch.ifft_scratch,
        &mut scratch.cd0,
    );

    let i0 = ((cand.dt_sec + 0.5) * fs2).round() as isize;
    let mut smax = 0.0_f32;
    let mut ibest = 0isize;
    let mut sync8d_coarse = Vec::with_capacity(21);
    for idt in (i0 - 10)..=(i0 + 10) {
        let sync = sync8d_tables.sync8d(&scratch.cd0, idt, None);
        sync8d_coarse.push(sync);
        if sync > smax {
            smax = sync;
            ibest = idt;
        }
    }

    smax = 0.0;
    let mut delfbest = 0.0_f32;
    let mut ctwk = [Complex32::ZERO; 32];
    for ifr in -5..=5 {
        let delf = (ifr as f32) * 0.5;
        let dphi = TAU * delf * dt2;
        let mut phi = 0.0_f32;
        for cell in &mut ctwk {
            *cell = Complex32::new(phi.cos(), phi.sin());
            phi = (phi + dphi) % TAU;
        }
        let sync = sync8d_tables.sync8d(&scratch.cd0, ibest, Some(&ctwk));
        if sync > smax {
            smax = sync;
            delfbest = delf;
        }
    }
    f1 += delfbest;

    // `ft8b.f90:141` immediately re-downsamples at `f1 + delfbest` from the immutable snapshot.
    downsampler.downsample_from_snapshot(
        snapshot,
        f1,
        plans,
        &mut scratch.ifft_scratch,
        &mut scratch.cd0,
    );

    let mut ss = [0.0_f32; 9];
    for idt in -4..=4 {
        ss[(idt + 4) as usize] = sync8d_tables.sync8d(&scratch.cd0, ibest + idt, None);
    }
    let mut iloc = 0usize;
    let mut best_fine = f32::NEG_INFINITY;
    for (idx, &val) in ss.iter().enumerate() {
        if val > best_fine {
            best_fine = val;
            iloc = idx;
        }
    }
    ibest += (iloc as isize) - 4;
    let dt_raw_sec = ((ibest - 1) as f32) * dt2;

    let np2_max = (NP2 as isize) - 1;
    let mut csymb = [Complex32::ZERO; 32];
    for k in 0..NN {
        let i1 = ibest + (k as isize) * 32;
        csymb.fill(Complex32::ZERO);
        if i1 >= 0 && i1 + 31 <= np2_max {
            let start = i1 as usize;
            csymb.copy_from_slice(&scratch.cd0[start..start + 32]);
        }
        plans
            .c2c_fwd_32
            .process_with_scratch(&mut csymb, &mut scratch.fft32_scratch);
        for tone in 0..8 {
            scratch.cs[k][tone] = csymb[tone] / 1_000.0;
            scratch.s8[k][tone] = csymb[tone].norm();
        }
    }

    // Hard Costas sync count (lowest-index tie-breaking matches Fortran `maxloc`).
    let mut nsync = 0u8;
    for (k, &expected_tone) in ICOS7.iter().enumerate() {
        for base in [0usize, 36, 72] {
            let sym_s8 = &scratch.s8[base + k];
            let mut best_tone = 0usize;
            let mut best_amp = f32::NEG_INFINITY;
            for (t, &amp) in sym_s8.iter().enumerate() {
                if amp > best_amp {
                    best_amp = amp;
                    best_tone = t;
                }
            }
            if best_tone == expected_tone as usize {
                nsync += 1;
            }
        }
    }

    let syncmin = if depth <= 2 {
        8
    } else if metric_mode == BitMetricMode::Power {
        7
    } else {
        6
    };
    if nsync <= syncmin {
        return None;
    }

    let bmeta_raw = compute_1sym_bmeta(&scratch.cs, metric_mode);
    let mut bmeta_norm = bmeta_raw;
    normalize_bmet(&mut bmeta_norm);
    const SCALEFAC: f32 = 2.83;
    let mut llra = [0.0_f32; LDPC_N];
    for (dst, &src) in llra.iter_mut().zip(bmeta_norm.iter()) {
        *dst = SCALEFAC * src;
    }

    Some(Ft8bIntermediates {
        f1_refined_hz: f1,
        delf_best_hz: delfbest,
        ibest,
        dt_raw_sec,
        sync_fine: best_fine,
        sync8d_coarse,
        nsync_hard: nsync,
        bmeta: bmeta_raw,
        llra,
    })
}

/// Decodes a single candidate with full Stage 3 multi-symbol coherent LLRs (`llra`–`llre`),
/// hybrid BP+OSD (`maxosd = 2` for `depth >= 2`), and single-slot AP (`a1`–`a6`).
///
/// Does NOT mutate `dd` or `hash_table`; caller performs deterministic hash-table commit
/// and signal subtraction when `Some` is returned.
#[allow(clippy::too_many_arguments)]
pub fn ft8b_decode_candidate_with_ap(
    snapshot: &Ft8SpectrumSnapshot,
    cand: &Ft8Candidate,
    xbase: f32,
    depth: u8,
    nzhsym: u8,
    metric_mode: BitMetricMode,
    nagain: bool,
    ncontest: u8,
    qso: Option<&QsoContext>,
    ap_seed: &ApSeed,
    plans: &Ft8FftPlans,
    downsampler: &Ft8Downsampler,
    sync8d_tables: &Sync8dTables,
    unpack_ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
    scratch: &mut Ft8bScratch,
) -> Option<Ft8bOutcome> {
    let never_cancel = AtomicBool::new(false);
    ft8b_decode_candidate_with_options(
        snapshot,
        cand,
        xbase,
        depth,
        nzhsym,
        metric_mode,
        nagain,
        ncontest,
        qso,
        ap_seed,
        plans,
        downsampler,
        sync8d_tables,
        unpack_ctx,
        hash_table,
        scratch,
        &never_cancel,
        false,
        crate::ft8::params::NMAX,
        false,
    )
}

/// Cancel-aware, lock-free FT8 candidate decoder supporting generic `CallsignLookup` and
/// strictly gated optional telemetry computation.
#[allow(clippy::too_many_arguments)]
pub fn ft8b_decode_candidate_with_options<H: CallsignLookup + ?Sized>(
    snapshot: &Ft8SpectrumSnapshot,
    cand: &Ft8Candidate,
    xbase: f32,
    depth: u8,
    nzhsym: u8,
    metric_mode: BitMetricMode,
    nagain: bool,
    ncontest: u8,
    qso: Option<&QsoContext>,
    ap_seed: &ApSeed,
    plans: &Ft8FftPlans,
    downsampler: &Ft8Downsampler,
    sync8d_tables: &Sync8dTables,
    unpack_ctx: &UnpackContext,
    hash_table: &H,
    scratch: &mut Ft8bScratch,
    cancel: &AtomicBool,
    enable_telemetry: bool,
    active_samples_12k: usize,
    early_pass: bool,
) -> Option<Ft8bOutcome> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let inter = ft8b_extract_stage2_intermediates(
        snapshot,
        cand,
        depth,
        metric_mode,
        plans,
        downsampler,
        sync8d_tables,
        scratch,
    )?;

    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let metrics = compute_all_bit_metrics(&scratch.cs, metric_mode, &mut scratch.s2);
    let npasses = ft8b_pass_count(qso, nzhsym);
    let maxosd: i32 = if depth <= 1 {
        -1
    } else if early_pass {
        1
    } else {
        2
    };
    let norder: u8 = 2;

    let mut hit = None;
    for ipass in 1..=npasses {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let iaptype: u8 = match ft8b_pass_plan(ipass, qso) {
            Ft8bPassPlan::NonAp { metric_idx } => {
                scratch.apmask.fill(0);
                scratch.llrz = metrics.llrs[metric_idx];
                0
            }
            Ft8bPassPlan::Ap {
                iaptype,
                use_llrc,
            } => {
                let Some(ctx) = qso else { continue };
                let base_llr = if use_llrc {
                    &metrics.llrs[2]
                } else {
                    &metrics.llrs[0]
                };
                if !apply_ap_hypothesis(
                    iaptype,
                    ctx,
                    ap_seed,
                    inter.f1_refined_hz,
                    base_llr,
                    &mut scratch.llrz,
                    &mut scratch.apmask,
                ) {
                    continue;
                }
                iaptype
            }
        };

        let Some(decoded) = decode174_91_with_scratch(
            &scratch.llrz,
            91,
            maxosd,
            norder,
            &scratch.apmask,
            &mut scratch.fec_scratch,
            cancel,
        ) else {
            continue;
        };

        if decoded.nhard_errors < 0 || decoded.nhard_errors > 36 {
            continue;
        }
        if decoded.codeword.iter().all(|&b| b == 0) {
            continue;
        }

        let mut message77 = [0u8; PAYLOAD_BITS];
        message77.copy_from_slice(&decoded.message91[..PAYLOAD_BITS]);

        let n3 = (message77[71] << 2) | (message77[72] << 1) | message77[73];
        let i3 = (message77[74] << 2) | (message77[75] << 1) | message77[76];
        if i3 > 5 || (i3 == 0 && (n3 > 6 || n3 == 2)) {
            continue;
        }

        let Ok(unpacked) = unpack77_readonly(&message77, UnpackMode::Rx, unpack_ctx, hash_table)
        else {
            continue;
        };
        if ncontest == 0
            && (1..=3).contains(&i3)
            && (unpacked.text.contains("/R") || unpacked.text.starts_with("TU; "))
        {
            continue;
        }

        hit = Some((decoded, message77, i3, n3, unpacked, iaptype));
        break;
    }

    let (decoded, message77, i3, n3, unpacked, iaptype) = hit?;
    let itone = encode_ft8_tones_from_77bits(&message77);

    let mut xsig = 0.0_f32;
    let mut xnoi = 0.0_f32;
    for (i, &tone) in itone.iter().enumerate() {
        let t = tone as usize;
        xsig += scratch.s8[i][t] * scratch.s8[i][t];
        let ios = (t + 4) % 7;
        xnoi += scratch.s8[i][ios] * scratch.s8[i][ios];
    }

    let arg1 = if xnoi > 0.0 { xsig / xnoi - 1.0 } else { 0.0 };
    let xsnr1 = 10.0 * (if arg1 > 0.1 { arg1 } else { 0.001 }).log10() - 27.0;

    let arg2 = if xbase > 0.0 {
        xsig / xbase / 3.0e6 - 1.0
    } else {
        0.0
    };
    let xsnr2 = 10.0 * (if arg2 > 0.1 { arg2 } else { 0.001 }).log10() - 27.0;

    let mut xsnr = if nagain { xsnr1 } else { xsnr2 };
    if inter.nsync_hard <= 10 && xsnr < -25.0 {
        return None;
    }
    if xsnr < -25.0 {
        xsnr = -25.0;
    }

    let qual = 1.0 - (decoded.nhard_errors as f32 + decoded.dmin) / 60.0;
    let ap_tag = ApTag::from_wsjtx(iaptype, qual);

    let (freq_drift_hz, qsb_envelope) = if enable_telemetry {
        let drift = estimate_ft8_costas_drift(&scratch.cd0, inter.ibest, active_samples_12k);
        let env = compute_ft8_symbol_envelope(
            &scratch.cd0,
            inter.ibest,
            &itone,
            drift,
            active_samples_12k,
        );
        (drift, env)
    } else {
        (None, None)
    };

    Some(Ft8bOutcome {
        freq_hz: inter.f1_refined_hz,
        dt_raw_sec: inter.dt_raw_sec,
        sync_fine: inter.sync_fine,
        nsync_hard: inter.nsync_hard,
        snr_db: xsnr,
        nhard_errors: decoded.nhard_errors,
        dmin: decoded.dmin,
        qual,
        iaptype,
        ap_tag,
        fec_method: decoded.fec_method,
        message: unpacked.text,
        message77,
        i3,
        n3,
        itone,
        pending_hashes: unpacked.pending_hashes,
        freq_drift_hz,
        qsb_envelope,
    })
}

/// Convenience wrapper for decoding a single candidate without AP context.
#[allow(clippy::too_many_arguments)]
pub fn ft8b_decode_candidate(
    snapshot: &Ft8SpectrumSnapshot,
    cand: &Ft8Candidate,
    xbase: f32,
    depth: u8,
    metric_mode: BitMetricMode,
    nagain: bool,
    ncontest: u8,
    plans: &Ft8FftPlans,
    downsampler: &Ft8Downsampler,
    sync8d_tables: &Sync8dTables,
    unpack_ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
    scratch: &mut Ft8bScratch,
) -> Option<Ft8bOutcome> {
    let default_seed = ApSeed::default();
    ft8b_decode_candidate_with_ap(
        snapshot,
        cand,
        xbase,
        depth,
        50,
        metric_mode,
        nagain,
        ncontest,
        None,
        &default_seed,
        plans,
        downsampler,
        sync8d_tables,
        unpack_ctx,
        hash_table,
        scratch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::downsample::DownsampleSnapshotScratch;
    use crate::ft8::params::{NH1, NMAX};
    use crate::ft8::sync8::{sync8, Sync8Scratch};
    use crate::modulate::ft8::gen_ft8_wave_real;
    use crate::protocol::pack77::pack77;

    #[test]
    fn ft8b_decodes_clean_stage1_loopback_waveform() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let msg = "CQ N0AAA EM10";
        let packed = pack77(msg, None, &ctx, &ht).unwrap();
        let tones = encode_ft8_tones_from_77bits(&packed.bits);
        let wave = gen_ft8_wave_real(&tones, 1500.0, 12_000);

        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for (i, &w) in wave.iter().enumerate() {
            dd[6000 + i] = w * 1000.0;
        }

        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let sync8d_tables = Sync8dTables::new();
        let mut sync_scratch = Sync8Scratch::new(&plans);
        let cands = sync8(&dd, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut sync_scratch);
        assert!(!cands.is_empty());

        let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let mut snapshot = Ft8SpectrumSnapshot::new();
        downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);

        let mut ft8b_scratch = Ft8bScratch::new(&plans);
        let cand = cands[0];
        let bin = ((cand.freq_hz / 3.125).round() as usize).clamp(1, NH1);
        let xbase = 10.0_f32.powf(0.1 * (sync_scratch.sbase_db()[bin] - 40.0));
        let outcome = ft8b_decode_candidate(
            &snapshot,
            &cand,
            xbase,
            1,
            BitMetricMode::Amplitude,
            false,
            0,
            &plans,
            &downsampler,
            &sync8d_tables,
            &ctx,
            &ht,
            &mut ft8b_scratch,
        )
        .expect("clean signal must decode");

        assert_eq!(outcome.message, msg);
        assert_eq!(outcome.nhard_errors, 0);
        assert_eq!(outcome.nsync_hard, 21);
        assert_eq!(outcome.ap_tag, ApTag::None);
        assert!((outcome.freq_hz - 1500.0).abs() < 1.0);
        assert!((outcome.dt_raw_sec - 0.5).abs() < 0.02);
    }

    #[test]
    fn compute_all_bit_metrics_populates_coherent_llrs_without_nans() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("N0AAA K9BBB RR73", None, &ctx, &ht).unwrap();
        let tones = encode_ft8_tones_from_77bits(&packed.bits);
        let wave = gen_ft8_wave_real(&tones, 1500.0, 12_000);
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        for (i, &w) in wave.iter().enumerate() {
            dd[6000 + i] = w * 1000.0;
        }

        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let sync8d_tables = Sync8dTables::new();
        let mut sync_scratch = Sync8Scratch::new(&plans);
        let cands = sync8(&dd, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut sync_scratch);
        let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let mut snapshot = Ft8SpectrumSnapshot::new();
        downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);
        let mut scratch = Ft8bScratch::new(&plans);
        let _inter = ft8b_extract_stage2_intermediates(
            &snapshot,
            &cands[0],
            3,
            BitMetricMode::Power,
            &plans,
            &downsampler,
            &sync8d_tables,
            &mut scratch,
        )
        .unwrap();

        let m = compute_all_bit_metrics(&scratch.cs, BitMetricMode::Power, &mut scratch.s2);
        for set in &m.llrs {
            assert!(set.iter().all(|v| v.is_finite()));
            assert!(set.iter().any(|&v| v.abs() > 1.0));
        }
    }

    #[test]
    fn normalize_bmet_handles_zero_variance_without_nan() {
        let mut zeros = [0.0_f32; LDPC_N];
        normalize_bmet(&mut zeros);
        assert!(zeros.iter().all(|&x| x == 0.0 && x.is_finite()));
    }
}
