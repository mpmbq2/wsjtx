// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Hybrid BP + OSD `decode174_91` entry point (`wsjtx/lib/ft8/decode174_91.f90`).

use super::bp::{bp_decode_174_91, LdpcDecoded, LDPC_N};
use super::osd::{osd174_91, OsdScratch};

/// Hybrid BP + OSD decoder matching `wsjtx/lib/ft8/decode174_91.f90`.
///
/// In Stage 2, `max_osd < 0` executes Belief Propagation only; Stage 3 enables `osd174_91`
/// over `bp.zsave`.
pub fn decode174_91(
    llr: &[f32; LDPC_N],
    keff: usize,
    max_osd: i32,
    norder: u8,
    apmask: &[u8; LDPC_N],
    osd_scratch: &mut OsdScratch,
) -> Option<LdpcDecoded> {
    let bp = bp_decode_174_91(llr, apmask, max_osd);
    if let Some(decoded) = bp.decoded {
        return Some(decoded);
    }

    if max_osd < 0 {
        return None;
    }

    for zrow in bp.zsave.iter().take(bp.num_zsave) {
        if let Some(res) = osd174_91(zrow, keff, apmask, norder, osd_scratch) {
            if res.crc_ok && res.nhardmin > 0 {
                let mut dmin = 0.0_f32;
                for b in 0..LDPC_N {
                    let hdec = if llr[b] >= 0.0 { 1u8 } else { 0u8 };
                    if hdec != res.cw[b] {
                        dmin += llr[b].abs();
                    }
                }
                return Some(LdpcDecoded {
                    message91: res.message91,
                    codeword: res.cw,
                    nhard_errors: res.nhardmin,
                    dmin,
                    fec_method: crate::telemetry::FecMethod::Osd {
                        order: res.winning_order,
                    },
                });
            }
        }
    }

    None
}
