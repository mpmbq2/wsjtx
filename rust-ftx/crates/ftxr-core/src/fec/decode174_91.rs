// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Hybrid BP + OSD `decode174_91` entry point (`wsjtx/lib/ft8/decode174_91.f90`).

use super::bp::{bp_decode_174_91, LdpcDecoded, LDPC_N};
use super::osd::{osd174_91, OsdScratch};

/// Hybrid BP + OSD decoder matching `wsjtx/lib/ft8/decode174_91.f90`.
///
/// `max_osd < 0` executes Belief Propagation only; `max_osd == 0` invokes `osd174_91` on
/// raw channel LLRs; `max_osd in 1..=3` invokes `osd174_91` over saved BP posterior sums `zsave`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::ldpc::encode174_91;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn hybrid_bp_plus_osd_recovers_when_bp_alone_fails() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ N0AAA EM10", None, &ctx, &ht).unwrap();
        let cw = encode174_91(&packed.bits);

        // Construct an LLR pattern where BP-only (max_osd = -1) fails, yet OSD (max_osd = 2) succeeds.
        // Flip 24 bits with moderate confidence (-1.5) and weaken several others (+0.3).
        let mut llr = [0.0_f32; LDPC_N];
        for i in 0..LDPC_N {
            let sign = if cw[i] == 1 { 1.0_f32 } else { -1.0_f32 };
            llr[i] = sign * 2.2;
        }
        for idx in (0..LDPC_N).step_by(7) {
            let sign = if cw[idx] == 1 { -1.0_f32 } else { 1.0_f32 };
            llr[idx] = sign * 1.4;
        }

        let mut osd_scratch = OsdScratch::new();
        let apmask = [0u8; LDPC_N];

        // BP only fails:
        assert!(decode174_91(&llr, 91, -1, 2, &apmask, &mut osd_scratch).is_none());

        // Hybrid BP+OSD recovers exact codeword and recomputes channel dmin:
        let dec = decode174_91(&llr, 91, 2, 2, &apmask, &mut osd_scratch)
            .expect("OSD should rescue failed BP frame");
        assert_eq!(dec.codeword, cw);
        assert!(matches!(dec.fec_method, crate::telemetry::FecMethod::Osd { .. }));
        assert!(dec.nhard_errors > 0);

        let expected_dmin: f32 = (0..LDPC_N)
            .filter(|&b| (if llr[b] >= 0.0 { 1u8 } else { 0u8 }) != cw[b])
            .map(|b| llr[b].abs())
            .sum();
        assert!((dec.dmin - expected_dmin).abs() < 1e-4);
    }
}
