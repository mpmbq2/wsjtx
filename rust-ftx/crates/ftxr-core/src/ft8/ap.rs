// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 single-slot A Priori (`a1`–`a6`) hypothesis engine (`ft8apset.f90`, `ft8b.f90`).
//!
//! Cross-slot `a7` and RX-frequency list `a8` are explicitly deferred; all generated
//! `ApTag`s emit `None`, `"a1"`–`"a6"`, or `"? a1"`–`"? a6"` matching `digi-dx`'s
//! `SlotDecodeReconciler`.

use serde::{Deserialize, Serialize};

use crate::fec::bp::LDPC_N;
use crate::protocol::hashtable::{ihashcall, sanitize_hash_call, CallsignHashTable};
use crate::protocol::pack77::{pack77, stdcall};
use crate::protocol::types::{UnpackContext, UnpackMode};
use crate::protocol::unpack77::unpack77;
use crate::QsoContext;

const fn to_bipolar<const N: usize>(bits: [u8; N]) -> [i8; N] {
    let mut out = [0i8; N];
    let mut i = 0;
    while i < N {
        out[i] = if bits[i] != 0 { 1 } else { -1 };
        i += 1;
    }
    out
}

/// `CQ` (`n28 = 2`, `ipa = 0`), `ft8b.f90:40`.
pub const MCQ: [i8; 29] = to_bipolar([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
]);
/// `CQ RU`, `ft8b.f90:41`.
pub const MCQ_RU: [i8; 29] = to_bipolar([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0,
]);
/// `CQ FD`, `ft8b.f90:42`.
pub const MCQ_FD: [i8; 29] = to_bipolar([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0,
]);
/// `CQ TEST`, `ft8b.f90:43`.
pub const MCQ_TEST: [i8; 29] = to_bipolar([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 0,
]);
/// `CQ WW`, `ft8b.f90:44`.
pub const MCQ_WW: [i8; 29] = to_bipolar([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1, 0,
]);
/// `RRR` tail bits `58..77` (`ir=0, igrid4=32402, i3=1`), `ft8b.f90:45`.
pub const MRRR: [i8; 19] =
    to_bipolar([0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1]);
/// `73` tail bits `58..77` (`ir=0, igrid4=32404, i3=1`), `ft8b.f90:46`.
pub const M73: [i8; 19] =
    to_bipolar([0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 1]);
/// `RR73` tail bits `58..77` (`ir=0, igrid4=32373, i3=1`), `ft8b.f90:47`.
pub const MRR73: [i8; 19] =
    to_bipolar([0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 0, 1]);

/// WSJT-X QSO progress state (`nQSOProgress` in `ft8b.f90:61-82`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum QsoProgress {
    #[default]
    Calling = 0,     // Tx6 (CQ) -> [a1, a2]
    Replying = 1,    // Tx1      -> [a2, a3]
    Report = 2,      // Tx2      -> [a2, a3]
    RogerReport = 3, // Tx3      -> [a3, a4, a5, a6]
    Rogers = 4,      // Tx4      -> [a3, a4, a5, a6]
    Signoff = 5,     // Tx5      -> [a3, a1, a2]
}

impl QsoProgress {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Calling,
            1 => Self::Replying,
            2 => Self::Report,
            3 => Self::RogerReport,
            4 => Self::Rogers,
            _ => Self::Signoff,
        }
    }
}

const NAPPASSES: [usize; 6] = [2, 2, 2, 4, 4, 3];
const NAPTYPES: [[u8; 4]; 6] = [
    [1, 2, 0, 0], // Calling (Tx6)
    [2, 3, 0, 0], // Replying (Tx1)
    [2, 3, 0, 0], // Report (Tx2)
    [3, 4, 5, 6], // RogerReport (Tx3)
    [3, 4, 5, 6], // Rogers (Tx4)
    [3, 1, 2, 0], // Signoff (Tx5)
];

/// Precomputed AP callsign bit seeds matching `wsjtx/lib/ft8/ft8apset.f90`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApSeed {
    /// Bipolar `{-1, +1}` bits for `MyCall DxCall` (`0..58`), with `99` sentinel at `[0]`/`[29]`.
    pub apsym: [i8; 58],
    /// Bipolar `{-1, +1}` 10-bit hash of `DxCall`, with `99` sentinel at `[0]`.
    pub aph10: [i8; 10],
}

impl Default for ApSeed {
    fn default() -> Self {
        let mut apsym = [0i8; 58];
        apsym[0] = 99;
        apsym[29] = 99;
        let mut aph10 = [0i8; 10];
        aph10[0] = 99;
        Self { apsym, aph10 }
    }
}

/// Computes `apsym` and `aph10` seeds from `QsoContext` (`wsjtx/lib/ft8/ft8apset.f90`).
///
/// Registers valid `his_call` into `hash_table` (matching `ft8apset.f90:28`) while using an
/// isolated scratch table for the dummy `"KA1ABC"` encoding check so dummy callsigns never
/// pollute the session hash table.
pub fn compute_ap_seed(ctx: &QsoContext, hash_table: &CallsignHashTable) -> ApSeed {
    let mut seed = ApSeed::default();
    let my_upper = ctx
        .my_call
        .as_deref()
        .map(|s| s.trim().to_ascii_uppercase())
        .unwrap_or_default();
    if my_upper.len() < 3 {
        return seed;
    }
    let my_clean = sanitize_hash_call(&my_upper);

    let his_upper = ctx
        .his_call
        .as_deref()
        .map(|s| s.trim().to_ascii_uppercase())
        .unwrap_or_default();
    let nohiscall = his_upper.len() < 3;
    let hiscall: &str = if nohiscall { "KA1ABC" } else { &his_upper };

    if !nohiscall {
        if let Some(his_clean) = sanitize_hash_call(hiscall) {
            hash_table.save_call(&his_clean, my_clean.as_deref());
            let n10 = ihashcall(&his_clean, 10) & 0x3FF;
            for (b, slot) in seed.aph10.iter_mut().enumerate() {
                *slot = if ((n10 >> (9 - b)) & 1) != 0 { 1 } else { -1 };
            }
        }
    }

    let msg = if stdcall(&my_upper) {
        format!("{my_upper} {hiscall} RRR")
    } else {
        format!("<{my_upper}> {hiscall} RRR")
    };

    let tmp_ht = CallsignHashTable::new();
    tmp_ht.save_call(&my_upper, my_clean.as_deref());
    if !nohiscall {
        tmp_ht.save_call(hiscall, my_clean.as_deref());
    }
    let uctx = UnpackContext {
        my_call: Some(my_upper.clone()),
        dx_call: Some(hiscall.to_string()),
        dx_base: None,
    };

    let Ok(packed) = pack77(&msg, None, &uctx, &tmp_ht) else {
        return seed;
    };
    let Ok(unpacked) = unpack77(&packed.bits, UnpackMode::Rx, &uctx, &tmp_ht) else {
        return seed;
    };

    let ncontest = ctx.contest as u8;
    if ncontest == 7 && packed.i3 != 1 {
        return seed;
    }
    if ncontest <= 5 && (packed.i3 != 1 || unpacked.text != msg) {
        return seed;
    }

    for b in 0..58 {
        seed.apsym[b] = if packed.bits[b] != 0 { 1 } else { -1 };
    }
    if nohiscall {
        seed.apsym[29] = 99;
        seed.aph10[0] = 99;
    }
    seed
}

/// Returns total number of LLR/AP passes (`5` to `13`) for `ft8b` (`ft8b.f90:272-281`).
#[inline]
pub fn ft8b_pass_count(qso: Option<&QsoContext>, nzhsym: u8) -> usize {
    if nzhsym < 50 {
        return 5;
    }
    let Some(ctx) = qso else {
        return 5;
    };
    let ncontest = ctx.contest as u8;
    if !(ctx.ap_enabled || ncontest == 7) {
        return 5;
    }
    if ctx.ap_cq_only {
        7
    } else {
        let prog = (ctx.qso_progress as usize).min(5);
        5 + 2 * NAPPASSES[prog]
    }
}

/// Descriptor for one `ft8b` decoding pass (`ipass = 1..=npasses`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ft8bPassPlan {
    NonAp { metric_idx: usize },
    Ap { iaptype: u8, use_llrc: bool },
}

/// Returns the exact LLR metric source and AP type for 1-based `ipass`.
#[inline]
pub fn ft8b_pass_plan(ipass: usize, qso: Option<&QsoContext>) -> Ft8bPassPlan {
    if ipass <= 5 {
        return Ft8bPassPlan::NonAp {
            metric_idx: ipass - 1,
        };
    }
    let use_llrc = (ipass - 5) % 2 == 0;
    let iaptype = match qso {
        Some(ctx) if ctx.ap_cq_only => 1,
        Some(ctx) => {
            let prog = (ctx.qso_progress as usize).min(5);
            let idx = (ipass - 6) / 2;
            NAPTYPES[prog].get(idx).copied().unwrap_or(0)
        }
        None => 0,
    };
    Ft8bPassPlan::Ap { iaptype, use_llrc }
}

/// Applies an AP hypothesis `iaptype in 1..=6` to `llrz_base`, returning `(llrz, apmask)`
/// or `None` if a WSJT-X bailout condition rejects this AP hypothesis (`ft8b.f90:315-422`).
pub fn apply_ap_hypothesis(
    iaptype: u8,
    ctx: &QsoContext,
    seed: &ApSeed,
    f1_hz: f32,
    llrz_base: &[f32; LDPC_N],
    llrz_out: &mut [f32; LDPC_N],
    apmask_out: &mut [u8; LDPC_N],
) -> bool {
    let ncontest = ctx.contest as u8;
    let nfqso = ctx.rx_freq_hz;
    let nftx = ctx.tx_freq_hz;
    let napwid = ctx.ap_width_hz;

    if ncontest <= 5
        && iaptype >= 3
        && (f1_hz - nfqso).abs() > napwid
        && (f1_hz - nftx).abs() > napwid
    {
        return false;
    }
    if ncontest == 6 {
        return false;
    }
    if ncontest == 7 && f1_hz > 950.0 {
        return false;
    }
    if iaptype >= 2 && seed.apsym[0] > 1 {
        return false;
    }
    if ncontest == 7 && iaptype >= 2 && seed.aph10[0] > 1 {
        return false;
    }
    if iaptype >= 3 && seed.apsym[29] > 1 {
        return false;
    }
    if iaptype == 5 && ncontest == 7 {
        return false;
    }

    *llrz_out = *llrz_base;
    apmask_out.fill(0);

    let apmag = llrz_base
        .iter()
        .map(|v| v.abs())
        .fold(0.0_f32, f32::max)
        * 1.1;

    match iaptype {
        1 => {
            let mcq_pat: &[i8; 29] = match ncontest {
                0 | 7 => &MCQ,
                1 | 2 | 8 => &MCQ_TEST,
                3 => &MCQ_FD,
                4 => &MCQ_RU,
                5 => &MCQ_WW,
                _ => return false,
            };
            apmask_out[0..29].fill(1);
            for i in 0..29 {
                llrz_out[i] = apmag * f32::from(mcq_pat[i]);
            }
            apmask_out[74..77].fill(1);
            llrz_out[74] = -apmag;
            llrz_out[75] = -apmag;
            llrz_out[76] = apmag;
            true
        }
        2 => match ncontest {
            0 | 1 | 5 | 8 => {
                apmask_out[0..29].fill(1);
                for i in 0..29 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = apmag;
                true
            }
            2 => {
                apmask_out[0..28].fill(1);
                for i in 0..28 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[71..74].fill(1);
                llrz_out[71] = -apmag;
                llrz_out[72] = apmag;
                llrz_out[73] = -apmag;
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = -apmag;
                true
            }
            3 => {
                apmask_out[0..28].fill(1);
                for i in 0..28 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = -apmag;
                true
            }
            4 => {
                apmask_out[1..29].fill(1);
                for i in 0..28 {
                    llrz_out[1 + i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = apmag;
                llrz_out[76] = apmag;
                true
            }
            7 => {
                apmask_out[28..56].fill(1);
                for i in 0..28 {
                    llrz_out[28 + i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[56..66].fill(1);
                for i in 0..10 {
                    llrz_out[56 + i] = apmag * f32::from(seed.aph10[i]);
                }
                apmask_out[71..77].fill(1);
                llrz_out[71] = -apmag;
                llrz_out[72] = -apmag;
                llrz_out[73] = apmag;
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = -apmag;
                true
            }
            _ => false,
        },
        3 => match ncontest {
            0 | 1 | 2 | 5 | 7 | 8 => {
                apmask_out[0..58].fill(1);
                for i in 0..58 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = apmag;
                true
            }
            3 => {
                apmask_out[0..56].fill(1);
                for i in 0..28 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                    llrz_out[28 + i] = apmag * f32::from(seed.apsym[29 + i]);
                }
                // `ft8b.f90:390-392`: masks 72:74 without overwriting their LLRs.
                apmask_out[71..74].fill(1);
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = -apmag;
                true
            }
            4 => {
                apmask_out[1..57].fill(1);
                for i in 0..28 {
                    llrz_out[1 + i] = apmag * f32::from(seed.apsym[i]);
                    llrz_out[29 + i] = apmag * f32::from(seed.apsym[29 + i]);
                }
                apmask_out[74..77].fill(1);
                llrz_out[74] = -apmag;
                llrz_out[75] = apmag;
                llrz_out[76] = apmag;
                true
            }
            _ => false,
        },
        4..=6 => {
            if ncontest <= 5 || (ncontest == 7 && iaptype == 6) || ncontest == 8 {
                apmask_out[0..77].fill(1);
                for i in 0..58 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                }
                let tail: &[i8; 19] = match iaptype {
                    4 => &MRRR,
                    5 => &M73,
                    _ => &MRR73,
                };
                for i in 0..19 {
                    llrz_out[58 + i] = apmag * f32::from(tail[i]);
                }
                true
            } else if ncontest == 7 && iaptype == 4 {
                apmask_out[0..28].fill(1);
                for i in 0..28 {
                    llrz_out[i] = apmag * f32::from(seed.apsym[i]);
                }
                apmask_out[56..66].fill(1);
                for i in 0..10 {
                    llrz_out[56 + i] = apmag * f32::from(seed.aph10[i]);
                }
                apmask_out[71..77].fill(1);
                llrz_out[71] = -apmag;
                llrz_out[72] = -apmag;
                llrz_out[73] = apmag;
                llrz_out[74] = -apmag;
                llrz_out[75] = -apmag;
                llrz_out[76] = -apmag;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Legacy Stage 2 stub alias retained for compatibility.
pub type ApContext = QsoContext;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApTag, ContestMode};

    #[test]
    fn static_cq_and_terminal_patterns_match_pack77() {
        let ht = CallsignHashTable::new();
        let uctx = UnpackContext::default();

        for (msg, expected) in [
            ("CQ K1ABC FN42", &MCQ),
            ("CQ RU K1ABC FN42", &MCQ_RU),
            ("CQ FD K1ABC FN42", &MCQ_FD),
            ("CQ TEST K1ABC FN42", &MCQ_TEST),
            ("CQ WW K1ABC FN42", &MCQ_WW),
        ] {
            let p = pack77(msg, None, &uctx, &ht).unwrap();
            let got: Vec<i8> = p.bits[..29]
                .iter()
                .map(|&b| if b != 0 { 1 } else { -1 })
                .collect();
            assert_eq!(got.as_slice(), expected.as_slice(), "mismatch for {msg}");
        }

        for (msg, expected) in [
            ("K1ABC W9XYZ RRR", &MRRR),
            ("K1ABC W9XYZ 73", &M73),
            ("K1ABC W9XYZ RR73", &MRR73),
        ] {
            let p = pack77(msg, None, &uctx, &ht).unwrap();
            let got: Vec<i8> = p.bits[58..77]
                .iter()
                .map(|&b| if b != 0 { 1 } else { -1 })
                .collect();
            assert_eq!(got.as_slice(), expected.as_slice(), "mismatch for {msg}");
        }
    }

    #[test]
    fn ap_seed_handles_standard_nonstandard_and_missing_calls() {
        let ht = CallsignHashTable::new();

        // Missing my_call => sentinels everywhere.
        let ctx_none = QsoContext::default();
        let s_none = compute_ap_seed(&ctx_none, &ht);
        assert_eq!(s_none.apsym[0], 99);
        assert_eq!(s_none.apsym[29], 99);
        assert_eq!(s_none.aph10[0], 99);

        // Standard my_call, missing his_call => apsym[0..29] valid, apsym[29]=99, aph10[0]=99.
        let ctx_my_only = QsoContext {
            my_call: Some("N0AAA".into()),
            ..Default::default()
        };
        let s_my = compute_ap_seed(&ctx_my_only, &ht);
        assert!(s_my.apsym[0] == -1 || s_my.apsym[0] == 1);
        assert_eq!(s_my.apsym[29], 99);
        assert_eq!(s_my.aph10[0], 99);

        // Standard my_call + nonstandard his_call in ContestMode::None => rejected (apsym[0]=99).
        let ctx_ns_his = QsoContext {
            my_call: Some("N0AAA".into()),
            his_call: Some("KH1/KH7Z".into()),
            contest: ContestMode::None,
            ..Default::default()
        };
        let s_ns = compute_ap_seed(&ctx_ns_his, &ht);
        assert_eq!(s_ns.apsym[0], 99);
        assert_eq!(s_ns.apsym[29], 99);

        // Nonstandard my_call + standard his_call => valid via `<my_call>` wrapping.
        let ctx_ns_my = QsoContext {
            my_call: Some("PJ4/N0AAA".into()),
            his_call: Some("K9BBB".into()),
            ..Default::default()
        };
        let s_ns_my = compute_ap_seed(&ctx_ns_my, &ht);
        assert!(s_ns_my.apsym[0] == -1 || s_ns_my.apsym[0] == 1);
        assert!(s_ns_my.apsym[29] == -1 || s_ns_my.apsym[29] == 1);
        assert!(s_ns_my.aph10[0] == -1 || s_ns_my.aph10[0] == 1);
    }

    #[test]
    fn ap_hypothesis_matrix_and_bailouts_cover_all_contests_and_progress() {
        let ht = CallsignHashTable::new();
        let base_llr = [0.5_f32; LDPC_N];
        let mut out_llr = [0.0_f32; LDPC_N];
        let mut apmask = [0u8; LDPC_N];

        let all_contests = [
            ContestMode::None,
            ContestMode::NaVhf,
            ContestMode::EuVhf,
            ContestMode::FieldDay,
            ContestMode::RttyRoundup,
            ContestMode::WwDigi,
            ContestMode::Fox,
            ContestMode::Hound,
            ContestMode::ArrlDigi,
        ];

        for contest in all_contests {
            for prog in 0..=5u8 {
                let ctx = QsoContext {
                    my_call: Some("N0AAA".into()),
                    his_call: Some("K9BBB".into()),
                    qso_progress: prog,
                    rx_freq_hz: 1500.0,
                    tx_freq_hz: 1500.0,
                    ap_enabled: true,
                    ap_cq_only: false,
                    ap_width_hz: 75.0,
                    contest,
                    ..Default::default()
                };
                let seed = compute_ap_seed(&ctx, &ht);
                let npasses = ft8b_pass_count(Some(&ctx), 50);
                assert!((9..=13).contains(&npasses));
                assert_eq!(ft8b_pass_count(Some(&ctx), 41), 5);

                for ipass in 6..=npasses {
                    if let Ft8bPassPlan::Ap { iaptype, .. } = ft8b_pass_plan(ipass, Some(&ctx)) {
                        let applied_low = apply_ap_hypothesis(
                            iaptype,
                            &ctx,
                            &seed,
                            600.0,
                            &base_llr,
                            &mut out_llr,
                            &mut apmask,
                        );
                        let applied_rx = apply_ap_hypothesis(
                            iaptype,
                            &ctx,
                            &seed,
                            1500.0,
                            &base_llr,
                            &mut out_llr,
                            &mut apmask,
                        );
                        match contest {
                            ContestMode::Fox => {
                                assert!(!applied_low);
                                assert!(!applied_rx);
                            }
                            ContestMode::Hound => {
                                assert!(!applied_rx, "Hound AP must bail above 950 Hz");
                                if iaptype == 5 {
                                    assert!(!applied_low, "Hound must skip a5 (73)");
                                } else {
                                    assert!(applied_low);
                                }
                            }
                            _ => {
                                if iaptype >= 3 && (contest as u8) <= 5 {
                                    assert!(!applied_low, "a3..a6 must bail outside napwid");
                                }
                                assert!(applied_rx);
                            }
                        }
                    }
                }
            }
        }

        // Verify ApTag boundary at qual = 0.17 vs 0.169 (digi-dx reconciler parity).
        assert_eq!(ApTag::from_wsjtx(0, 0.0).wsjtx_metadata(), None);
        assert_eq!(
            ApTag::from_wsjtx(1, 0.169).wsjtx_metadata().as_deref(),
            Some("? a1")
        );
        assert_eq!(
            ApTag::from_wsjtx(1, 0.170).wsjtx_metadata().as_deref(),
            Some("a1")
        );
    }
}
