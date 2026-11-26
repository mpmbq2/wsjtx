// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 multi-symbol coherent bit metrics and Costas sync quality checks
//! (`wsjtx/lib/ft4/get_ft4_bitmetrics.f90` and `wsjtx/lib/ft4_decode.f90`).

use num_complex::Complex32;

use crate::dsp::fft::Ft4FftPlans;

use super::params::{
    GRAYMAP, ICOS4_A, ICOS4_B, ICOS4_C, ICOS4_D, NBITS, NCW, NFRAME_DOWN, NN, NSS, NSYNC_MIN,
    SYNC_BITS_A, SYNC_BITS_B, SYNC_BITS_C, SYNC_BITS_D, SYNC_POSITIONS,
};

/// Result of `get_ft4_bitmetrics`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ft4BitMetricsStatus {
    /// Hard-decision Costas symbol matches (`0..=16`).
    pub nsync: usize,
    /// True if `nsync < 8` (frame rejected before LDPC).
    pub badsync: bool,
}

/// Reusable scratch buffers for `get_ft4_bitmetrics` (zero per-candidate allocations).
pub struct Ft4BitMetricsScratch {
    cs: [[Complex32; 4]; NN],
    s4: [[f32; 4]; NN],
    fft_buf: [Complex32; NSS],
    fft_scratch: Vec<Complex32>,
    s2: [f32; 256],
    col_buf: [f32; NBITS],
}

impl Ft4BitMetricsScratch {
    pub fn new(plans: &Ft4FftPlans) -> Self {
        Self {
            cs: [[Complex32::ZERO; 4]; NN],
            s4: [[0.0; 4]; NN],
            fft_buf: [Complex32::ZERO; NSS],
            fft_scratch: vec![Complex32::ZERO; plans.c2c_fwd_32.get_inplace_scratch_len()],
            s2: [0.0; 256],
            col_buf: [0.0; NBITS],
        }
    }
}

/// Exact port of `wsjtx/lib/ft8/normalizebmet.f90`.
pub fn normalizebmet(bmet: &mut [f32]) {
    let n = bmet.len();
    if n == 0 {
        return;
    }
    let inv_n = 1.0_f32 / (n as f32);
    let bmet_av: f32 = bmet.iter().sum::<f32>() * inv_n;
    let bmet2_av: f32 = bmet.iter().map(|&v| v * v).sum::<f32>() * inv_n;
    let var = bmet2_av - bmet_av * bmet_av;
    let bmet_sig = if var > 0.0 {
        var.sqrt()
    } else {
        bmet2_av.sqrt()
    };
    if bmet_sig > 0.0 && bmet_sig.is_finite() {
        let inv_sig = 1.0_f32 / bmet_sig;
        for v in bmet.iter_mut() {
            *v *= inv_sig;
        }
    }
}

#[inline]
fn argmax4(vals: &[f32; 4]) -> u8 {
    let mut best_idx = 0usize;
    let mut best_val = vals[0];
    for (idx, &v) in vals.iter().enumerate().skip(1) {
        if v > best_val {
            best_val = v;
            best_idx = idx;
        }
    }
    best_idx as u8
}

/// Port of `wsjtx/lib/ft4/get_ft4_bitmetrics.f90`.
///
/// Computes 1-, 2-, and 4-symbol coherent log-likelihood ratios (`bitmetrics[206][3]`),
/// copies trailing ungrouped symbol metrics before calling `normalizebmet`, and performs
/// the `nsync < 8` hard-decision Costas gate.
pub fn get_ft4_bitmetrics(
    cd: &[Complex32; NFRAME_DOWN],
    plans: &Ft4FftPlans,
    scratch: &mut Ft4BitMetricsScratch,
    bitmetrics: &mut [[f32; 3]; NBITS],
) -> Ft4BitMetricsStatus {
    // 1. Symbol-synchronous 32-pt forward FFTs across all 103 symbols.
    for k in 0..NN {
        let i1 = k * NSS;
        scratch.fft_buf.copy_from_slice(&cd[i1..i1 + NSS]);
        plans
            .c2c_fwd_32
            .process_with_scratch(&mut scratch.fft_buf, &mut scratch.fft_scratch);
        for t in 0..4 {
            let c = scratch.fft_buf[t];
            scratch.cs[k][t] = c;
            scratch.s4[k][t] = c.norm();
        }
    }

    // 2. Hard-decision Costas symbol check across 4 sync arrays (16 symbols).
    let costas = [ICOS4_A, ICOS4_B, ICOS4_C, ICOS4_D];
    let mut nsync = 0usize;
    for (block_idx, &pos) in SYNC_POSITIONS.iter().enumerate() {
        for (j, &expected_tone) in costas[block_idx].iter().enumerate() {
            if argmax4(&scratch.s4[pos + j]) == expected_tone {
                nsync += 1;
            }
        }
    }
    if nsync < NSYNC_MIN {
        return Ft4BitMetricsStatus {
            nsync,
            badsync: true,
        };
    }

    bitmetrics.fill([0.0; 3]);

    // 3. Coherent 1-symbol, 2-symbol, and 4-symbol soft bit metrics.
    // ip = 0 -> nsym = 1 (steps 0..=102)
    for ks in 0..NN {
        for (dibit, &tone) in GRAYMAP.iter().enumerate() {
            scratch.s2[dibit] = scratch.s4[ks][tone as usize];
        }
        let s2 = &scratch.s2[..4];
        let b0_m1 = s2[2].max(s2[3]);
        let b0_m0 = s2[0].max(s2[1]);
        let b1_m1 = s2[1].max(s2[3]);
        let b1_m0 = s2[0].max(s2[2]);
        bitmetrics[2 * ks][0] = b0_m1 - b0_m0;
        bitmetrics[2 * ks + 1][0] = b1_m1 - b1_m0;
    }

    // ip = 1 -> nsym = 2 (steps 0, 2, ..., 100)
    for ks in (0..=(NN - 2)).step_by(2) {
        let cs0 = scratch.cs[ks];
        let cs1 = scratch.cs[ks + 1];
        for t in 0..16 {
            let t0 = GRAYMAP[t >> 2] as usize;
            let t1 = GRAYMAP[t & 3] as usize;
            scratch.s2[t] = (cs0[t0] + cs1[t1]).norm();
        }
        let s2 = &scratch.s2[..16];
        for j in 0..4 {
            let bit_mask = 1usize << (3 - j);
            let mut xm1 = f32::NEG_INFINITY;
            let mut xm0 = f32::NEG_INFINITY;
            for (t, &val) in s2.iter().enumerate() {
                if (t & bit_mask) != 0 {
                    xm1 = xm1.max(val);
                } else {
                    xm0 = xm0.max(val);
                }
            }
            bitmetrics[2 * ks + j][1] = xm1 - xm0;
        }
    }

    // ip = 2 -> nsym = 4 (steps 0, 4, ..., 96)
    for ks in (0..=(NN - 4)).step_by(4) {
        let cs0 = scratch.cs[ks];
        let cs1 = scratch.cs[ks + 1];
        let cs2 = scratch.cs[ks + 2];
        let cs3 = scratch.cs[ks + 3];
        let mut pair0 = [Complex32::ZERO; 16];
        let mut pair1 = [Complex32::ZERO; 16];
        for p in 0..16 {
            let ta = GRAYMAP[p >> 2] as usize;
            let tb = GRAYMAP[p & 3] as usize;
            pair0[p] = cs0[ta] + cs1[tb];
            pair1[p] = cs2[ta] + cs3[tb];
        }
        for t in 0..256 {
            scratch.s2[t] = (pair0[t >> 4] + pair1[t & 15]).norm();
        }
        let s2 = &scratch.s2[..256];
        for j in 0..8 {
            let bit_mask = 1usize << (7 - j);
            let mut xm1 = f32::NEG_INFINITY;
            let mut xm0 = f32::NEG_INFINITY;
            for (t, &val) in s2.iter().enumerate() {
                if (t & bit_mask) != 0 {
                    xm1 = xm1.max(val);
                } else {
                    xm0 = xm0.max(val);
                }
            }
            bitmetrics[2 * ks + j][2] = xm1 - xm0;
        }
    }

    // Copy ungrouped tail symbol metrics BEFORE normalization (NN = 103).
    // Note `ks = 102` (bits 204..=205) for nsym = 2 comes from nsym = 1, and
    // `ks = 100..=102` (bits 200..=205) for nsym = 4 come from nsym = 2 and nsym = 1.
    bitmetrics[204][1] = bitmetrics[204][0];
    bitmetrics[205][1] = bitmetrics[205][0];
    for row in &mut bitmetrics[200..204] {
        row[2] = row[1];
    }
    bitmetrics[204][2] = bitmetrics[204][0];
    bitmetrics[205][2] = bitmetrics[205][0];

    // 4. Independent RMS variance normalization of each of the 3 columns.
    for col in 0..3 {
        for (dst, row) in scratch.col_buf.iter_mut().zip(bitmetrics.iter()) {
            *dst = row[col];
        }
        normalizebmet(&mut scratch.col_buf);
        for (row, &val) in bitmetrics.iter_mut().zip(scratch.col_buf.iter()) {
            row[col] = val;
        }
    }

    Ft4BitMetricsStatus {
        nsync,
        badsync: false,
    }
}

/// Counts hard-decision sync bit matches across all 32 Costas bits (`bitmetrics[:, 0] >= 0.0`)
/// matching `ft4_decode.f90:300-307` (`nsync_qual`).
pub fn count_sync_bit_quality(bitmetrics: &[[f32; 3]; NBITS]) -> usize {
    let sync_bits = [SYNC_BITS_A, SYNC_BITS_B, SYNC_BITS_C, SYNC_BITS_D];
    let mut matches = 0usize;
    for (block_idx, &sym_pos) in SYNC_POSITIONS.iter().enumerate() {
        let bit_start = 2 * sym_pos;
        for (j, &expected) in sync_bits[block_idx].iter().enumerate() {
            let hard = u8::from(bitmetrics[bit_start + j][0] >= 0.0);
            if hard == expected {
                matches += 1;
            }
        }
    }
    matches
}

/// Extracts the 174 scaled payload bit LLRs (`llra`, `llrb`, `llrc`) skipping the 4 Costas
/// blocks (`ft4_decode.f90:312-320`).
pub fn extract_ft4_llrs(bitmetrics: &[[f32; 3]; NBITS], scalefac: f32) -> [[f32; NCW]; 3] {
    let mut llrs = [[0.0_f32; NCW]; 3];
    for col in 0..3 {
        let mut out_idx = 0usize;
        for row in &bitmetrics[8..66] {
            llrs[col][out_idx] = scalefac * row[col];
            out_idx += 1;
        }
        for row in &bitmetrics[74..132] {
            llrs[col][out_idx] = scalefac * row[col];
            out_idx += 1;
        }
        for row in &bitmetrics[140..198] {
            llrs[col][out_idx] = scalefac * row[col];
            out_idx += 1;
        }
        debug_assert_eq!(out_idx, NCW);
    }
    llrs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::downsample::{Ft4DownsampleScratch, Ft4Downsampler, Ft4PassSpectrum};
    use crate::ft4::params::{NMAX, SCALEFAC};
    use crate::modulate::ft4::{encode_ft4_tones_from_77bits, gen_ft4_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::ldpc::encode174_91;
    use crate::protocol::pack77::pack77;
    use crate::protocol::rvec::scramble_ft4;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn clean_ft4_frame_gives_perfect_sync_and_zero_bit_errors() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let scrambled = scramble_ft4(&packed.bits);
        let cw = encode174_91(&scrambled);

        let tones = encode_ft4_tones_from_77bits(&packed.bits);
        let wave = gen_ft4_wave_real(&tones, 1500.0, 12_000);

        // Place TX ramp at sample 5424 so Costas symbol 0 starts cleanly at sample 6000 (i0 = 333).
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        dd[5424..5424 + wave.len()].copy_from_slice(&wave);

        let plans = Ft4FftPlans::new();
        let down = Ft4Downsampler::new();
        let mut d_scratch = Ft4DownsampleScratch::new(&plans);
        let mut spectrum = Ft4PassSpectrum::new();
        down.compute_pass_spectrum(&dd, &plans, &mut d_scratch, &mut spectrum);

        let mut cd0 = [Complex32::ZERO; super::super::params::NDMAX];
        down.downsample_from_spectrum(&spectrum, 1500.0, &plans, &mut d_scratch, &mut cd0);

        let mut cd = [Complex32::ZERO; NFRAME_DOWN];
        let i0 = 6000 / 18; // 333
        cd.copy_from_slice(&cd0[i0..i0 + NFRAME_DOWN]);

        let mut scratch = Ft4BitMetricsScratch::new(&plans);
        let mut bitmetrics = [[0.0_f32; 3]; NBITS];
        let status = get_ft4_bitmetrics(&cd, &plans, &mut scratch, &mut bitmetrics);

        assert!(!status.badsync);
        assert_eq!(status.nsync, 16);
        assert_eq!(count_sync_bit_quality(&bitmetrics), 32);

        let llrs = extract_ft4_llrs(&bitmetrics, SCALEFAC);
        for (col_idx, col) in llrs.iter().enumerate() {
            let hard_errs = (0..NCW)
                .filter(|&b| u8::from(col[b] >= 0.0) != cw[b])
                .count();
            assert_eq!(hard_errs, 0, "unexpected hard errors on metric col {col_idx}");
        }
    }
}
