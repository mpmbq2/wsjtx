// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Bit-packed GF(2) Ordered Statistics Decoder (`wsjtx/lib/ft8/osd174_91.f90`).
//!
//! Uses `std` sorting instead of Numerical Recipes `indexx`, per-scratch pre-2 hash buckets
//! instead of Fortran `COMMON /boxes/`, and full-width `id..N` pivoting for guaranteed
//! full-rank systematic reduction while preserving exact WSJT-X parity.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::protocol::crc14::{compute_crc14, verify_crc14};
use crate::protocol::ldpc::encode174_91_nocrc;

use super::bp::{LDPC_K, LDPC_N};

pub const N_BITS: usize = LDPC_N;
pub const K_MAX: usize = LDPC_K;
pub const K_MIN: usize = 77;
const WORD2_MASK: u64 = (1u64 << (N_BITS - 128)) - 1; // 46 bits

/// 174-bit GF(2) row vector packed into `3 × u64` (LSB = bit `0`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct BitVec174(pub [u64; 3]);

impl BitVec174 {
    #[inline(always)]
    pub fn get(&self, bit: usize) -> bool {
        debug_assert!(bit < N_BITS);
        ((self.0[bit >> 6] >> (bit & 63)) & 1) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, bit: usize) {
        debug_assert!(bit < N_BITS);
        self.0[bit >> 6] |= 1u64 << (bit & 63);
    }

    #[inline(always)]
    pub fn xor_assign(&mut self, rhs: &Self) {
        self.0[0] ^= rhs.0[0];
        self.0[1] ^= rhs.0[1];
        self.0[2] ^= rhs.0[2];
    }

    #[inline(always)]
    pub fn swap_bits(&mut self, a: usize, b: usize) {
        debug_assert!(a < N_BITS && b < N_BITS);
        let ba = (self.0[a >> 6] >> (a & 63)) & 1;
        let bb = (self.0[b >> 6] >> (b & 63)) & 1;
        let diff = ba ^ bb;
        self.0[a >> 6] ^= diff << (a & 63);
        self.0[b >> 6] ^= diff << (b & 63);
    }

    /// Extracts least-reliable parity bits `k..174` (at most 97 bits) into the low bits of `u128`.
    #[inline(always)]
    pub fn extract_lrb(&self, k: usize) -> u128 {
        debug_assert!((K_MIN..=K_MAX).contains(&k));
        let shift1 = k - 64; // 13..=27
        let lo = (self.0[1] >> shift1) as u128;
        let hi = ((self.0[2] & WORD2_MASK) as u128) << (128 - k);
        lo | hi
    }
}

struct GeneratorCache {
    /// Column `c` contains bit `r` set iff `gen[r][c] == 1` (`r in 0..k`).
    cols: [u128; N_BITS],
}

fn build_generator_cache(k: usize) -> GeneratorCache {
    assert!((K_MIN..=K_MAX).contains(&k));
    let mut cols = [0u128; N_BITS];
    for r in 0..k {
        let mut message91 = [0u8; K_MAX];
        message91[r] = 1;
        if r < 77 {
            let mut msg77 = [0u8; 77];
            msg77[r] = 1;
            let crc = compute_crc14(&msg77);
            for bit in 0..14 {
                message91[77 + bit] = ((crc >> (13 - bit)) & 1) as u8;
            }
            message91[77..k].fill(0);
        }
        let cw = encode174_91_nocrc(&message91);
        for (c, &bit) in cw.iter().enumerate() {
            if bit != 0 {
                cols[c] |= 1u128 << r;
            }
        }
    }
    GeneratorCache { cols }
}

static GEN_CACHES: [OnceLock<GeneratorCache>; K_MAX - K_MIN + 1] =
    [const { OnceLock::new() }; K_MAX - K_MIN + 1];

fn generator_cache_for(k: usize) -> &'static GeneratorCache {
    GEN_CACHES[k - K_MIN].get_or_init(|| build_generator_cache(k))
}

#[derive(Debug, Clone)]
struct Pre2Boxes {
    head: Vec<i32>,
    tail: Vec<i32>,
    pairs: Vec<(u8, u8)>,
    next: Vec<i32>,
}

impl Pre2Boxes {
    fn new() -> Self {
        Self {
            head: Vec::new(),
            tail: Vec::new(),
            pairs: Vec::with_capacity(K_MAX * (K_MAX - 1) / 2),
            next: Vec::with_capacity(K_MAX * (K_MAX - 1) / 2),
        }
    }

    fn reset(&mut self, ntau: usize) {
        let n_buckets = 1usize << ntau;
        self.head.clear();
        self.head.resize(n_buckets, -1);
        self.tail.clear();
        self.tail.resize(n_buckets, -1);
        self.pairs.clear();
        self.next.clear();
    }
}

/// Reusable per-worker scratch buffers for `osd174_91` (zero static mutable state, `rayon`-safe).
#[derive(Debug, Clone)]
pub struct OsdScratch {
    genmrb: [BitVec174; K_MAX],
    parity_rows: [u128; K_MAX],
    indices: [usize; N_BITS],
    absrx_orig: [f32; N_BITS],
    absrx_perm: [f32; N_BITS],
    hdec_orig: [u8; N_BITS],
    boxes: Option<Box<Pre2Boxes>>,
}

impl Default for OsdScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl OsdScratch {
    pub fn new() -> Self {
        Self {
            genmrb: [BitVec174::default(); K_MAX],
            parity_rows: [0u128; K_MAX],
            indices: [0usize; N_BITS],
            absrx_orig: [0.0; N_BITS],
            absrx_perm: [0.0; N_BITS],
            hdec_orig: [0u8; N_BITS],
            boxes: None,
        }
    }
}

/// Result returned by `osd174_91`.
#[derive(Debug, Clone, Copy)]
pub struct OsdResult {
    pub message91: [u8; K_MAX],
    pub cw: [u8; N_BITS],
    /// Positive (or 0) if CRC-14 verified; negative (`-nhardmin`) if CRC-14 failed.
    pub nhardmin: i32,
    pub crc_ok: bool,
    pub dmin: f32,
    /// OSD order that produced the winning codeword (`0`, `1`, `2`, ...).
    pub winning_order: u8,
    /// Diagnostic flag set when Gauss-Jordan required a pivot at column `>= k + 20`.
    pub used_extended_pivot: bool,
}

#[inline(always)]
fn xor_parity_lrb(mut mask: u128, parity_rows: &[u128]) -> u128 {
    let mut acc = 0u128;
    while mask != 0 {
        let r = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        acc ^= parity_rows[r];
    }
    acc
}

#[inline(always)]
fn mrb_weighted_dist(mut mask: u128, weights: &[f32]) -> f32 {
    let mut acc = 0.0_f32;
    while mask != 0 {
        let idx = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        acc += weights[idx];
    }
    acc
}

#[inline(always)]
fn lrb_weighted_dist(mut mask: u128, weights: &[f32]) -> f32 {
    let mut acc = 0.0_f32;
    while mask != 0 {
        let idx = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        acc += weights[idx];
    }
    acc
}

/// Bit-packed equivalent of Fortran `nextpat91(misub, k, iorder, iflag)`.
///
/// Advances `misub` (weight `iorder` within bits `0..k`) in the exact same order as
/// `osd174_91.f90:307-334`, returning `Some((next_mask, lowest_set_bit))` or `None` when done.
#[inline]
fn next_pat(misub: u128, k: usize, iorder: usize) -> Option<(u128, usize)> {
    // Find largest `ind` in `0..k-1` with bit `ind == 0` and bit `ind+1 == 1`.
    let candidates = (!misub) & (misub >> 1);
    if candidates == 0 {
        return None;
    }
    let ind = 127 - candidates.leading_zeros() as usize;
    let prefix_mask = if ind == 0 { 0 } else { (1u128 << ind) - 1 };
    let mut ms = (misub & prefix_mask) | (1u128 << ind);
    let nz = iorder - (ms.count_ones() as usize);
    if nz > 0 {
        ms |= ((1u128 << nz) - 1) << (k - nz);
    }
    let iflag = ms.trailing_zeros() as usize;
    Some((ms, iflag))
}

#[derive(Clone, Copy)]
struct NdeepParams {
    nord: usize,
    npre1: usize,
    npre2: usize,
    nt: usize,
    ntheta: u32,
    ntau: usize,
}

#[inline]
fn ndeep_params(ndeep: u8) -> NdeepParams {
    match ndeep {
        1 => NdeepParams {
            nord: 1,
            npre1: 0,
            npre2: 0,
            nt: 40,
            ntheta: 12,
            ntau: 0,
        },
        2 => NdeepParams {
            nord: 1,
            npre1: 1,
            npre2: 0,
            nt: 40,
            ntheta: 10,
            ntau: 0,
        },
        3 => NdeepParams {
            nord: 1,
            npre1: 1,
            npre2: 1,
            nt: 40,
            ntheta: 12,
            ntau: 14,
        },
        4 => NdeepParams {
            nord: 2,
            npre1: 1,
            npre2: 1,
            nt: 40,
            ntheta: 12,
            ntau: 17,
        },
        5 => NdeepParams {
            nord: 3,
            npre1: 1,
            npre2: 1,
            nt: 40,
            ntheta: 12,
            ntau: 15,
        },
        _ => NdeepParams {
            nord: 4,
            npre1: 1,
            npre2: 1,
            nt: 95,
            ntheta: 12,
            ntau: 15,
        },
    }
}

/// Bit-packed GF(2) Ordered Statistics Decoder porting `wsjtx/lib/ft8/osd174_91.f90`.
///
/// Valid `keff` values are `77..=91` (`91` in FT8/FT4). `ndeep` (`0..=6`) controls search order.
pub fn osd174_91(
    llr: &[f32; N_BITS],
    keff: usize,
    apmask: &[u8; N_BITS],
    ndeep: u8,
    scratch: &mut OsdScratch,
) -> Option<OsdResult> {
    let never_cancel = AtomicBool::new(false);
    osd174_91_cancelable(llr, keff, apmask, ndeep, scratch, &never_cancel)
}

/// Cancel-aware Ordered Statistics Decoder (`osd174_91.f90`), checking `cancel` every 16 outer iterations.
pub fn osd174_91_cancelable(
    llr: &[f32; N_BITS],
    keff: usize,
    apmask: &[u8; N_BITS],
    ndeep: u8,
    scratch: &mut OsdScratch,
    cancel: &AtomicBool,
) -> Option<OsdResult> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    if !(K_MIN..=K_MAX).contains(&keff) {
        return None;
    }
    let k = keff;
    let gen = generator_cache_for(k);

    for i in 0..N_BITS {
        scratch.hdec_orig[i] = if llr[i] >= 0.0 { 1 } else { 0 };
        scratch.absrx_orig[i] = llr[i].abs();
        scratch.indices[i] = i;
    }

    // Decreasing reliability sort (deterministic clean-room std sort replacing NR `indexx`).
    scratch.indices.sort_by(|&a, &b| {
        scratch.absrx_orig[b]
            .total_cmp(&scratch.absrx_orig[a])
            .then_with(|| a.cmp(&b))
    });

    // Populate permuted generator matrix column-by-column via sparse column masks.
    for row in &mut scratch.genmrb[..k] {
        *row = BitVec174::default();
    }
    for c in 0..N_BITS {
        let mut col_mask = gen.cols[scratch.indices[c]];
        while col_mask != 0 {
            let r = col_mask.trailing_zeros() as usize;
            col_mask &= col_mask - 1;
            scratch.genmrb[r].set(c);
        }
    }

    // GF(2) Gauss-Jordan elimination over `id..N_BITS` (identical pivot to `id..k+20` whenever
    // present in `id..k+20`, while avoiding Fortran's latent silent non-systematic bug).
    let mut used_extended_pivot = false;
    let adhoc_limit = (k + 20).min(N_BITS);
    for id in 0..k {
        let mut pivot_col = None;
        for icol in id..N_BITS {
            if scratch.genmrb[id].get(icol) {
                pivot_col = Some(icol);
                break;
            }
        }
        let icol = pivot_col?;
        if icol >= adhoc_limit {
            used_extended_pivot = true;
        }
        if icol != id {
            for row in &mut scratch.genmrb[..k] {
                row.swap_bits(id, icol);
            }
            scratch.indices.swap(id, icol);
        }
        let pivot_row = scratch.genmrb[id];
        for (ii, row) in scratch.genmrb[..k].iter_mut().enumerate() {
            if ii != id && row.get(id) {
                row.xor_assign(&pivot_row);
            }
        }
    }

    for r in 0..k {
        scratch.parity_rows[r] = scratch.genmrb[r].extract_lrb(k);
    }

    let mut hdec_mrb: u128 = 0;
    let mut apmask_mrb: u128 = 0;
    for pos in 0..k {
        let orig = scratch.indices[pos];
        scratch.absrx_perm[pos] = scratch.absrx_orig[orig];
        if scratch.hdec_orig[orig] != 0 {
            hdec_mrb |= 1u128 << pos;
        }
        if apmask[orig] != 0 {
            apmask_mrb |= 1u128 << pos;
        }
    }

    let lrb_len = N_BITS - k;
    let mut hdec_lrb: u128 = 0;
    for pos in 0..lrb_len {
        let orig = scratch.indices[k + pos];
        scratch.absrx_perm[k + pos] = scratch.absrx_orig[orig];
        if scratch.hdec_orig[orig] != 0 {
            hdec_lrb |= 1u128 << pos;
        }
    }

    // Order-0 codeword evaluation.
    let c0_lrb = xor_parity_lrb(hdec_mrb, &scratch.parity_rows[..k]);
    let e0_lrb = c0_lrb ^ hdec_lrb;
    let mut best_nhard = e0_lrb.count_ones() as i32;
    let mut best_dmin = lrb_weighted_dist(e0_lrb, &scratch.absrx_perm[k..]);
    let mut best_mrb = hdec_mrb;
    let mut best_lrb = c0_lrb;
    let mut winning_order: u8 = 0;

    let ndeep_clamped = ndeep.min(6);
    if ndeep_clamped > 0 {
        let params = ndeep_params(ndeep_clamped);
        let nt_clamped = params.nt.min(lrb_len);
        let nt_mask = if nt_clamped == 128 {
            u128::MAX
        } else {
            (1u128 << nt_clamped) - 1
        };

        let mut iter_count = 0usize;
        for iorder in 1..=params.nord {
            let mut misub: u128 = ((1u128 << iorder) - 1) << (k - iorder);
            let mut iflag_opt = Some(k - iorder);
            while let Some(iflag) = iflag_opt {
                if (iter_count & 0x0F) == 0 && cancel.load(Ordering::Relaxed) {
                    return None;
                }
                iter_count += 1;
                if (apmask_mrb & misub) == 0 {
                    let iend = if iorder == params.nord && params.npre1 == 0 {
                        iflag
                    } else {
                        0
                    };

                    let me_mrb = hdec_mrb ^ misub;
                    let e2sub = e0_lrb ^ xor_parity_lrb(misub, &scratch.parity_rows[..k]);
                    let d1 = mrb_weighted_dist(misub, &scratch.absrx_perm[..k]);

                    // `n1 == iflag`: pattern is `misub` itself (weight `iorder`).
                    let nd1kpt = (e2sub & nt_mask).count_ones() + 1;
                    if nd1kpt <= params.ntheta {
                        let dd = d1 + lrb_weighted_dist(e2sub, &scratch.absrx_perm[k..]);
                        if dd < best_dmin {
                            best_dmin = dd;
                            best_mrb = me_mrb;
                            best_lrb = e2sub ^ hdec_lrb;
                            best_nhard = (iorder as i32) + (e2sub.count_ones() as i32);
                            winning_order = iorder as u8;
                        }
                    }

                    // `n1 < iflag` down to `iend`: pre-1 single-bit extension (weight `iorder + 1`).
                    if iend < iflag {
                        for n1 in (iend..iflag).rev() {
                            if ((apmask_mrb >> n1) & 1) != 0 {
                                continue;
                            }
                            let e2 = e2sub ^ scratch.parity_rows[n1];
                            let nd1kpt = (e2 & nt_mask).count_ones() + 2;
                            if nd1kpt <= params.ntheta {
                                let dd = d1
                                    + scratch.absrx_perm[n1]
                                    + lrb_weighted_dist(e2, &scratch.absrx_perm[k..]);
                                if dd < best_dmin {
                                    best_dmin = dd;
                                    best_mrb = me_mrb ^ (1u128 << n1);
                                    best_lrb = e2 ^ hdec_lrb;
                                    best_nhard =
                                        (iorder as i32 + 1) + (e2.count_ones() as i32);
                                    winning_order = (iorder + 1) as u8;
                                }
                            }
                        }
                    }
                }

                if let Some((next_m, next_iflag)) = next_pat(misub, k, iorder) {
                    misub = next_m;
                    iflag_opt = Some(next_iflag);
                } else {
                    iflag_opt = None;
                }
            }
        }

        if params.npre2 == 1 {
            let ntau = params.ntau.min(lrb_len);
            let pat_mask = (1u128 << ntau) - 1;
            let boxes = scratch
                .boxes
                .get_or_insert_with(|| Box::new(Pre2Boxes::new()));
            boxes.reset(ntau);

            for i1 in (0..k).rev() {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                for i2 in (0..i1).rev() {
                    let ipat = ((scratch.parity_rows[i1] ^ scratch.parity_rows[i2]) & pat_mask)
                        as usize;
                    let idx = boxes.pairs.len() as i32;
                    boxes.pairs.push((i1 as u8, i2 as u8));
                    boxes.next.push(-1);
                    if boxes.head[ipat] == -1 {
                        boxes.head[ipat] = idx;
                        boxes.tail[ipat] = idx;
                    } else {
                        let prev_tail = boxes.tail[ipat] as usize;
                        boxes.next[prev_tail] = idx;
                        boxes.tail[ipat] = idx;
                    }
                }
            }

            let mut misub: u128 = ((1u128 << params.nord) - 1) << (k - params.nord);
            let mut iflag_opt = Some(k - params.nord);
            let mut pre2_iter = 0usize;
            while let Some(_iflag) = iflag_opt {
                if (pre2_iter & 0x0F) == 0 && cancel.load(Ordering::Relaxed) {
                    return None;
                }
                pre2_iter += 1;
                if (apmask_mrb & misub) == 0 {
                    let e2sub = e0_lrb ^ xor_parity_lrb(misub, &scratch.parity_rows[..k]);
                    let e2sub_tau = e2sub & pat_mask;
                    let d_base = mrb_weighted_dist(misub, &scratch.absrx_perm[..k]);

                    for i2_err in 0..=ntau {
                        let r2_tau = if i2_err == 0 {
                            e2sub_tau
                        } else {
                            e2sub_tau ^ (1u128 << (i2_err - 1))
                        };
                        let mut cur = boxes.head[r2_tau as usize];
                        while cur >= 0 {
                            let (in1, in2) = boxes.pairs[cur as usize];
                            cur = boxes.next[cur as usize];
                            let pair_mask = (1u128 << in1) | (1u128 << in2);
                            if (misub & pair_mask) != 0 || (apmask_mrb & pair_mask) != 0 {
                                continue;
                            }
                            let mi = misub | pair_mask;
                            let e2_lrb = e2sub ^ scratch.parity_rows[in1 as usize]
                                ^ scratch.parity_rows[in2 as usize];
                            let dd = d_base
                                + scratch.absrx_perm[in1 as usize]
                                + scratch.absrx_perm[in2 as usize]
                                + lrb_weighted_dist(e2_lrb, &scratch.absrx_perm[k..]);
                            if dd < best_dmin {
                                best_dmin = dd;
                                best_mrb = hdec_mrb ^ mi;
                                best_lrb = e2_lrb ^ hdec_lrb;
                                best_nhard = (params.nord as i32 + 2)
                                    + (e2_lrb.count_ones() as i32);
                                winning_order = (params.nord + 2) as u8;
                            }
                        }
                    }
                }

                if let Some((next_m, next_iflag)) = next_pat(misub, k, params.nord) {
                    misub = next_m;
                    iflag_opt = Some(next_iflag);
                } else {
                    iflag_opt = None;
                }
            }
        }
    }

    // Un-permute back to original [message91 | parity] order and verify CRC-14.
    let mut cw = [0u8; N_BITS];
    for pos in 0..k {
        cw[scratch.indices[pos]] = ((best_mrb >> pos) & 1) as u8;
    }
    for pos in 0..lrb_len {
        cw[scratch.indices[k + pos]] = ((best_lrb >> pos) & 1) as u8;
    }

    let mut message91 = [0u8; K_MAX];
    message91.copy_from_slice(&cw[..K_MAX]);
    let crc_ok = verify_crc14(&message91);
    let nhardmin = if crc_ok { best_nhard } else { -best_nhard };

    Some(OsdResult {
        message91,
        cw,
        nhardmin,
        crc_ok,
        dmin: best_dmin,
        winning_order,
        used_extended_pivot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fec::bp::{NM, NRW};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::ldpc::encode174_91;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    fn sample_codeword(msg: &str) -> [u8; N_BITS] {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77(msg, None, &ctx, &ht).unwrap();
        encode174_91(&packed.bits)
    }

    #[test]
    fn generator_caches_satisfy_parity_equations_for_all_k() {
        for k in K_MIN..=K_MAX {
            let gen = generator_cache_for(k);
            for r in 0..k {
                let mut cw = [0u8; N_BITS];
                for c in 0..N_BITS {
                    cw[c] = ((gen.cols[c] >> r) & 1) as u8;
                }
                for chk in 0..83 {
                    let parity: u8 = NM[chk][..NRW[chk]].iter().map(|&b| cw[b]).sum();
                    assert_eq!(parity & 1, 0, "k={k} row={r} chk={chk}");
                }
            }
        }
    }

    #[test]
    fn osd_recovers_corrupted_codewords_across_orders_and_keff() {
        let cw = sample_codeword("CQ N0AAA EM10");
        let mut scratch = OsdScratch::new();
        let apmask = [0u8; N_BITS];

        let mut llr = [0.0_f32; N_BITS];
        for i in 0..N_BITS {
            // Make reliability strictly decreasing with index so first 91 bits are MRB.
            let mag = 5.0 - (i as f32) * 0.02;
            llr[i] = if cw[i] == 1 { mag } else { -mag };
        }

        // Order-0 clean recovery.
        let r0 = osd174_91(&llr, 91, &apmask, 0, &mut scratch).unwrap();
        assert!(r0.crc_ok);
        assert_eq!(r0.nhardmin, 0);
        assert_eq!(r0.cw, cw);
        assert_eq!(r0.winning_order, 0);

        // Flip 2 high-confidence MRB bits (kept in MRB so order-0 fails, order-2 succeeds).
        let mut llr_2err = llr;
        llr_2err[5] = -llr_2err[5];
        llr_2err[25] = -llr_2err[25];
        // Also flip 2 LRB bits (reliabilities stay lower than MRB).
        for idx in [110, 125] {
            llr_2err[idx] = -llr_2err[idx];
        }

        let r0_fail = osd174_91(&llr_2err, 91, &apmask, 0, &mut scratch).unwrap();
        assert_ne!(r0_fail.cw, cw);

        let r2 = osd174_91(&llr_2err, 91, &apmask, 2, &mut scratch).unwrap();
        assert!(r2.crc_ok);
        assert_eq!(r2.cw, cw);
        assert!(r2.nhardmin > 0);
        assert_eq!(r2.winning_order, 2);

        // Test Keff = 77 and 84 and ndeep = 3, 4.
        for keff in [77usize, 84, 91] {
            for ndeep in [0u8, 1, 2, 3, 4] {
                let r = osd174_91(&llr_2err, keff, &apmask, ndeep, &mut scratch).unwrap();
                if ndeep >= 2 {
                    assert!(r.crc_ok, "keff={keff} ndeep={ndeep} failed CRC");
                    assert_eq!(r.cw, cw, "keff={keff} ndeep={ndeep} wrong cw");
                }
            }
        }
    }

    #[test]
    fn osd_respects_apmask_and_handles_adversarial_llrs() {
        let cw = sample_codeword("N0AAA K9BBB RR73");
        let mut scratch = OsdScratch::new();

        // Lock first 77 bits with huge AP magnitude; corrupt bit 10 inside AP mask?
        // No, test that AP bits are NEVER flipped by OSD when they sit in MRB.
        let mut llr = [0.0_f32; N_BITS];
        let mut apmask = [0u8; N_BITS];
        for i in 0..N_BITS {
            if i < 58 {
                apmask[i] = 1;
                llr[i] = if cw[i] == 1 { 50.0 } else { -50.0 };
            } else {
                let mag = 3.0 - ((i - 58) as f32) * 0.015;
                llr[i] = if cw[i] == 1 { mag } else { -mag };
            }
        }
        // Flip bit 65 (non-AP MRB) and bit 10 (AP-locked). Flipping an AP-locked bit wrong should fail.
        let mut llr_ap_wrong = llr;
        llr_ap_wrong[10] = -llr_ap_wrong[10];
        let res_locked = osd174_91(&llr_ap_wrong, 91, &apmask, 2, &mut scratch).unwrap();
        assert_ne!(
            res_locked.cw[10], cw[10],
            "OSD must not flip AP-locked MRB bit 10"
        );

        // Adversarial all-zero and extreme LLRs must not panic or produce NaNs.
        let zeros = [0.0_f32; N_BITS];
        let r_zero = osd174_91(&zeros, 91, &[0u8; N_BITS], 3, &mut scratch).unwrap();
        assert!(r_zero.dmin.is_finite());

        let mut extreme = [0.0_f32; N_BITS];
        for (i, v) in extreme.iter_mut().enumerate() {
            *v = if cw[i] == 1 { 1e4 } else { -1e4 };
        }
        let r_ext = osd174_91(&extreme, 91, &[0u8; N_BITS], 2, &mut scratch).unwrap();
        assert!(r_ext.crc_ok);
        assert_eq!(r_ext.cw, cw);
    }

    #[test]
    fn osd174_91_aborts_on_cancellation_signal() {
        let mut scratch = OsdScratch::new();
        let llr = [0.1_f32; N_BITS];
        let apmask = [0u8; N_BITS];
        let cancelled = AtomicBool::new(true);
        assert!(osd174_91_cancelable(&llr, 91, &apmask, 4, &mut scratch, &cancelled).is_none());
    }
}
