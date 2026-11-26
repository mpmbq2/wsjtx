// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! LDPC(174,91) Belief Propagation decoder (`decode174_91.f90`, `platanh.f90`,
//! `ldpc_174_91_c_parity.f90`).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::protocol::crc14::verify_crc14;
use crate::telemetry::FecMethod;

pub const LDPC_N: usize = 174;
pub const LDPC_K: usize = 91;
pub const LDPC_M: usize = 83;
pub const LDPC_COL_WEIGHT: usize = 3;
pub const BP_MAX_ITERS: usize = 30;

/// 0-based check indices per variable bit (`Mn(1:3, 1:174)` minus 1 from `ldpc_174_91_c_parity.f90`).
#[rustfmt::skip]
pub static MN: [[usize; LDPC_COL_WEIGHT]; LDPC_N] = [
    [15, 44, 72], [24, 50, 61], [32, 57, 77], [ 0, 43, 44], [ 1,  6, 60], [ 2,  5, 53],
    [ 3, 34, 47], [ 4, 12, 20], [ 7, 55, 78], [ 8, 63, 68], [ 9, 18, 65], [10, 35, 59],
    [11, 36, 57], [13, 31, 42], [14, 62, 79], [16, 27, 76], [17, 73, 82], [21, 52, 80],
    [22, 29, 33], [23, 30, 39], [25, 40, 75], [26, 56, 69], [28, 48, 64], [ 2, 37, 77],
    [ 4, 38, 81], [45, 49, 72], [50, 51, 73], [54, 70, 71], [43, 66, 71], [42, 67, 77],
    [ 0, 31, 58], [ 1,  5, 70], [ 3, 15, 53], [ 6, 64, 66], [ 7, 29, 41], [ 8, 21, 30],
    [ 9, 17, 75], [10, 22, 81], [11, 27, 60], [12, 51, 78], [13, 49, 50], [14, 80, 82],
    [16, 28, 59], [18, 32, 63], [19, 25, 72], [20, 33, 39], [23, 26, 76], [24, 54, 57],
    [34, 52, 65], [35, 47, 67], [36, 45, 74], [37, 44, 46], [38, 56, 68], [40, 55, 61],
    [19, 48, 52], [45, 51, 62], [44, 69, 74], [26, 34, 79], [ 0, 14, 29], [ 1, 67, 79],
    [ 2, 35, 50], [ 3, 27, 50], [ 4, 30, 55], [ 5, 19, 36], [ 6, 39, 81], [ 7, 59, 68],
    [ 8,  9, 48], [10, 43, 56], [11, 38, 58], [12, 23, 54], [13, 20, 64], [15, 70, 77],
    [16, 29, 75], [17, 24, 79], [18, 60, 82], [21, 37, 76], [22, 40, 49], [ 6, 25, 57],
    [28, 31, 80], [32, 39, 72], [17, 33, 47], [12, 41, 63], [ 4, 25, 42], [46, 68, 71],
    [53, 54, 69], [44, 61, 67], [ 9, 62, 66], [13, 65, 71], [21, 59, 73], [34, 38, 78],
    [ 0, 45, 63], [ 0, 23, 65], [ 1,  4, 69], [ 2, 30, 64], [ 3, 48, 57], [ 0,  3,  4],
    [ 5, 59, 66], [ 6, 31, 74], [ 7, 47, 81], [ 8, 34, 40], [ 9, 38, 61], [10, 13, 60],
    [11, 70, 73], [12, 22, 77], [10, 34, 54], [14, 15, 78], [ 6,  8, 15], [16, 53, 62],
    [17, 49, 56], [18, 29, 46], [19, 63, 79], [20, 27, 68], [21, 24, 42], [12, 21, 36],
    [ 1, 46, 50], [22, 53, 73], [25, 33, 71], [26, 35, 36], [20, 35, 62], [28, 39, 43],
    [18, 25, 56], [ 2, 45, 81], [13, 14, 57], [32, 51, 52], [29, 42, 51], [ 5,  8, 51],
    [26, 32, 64], [24, 68, 72], [37, 54, 82], [19, 38, 76], [17, 28, 55], [31, 47, 70],
    [41, 50, 58], [27, 43, 78], [33, 59, 61], [30, 44, 60], [45, 67, 76], [ 5, 23, 75],
    [ 7,  9, 77], [39, 40, 69], [16, 49, 52], [41, 65, 67], [ 3, 21, 71], [35, 63, 80],
    [12, 28, 46], [ 1,  7, 80], [55, 66, 72], [ 4, 37, 49], [11, 37, 63], [58, 71, 79],
    [ 2, 25, 78], [44, 75, 80], [ 0, 64, 73], [ 6, 17, 76], [10, 55, 58], [13, 38, 53],
    [15, 36, 65], [ 9, 27, 54], [14, 59, 69], [16, 24, 81], [19, 29, 30], [11, 66, 67],
    [22, 74, 79], [26, 31, 61], [23, 68, 74], [18, 20, 70], [33, 52, 60], [34, 45, 46],
    [32, 58, 75], [39, 42, 82], [40, 41, 62], [48, 74, 82], [19, 43, 47], [41, 48, 56],
];

/// 0-based variable bit indices per parity check (`Nm(1:7, 1:83)` minus 1; unused 7th slot = 0).
#[rustfmt::skip]
pub static NM: [[usize; 7]; LDPC_M] = [
    [ 3, 30, 58, 90, 91, 95, 152], [ 4, 31, 59, 92, 114, 145, 0],
    [ 5, 23, 60, 93, 121, 150, 0], [ 6, 32, 61, 94, 95, 142, 0],
    [ 7, 24, 62, 82, 92, 95, 147], [ 5, 31, 63, 96, 125, 137, 0],
    [ 4, 33, 64, 77, 97, 106, 153], [ 8, 34, 65, 98, 138, 145, 0],
    [ 9, 35, 66, 99, 106, 125, 0], [10, 36, 66, 86, 100, 138, 157],
    [11, 37, 67, 101, 104, 154, 0], [12, 38, 68, 102, 148, 161, 0],
    [ 7, 39, 69, 81, 103, 113, 144], [13, 40, 70, 87, 101, 122, 155],
    [14, 41, 58, 105, 122, 158, 0], [ 0, 32, 71, 105, 106, 156, 0],
    [15, 42, 72, 107, 140, 159, 0], [16, 36, 73, 80, 108, 130, 153],
    [10, 43, 74, 109, 120, 165, 0], [44, 54, 63, 110, 129, 160, 172],
    [ 7, 45, 70, 111, 118, 165, 0], [17, 35, 75, 88, 112, 113, 142],
    [18, 37, 76, 103, 115, 162, 0], [19, 46, 69, 91, 137, 164, 0],
    [ 1, 47, 73, 112, 127, 159, 0], [20, 44, 77, 82, 116, 120, 150],
    [21, 46, 57, 117, 126, 163, 0], [15, 38, 61, 111, 133, 157, 0],
    [22, 42, 78, 119, 130, 144, 0], [18, 34, 58, 72, 109, 124, 160],
    [19, 35, 62, 93, 135, 160, 0], [13, 30, 78, 97, 131, 163, 0],
    [ 2, 43, 79, 123, 126, 168, 0], [18, 45, 80, 116, 134, 166, 0],
    [ 6, 48, 57, 89, 99, 104, 167], [11, 49, 60, 117, 118, 143, 0],
    [12, 50, 63, 113, 117, 156, 0], [23, 51, 75, 128, 147, 148, 0],
    [24, 52, 68, 89, 100, 129, 155], [19, 45, 64, 79, 119, 139, 169],
    [20, 53, 76, 99, 139, 170, 0], [34, 81, 132, 141, 170, 173, 0],
    [13, 29, 82, 112, 124, 169, 0], [ 3, 28, 67, 119, 133, 172, 0],
    [ 0,  3, 51, 56, 85, 135, 151], [25, 50, 55, 90, 121, 136, 167],
    [51, 83, 109, 114, 144, 167, 0], [ 6, 49, 80, 98, 131, 172, 0],
    [22, 54, 66, 94, 171, 173, 0], [25, 40, 76, 108, 140, 147, 0],
    [ 1, 26, 40, 60, 61, 114, 132], [26, 39, 55, 123, 124, 125, 0],
    [17, 48, 54, 123, 140, 166, 0], [ 5, 32, 84, 107, 115, 155, 0],
    [27, 47, 69, 84, 104, 128, 157], [ 8, 53, 62, 130, 146, 154, 0],
    [21, 52, 67, 108, 120, 173, 0], [ 2, 12, 47, 77, 94, 122, 0],
    [30, 68, 132, 149, 154, 168, 0], [11, 42, 65, 88, 96, 134, 158],
    [ 4, 38, 74, 101, 135, 166, 0], [ 1, 53, 85, 100, 134, 163, 0],
    [14, 55, 86, 107, 118, 170, 0], [ 9, 43, 81, 90, 110, 143, 148],
    [22, 33, 70, 93, 126, 152, 0], [10, 48, 87, 91, 141, 156, 0],
    [28, 33, 86, 96, 146, 161, 0], [29, 49, 59, 85, 136, 141, 161],
    [ 9, 52, 65, 83, 111, 127, 164], [21, 56, 84, 92, 139, 158, 0],
    [27, 31, 71, 102, 131, 165, 0], [27, 28, 83, 87, 116, 142, 149],
    [ 0, 25, 44, 79, 127, 146, 0], [16, 26, 88, 102, 115, 152, 0],
    [50, 56, 97, 162, 164, 171, 0], [20, 36, 72, 137, 151, 168, 0],
    [15, 46, 75, 129, 136, 153, 0], [ 2, 23, 29, 71, 103, 138, 0],
    [ 8, 39, 89, 105, 133, 150, 0], [14, 57, 59, 73, 110, 149, 162],
    [17, 41, 78, 143, 145, 151, 0], [24, 37, 64, 98, 121, 159, 0],
    [16, 41, 74, 128, 169, 171, 0],
];

/// Row weights for each of the 83 parity-check rows (`6` or `7`).
#[rustfmt::skip]
pub static NRW: [usize; LDPC_M] = [
    7, 6, 6, 6, 7, 6, 7, 6, 6, 7, 6, 6, 7, 7, 6, 6,
    6, 7, 6, 7, 6, 7, 6, 6, 6, 7, 6, 6, 6, 7, 6, 6,
    6, 6, 7, 6, 6, 6, 7, 7, 6, 6, 6, 6, 7, 7, 6, 6,
    6, 6, 7, 6, 6, 6, 7, 6, 6, 6, 6, 7, 6, 6, 6, 7,
    6, 6, 6, 7, 7, 6, 6, 7, 6, 6, 6, 6, 6, 6, 6, 7,
    6, 6, 6,
];

/// Exact piecewise-linear `atanh` approximation from `wsjtx/lib/platanh.f90`.
#[inline]
pub fn platanh(x: f32) -> f32 {
    let (isign, z) = if x < 0.0 { (-1.0_f32, -x) } else { (1.0_f32, x) };
    if z <= 0.664 {
        x / 0.83
    } else if z <= 0.9217 {
        isign * (z - 0.4064) / 0.322
    } else if z <= 0.9951 {
        isign * (z - 0.8378) / 0.0524
    } else if z <= 0.9998 {
        isign * (z - 0.9914) / 0.0012
    } else {
        isign * 7.0
    }
}

#[derive(Debug, Clone)]
pub struct LdpcDecoded {
    pub message91: [u8; LDPC_K],
    pub codeword: [u8; LDPC_N],
    pub nhard_errors: i32,
    pub dmin: f32,
    pub fec_method: FecMethod,
}

#[derive(Debug, Clone)]
pub struct BpDecodeOutput {
    pub decoded: Option<LdpcDecoded>,
    /// Cumulative posterior LLRs saved at iterations `1..=max_osd` (or channel LLR if `max_osd == 0`).
    /// Consumed by Stage 3 `osd174_91`.
    pub zsave: [[f32; LDPC_N]; 3],
    pub num_zsave: usize,
    pub iterations_run: usize,
    pub final_unsatisfied_checks: usize,
}

/// Reusable scratch buffers for `bp_decode_174_91_with_scratch` (zero per-call heap allocations).
#[derive(Debug, Clone)]
pub struct BpScratch {
    toc: [[f32; 7]; LDPC_M],
    tov: [[f32; LDPC_COL_WEIGHT]; LDPC_N],
    tanhtoc: [[f32; 7]; LDPC_M],
    zn: [f32; LDPC_N],
    zsum: [f32; LDPC_N],
    cw: [u8; LDPC_N],
}

impl Default for BpScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl BpScratch {
    pub fn new() -> Self {
        Self {
            toc: [[0.0_f32; 7]; LDPC_M],
            tov: [[0.0_f32; LDPC_COL_WEIGHT]; LDPC_N],
            tanhtoc: [[0.0_f32; 7]; LDPC_M],
            zn: [0.0_f32; LDPC_N],
            zsum: [0.0_f32; LDPC_N],
            cw: [0u8; LDPC_N],
        }
    }
}

/// Pure-Rust port of the Belief Propagation stage in `wsjtx/lib/ft8/decode174_91.f90`.
pub fn bp_decode_174_91(
    llr: &[f32; LDPC_N],
    apmask: &[u8; LDPC_N],
    max_osd: i32,
) -> BpDecodeOutput {
    let mut scratch = BpScratch::new();
    let never_cancel = AtomicBool::new(false);
    bp_decode_174_91_with_scratch(llr, apmask, max_osd, &mut scratch, &never_cancel)
}

/// Cancel-aware, scratch-backed LDPC(174, 91) Belief Propagation decoder.
pub fn bp_decode_174_91_with_scratch(
    llr: &[f32; LDPC_N],
    apmask: &[u8; LDPC_N],
    max_osd: i32,
    scratch: &mut BpScratch,
    cancel: &AtomicBool,
) -> BpDecodeOutput {
    let max_osd_clamped = max_osd.min(3);
    let mut zsave = [[0.0_f32; LDPC_N]; 3];
    let num_zsave = if max_osd_clamped == 0 {
        zsave[0] = *llr;
        1
    } else if max_osd_clamped > 0 {
        max_osd_clamped as usize
    } else {
        0
    };

    let toc = &mut scratch.toc;
    let tov = &mut scratch.tov;
    let tanhtoc = &mut scratch.tanhtoc;
    tov.fill([0.0_f32; LDPC_COL_WEIGHT]);

    for j in 0..LDPC_M {
        for i in 0..NRW[j] {
            toc[j][i] = llr[NM[j][i]];
        }
    }

    let zn = &mut scratch.zn;
    let zsum = &mut scratch.zsum;
    zsum.fill(0.0);
    let mut ncnt = 0usize;
    let mut nclast = 0usize;
    let mut iterations_run = 0usize;
    let mut final_unsatisfied_checks = LDPC_M;
    let cw = &mut scratch.cw;

    for iter in 0..=BP_MAX_ITERS {
        if cancel.load(Ordering::Relaxed) {
            return BpDecodeOutput {
                decoded: None,
                zsave,
                num_zsave: 0,
                iterations_run,
                final_unsatisfied_checks,
            };
        }
        iterations_run = iter + 1;

        for i in 0..LDPC_N {
            if apmask[i] != 1 {
                zn[i] = llr[i] + tov[i][0] + tov[i][1] + tov[i][2];
            } else {
                zn[i] = llr[i];
            }
            zsum[i] += zn[i];
        }
        if iter > 0 && (iter as i32) <= max_osd_clamped {
            zsave[iter - 1] = *zsum;
        }

        cw.fill(0);
        for i in 0..LDPC_N {
            if zn[i] > 0.0 {
                cw[i] = 1;
            }
        }

        let mut ncheck = 0usize;
        for j in 0..LDPC_M {
            let mut sum = 0u8;
            for i in 0..NRW[j] {
                sum ^= cw[NM[j][i]];
            }
            if sum != 0 {
                ncheck += 1;
            }
        }
        final_unsatisfied_checks = ncheck;

        if ncheck == 0 {
            let mut message91 = [0u8; LDPC_K];
            message91.copy_from_slice(&cw[..LDPC_K]);
            if verify_crc14(&message91) {
                let mut nhard = 0i32;
                let mut dmin = 0.0_f32;
                for i in 0..LDPC_N {
                    let sign = if cw[i] == 1 { 1.0_f32 } else { -1.0_f32 };
                    if sign * llr[i] < 0.0 {
                        nhard += 1;
                    }
                    let hdec = if llr[i] >= 0.0 { 1u8 } else { 0u8 };
                    if hdec != cw[i] {
                        dmin += llr[i].abs();
                    }
                }
                return BpDecodeOutput {
                    decoded: Some(LdpcDecoded {
                        message91,
                        codeword: *cw,
                        nhard_errors: nhard,
                        dmin,
                        fec_method: FecMethod::Bp {
                            iterations: iterations_run as u8,
                        },
                    }),
                    zsave,
                    num_zsave,
                    iterations_run,
                    final_unsatisfied_checks,
                };
            }
        }

        if iter > 0 {
            if ncheck < nclast {
                ncnt = 0;
            } else {
                ncnt += 1;
            }
            if ncnt >= 5 && iter >= 10 && ncheck > 15 {
                break;
            }
        }
        nclast = ncheck;

        if iter == BP_MAX_ITERS {
            break;
        }

        for j in 0..LDPC_M {
            for i in 0..NRW[j] {
                let ibj = NM[j][i];
                let mut val = zn[ibj];
                for kk in 0..LDPC_COL_WEIGHT {
                    if MN[ibj][kk] == j {
                        val -= tov[ibj][kk];
                    }
                }
                toc[j][i] = val;
            }
        }

        for j in 0..LDPC_M {
            for i in 0..NRW[j] {
                tanhtoc[j][i] = (-toc[j][i] * 0.5).tanh();
            }
        }

        for j in 0..LDPC_N {
            for i in 0..LDPC_COL_WEIGHT {
                let ichk = MN[j][i];
                let mut tmn = 1.0_f32;
                for k in 0..NRW[ichk] {
                    if NM[ichk][k] != j {
                        tmn *= tanhtoc[ichk][k];
                    }
                }
                tov[j][i] = 2.0 * platanh(-tmn);
            }
        }
    }

    BpDecodeOutput {
        decoded: None,
        zsave,
        num_zsave,
        iterations_run,
        final_unsatisfied_checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::crc14::compute_crc14;
    use crate::protocol::ldpc::encode174_91;

    #[test]
    fn mn_and_nm_are_exact_transposes_and_orthogonal_to_generator() {
        let mut from_mn = Vec::new();
        for (bit, checks) in MN.iter().enumerate() {
            for &chk in checks {
                assert!(chk < LDPC_M);
                from_mn.push((bit, chk));
            }
        }
        let mut from_nm = Vec::new();
        for (chk, row) in NM.iter().enumerate() {
            let w = NRW[chk];
            assert!(w == 6 || w == 7);
            for &bit in &row[..w] {
                assert!(bit < LDPC_N);
                from_nm.push((bit, chk));
            }
        }
        assert_eq!(from_mn.len(), 522);
        assert_eq!(from_nm.len(), 522);
        from_mn.sort_unstable();
        from_nm.sort_unstable();
        assert_eq!(from_mn, from_nm);

        // Verify H * cw == 0 (mod 2) across 500 random Stage-1 encoded codewords.
        let mut state: u64 = 0x517C_C1B7_2722_0A95;
        for _ in 0..500 {
            let mut msg77 = [0u8; 77];
            for b in &mut msg77 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                *b = ((state >> 63) & 1) as u8;
            }
            let cw = encode174_91(&msg77);
            for chk in 0..LDPC_M {
                let parity: u8 = NM[chk][..NRW[chk]].iter().map(|&b| cw[b]).sum();
                assert_eq!(parity & 1, 0);
            }
        }
    }

    #[test]
    fn bp_recovers_noisy_codewords_and_rejects_corrupted_crc() {
        let mut msg77 = [0u8; 77];
        msg77[0] = 1;
        msg77[17] = 1;
        msg77[76] = 1;
        let cw = encode174_91(&msg77);

        let mut llr = [0.0_f32; LDPC_N];
        for i in 0..LDPC_N {
            llr[i] = if cw[i] == 1 { 3.5 } else { -3.5 };
        }
        // Flip 12 strong bits (exceeding single-step check correction, within BP threshold).
        for idx in (0..174).step_by(15) {
            llr[idx] = -llr[idx];
        }

        let apmask = [0u8; LDPC_N];
        let out = bp_decode_174_91(&llr, &apmask, 2);
        let dec = out.decoded.expect("BP should recover 12-error codeword");
        assert_eq!(dec.codeword, cw);
        assert_eq!(dec.nhard_errors, 12);
        assert!(matches!(dec.fec_method, FecMethod::Bp { iterations } if iterations >= 1));

        // Pre-cancelled decode aborts immediately with None and zero iterations.
        let cancelled = AtomicBool::new(true);
        let mut scratch = BpScratch::new();
        let abort_out =
            bp_decode_174_91_with_scratch(&llr, &apmask, 2, &mut scratch, &cancelled);
        assert!(abort_out.decoded.is_none());
        assert_eq!(abort_out.iterations_run, 0);

        // Also verify 20 flipped bits with soft LLR magnitudes (like AWGN fading).
        let mut soft_llr = [0.0_f32; LDPC_N];
        for i in 0..LDPC_N {
            soft_llr[i] = if cw[i] == 1 { 4.0 } else { -4.0 };
        }
        for idx in (0..174).step_by(9) {
            soft_llr[idx] = if cw[idx] == 1 { -0.5 } else { 0.5 };
        }
        let soft_dec = bp_decode_174_91(&soft_llr, &apmask, -1)
            .decoded
            .expect("BP should recover 20 soft-error bits");
        assert_eq!(soft_dec.codeword, cw);
        assert_eq!(soft_dec.nhard_errors, 20);

        // Verify CRC check catches a valid-parity non-CRC codeword (e.g. flip payload + recompute parity without CRC).
        let crc = compute_crc14(&msg77);
        let mut msg91_bad_crc = [0u8; 91];
        msg91_bad_crc[..77].copy_from_slice(&msg77);
        for i in 0..14 {
            msg91_bad_crc[77 + i] = ((crc >> (13 - i)) & 1) as u8;
        }
        msg91_bad_crc[77] ^= 1;
        let bad_cw = crate::protocol::ldpc::encode174_91_nocrc(&msg91_bad_crc);
        let mut bad_llr = [0.0_f32; LDPC_N];
        for i in 0..LDPC_N {
            bad_llr[i] = if bad_cw[i] == 1 { 5.0 } else { -5.0 };
        }
        assert!(bp_decode_174_91(&bad_llr, &apmask, -1).decoded.is_none());
    }
}
