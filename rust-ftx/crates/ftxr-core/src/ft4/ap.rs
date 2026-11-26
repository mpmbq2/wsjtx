// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 A Priori (`AP`) hypothesis masks (`a1`–`a3`, `a6`) in `FT4_RVEC`-scrambled bit domain
//! matching `wsjtx/lib/ft4_decode.f90`.

use crate::fec::bp::LDPC_N;
use crate::protocol::hashtable::CallsignHashTable;
use crate::protocol::ldpc::encode174_91;
use crate::protocol::pack77::pack77;
use crate::protocol::rvec::FT4_RVEC;
use crate::protocol::types::UnpackContext;
use crate::{ContestMode, QsoContext};

use super::params::NAPWID_HZ;

/// Active FT4 AP pass hypothesis (`wsjtx/lib/ft4_decode.f90`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ft4ApPassType {
    /// Pass 1..=3: no AP priors (`llra`, `llrb`, `llrc`).
    None = 0,
    /// `a1`: `CQ` (or contest `CQ TEST`/`CQ FD`/`CQ RU`/`CQ WW`), bits `0..29`.
    Cq = 1,
    /// `a2`: `MyCall` alone (any frequency), bits `0..28` (or `1..29` for RTTY RU).
    MyCall = 2,
    /// `a3`: `MyCall + HisCall` (`|f1 - rx_freq_hz| <= 50 Hz`), bits `0..58` (`0..28` + `28..56` FD, `1..29` + `29..57` RU).
    MyCallHisCall = 3,
    /// `a6`: `MyCall + HisCall + RR73` (`|f1 - rx_freq_hz| <= 50 Hz`), all 77 message bits `0..77`.
    MyCallHisCallRr73 = 6,
}

/// Pass descriptor returned by [`ft4_ap_schedule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ft4PassSpec {
    /// Which base bit-metric column to initialize (`0 = llra`, `1 = llrb`, `2 = llrc`).
    pub llr_col: usize,
    /// Active AP hypothesis (`None` for blind passes `0..3`).
    pub ap_type: Ft4ApPassType,
}

/// Precomputed static CQ prefix bit patterns (`0..29`) already XORed with `FT4_RVEC[0..29]`.
#[derive(Debug, Clone)]
pub struct Ft4ApTables {
    mcq: [f32; 29],
    mcq_test: [f32; 29],
    mcq_fd: [f32; 29],
    mcq_ru: [f32; 29],
    mcq_ww: [f32; 29],
}

impl Default for Ft4ApTables {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4ApTables {
    pub fn new() -> Self {
        Self {
            mcq: pack_scrambled_cq_prefix("CQ K1ABC FN42"),
            mcq_test: pack_scrambled_cq_prefix("CQ TEST K1ABC FN42"),
            mcq_fd: pack_scrambled_cq_prefix("CQ FD K1ABC FN42"),
            mcq_ru: pack_scrambled_cq_prefix("CQ RU K1ABC FN42"),
            mcq_ww: pack_scrambled_cq_prefix("CQ WW K1ABC FN42"),
        }
    }

    /// Selects the 29-bit scrambled CQ prefix for `contest`.
    pub fn cq_prefix_for(&self, contest: ContestMode) -> &[f32; 29] {
        match contest {
            ContestMode::NaVhf | ContestMode::EuVhf => &self.mcq_test,
            ContestMode::FieldDay => &self.mcq_fd,
            ContestMode::RttyRoundup => &self.mcq_ru,
            ContestMode::WwDigi => &self.mcq_ww,
            ContestMode::None | ContestMode::Fox | ContestMode::Hound | ContestMode::ArrlDigi => {
                &self.mcq
            }
        }
    }
}

fn pack_scrambled_cq_prefix(sample_msg: &str) -> [f32; 29] {
    let ht = CallsignHashTable::new();
    let ctx = UnpackContext::default();
    let packed = pack77(sample_msg, None, &ctx, &ht).expect("static CQ message must pack");
    let mut out = [0.0_f32; 29];
    for (idx, dst) in out.iter_mut().enumerate() {
        let bit = packed.bits[idx] ^ FT4_RVEC[idx];
        *dst = if bit == 1 { 1.0 } else { -1.0 };
    }
    out
}

/// Slot-level dynamic FT4 AP priors derived once per decode call from [`QsoContext`].
#[derive(Debug, Clone)]
pub struct Ft4ApSet {
    pub enabled: bool,
    pub cq_only: bool,
    pub contest: ContestMode,
    pub qso_progress: u8,
    pub rx_freq_hz: f32,
    pub no_my_call: bool,
    pub no_his_call: bool,
    /// Scrambled LDPC codeword bipolar values (`-1.0` / `+1.0`, or `apbits[29] == 99.0` if `!has_his_call`).
    pub apbits: [f32; LDPC_N],
    /// `mycall` bits shifted into Type 3 RTTY Roundup slot `[1..29]` (`msg77[0..28] ^ FT4_RVEC[1..29]`).
    pub apmy_ru: [f32; 28],
    /// `hiscall` bits shifted into Type 0.3 Field Day slot `[28..56]` (`msg77[29..57] ^ FT4_RVEC[28..56]`).
    pub aphis_fd: [f32; 28],
}

impl Ft4ApSet {
    /// Builds FT4 AP priors matching `wsjtx/lib/ft4_decode.f90:155-205`.
    pub fn from_context(qso: Option<&QsoContext>, hash_table: &CallsignHashTable) -> Self {
        let Some(qso) = qso else {
            return Self {
                enabled: false,
                cq_only: false,
                contest: ContestMode::None,
                qso_progress: 0,
                rx_freq_hz: 1500.0,
                no_my_call: true,
                no_his_call: true,
                apbits: [0.0; LDPC_N],
                apmy_ru: [0.0; 28],
                aphis_fd: [0.0; 28],
            };
        };

        let my_trimmed = qso
            .my_call
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        let his_trimmed = qso
            .his_call
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();

        let no_my_call = my_trimmed.len() < 3;
        let no_his_call = his_trimmed.len() < 3;

        // If `my_call` is blank, use a dummy standard callsign so `hiscall` can still pack if ever needed,
        // while `no_my_call = true` suppresses a2/a3/a6 identically to `ft4_decode.f90`.
        let my_effective = if no_my_call {
            "K1ABC".to_string()
        } else {
            my_trimmed
        };
        let his_effective = if no_his_call {
            my_effective.clone()
        } else {
            his_trimmed
        };

        let msg_rr73 = format!("{my_effective} {his_effective} RR73");
        let unpack_ctx = UnpackContext {
            my_call: qso.my_call.clone(),
            dx_call: qso.his_call.clone(),
            dx_base: None,
        };

        let mut apbits = [0.0_f32; LDPC_N];
        let mut apmy_ru = [0.0_f32; 28];
        let mut aphis_fd = [0.0_f32; 28];

        let packed_opt = pack77(&msg_rr73, None, &unpack_ctx, hash_table)
            .ok()
            .or_else(|| {
                // Fallback if compound callsign didn't support "RR73" hash form -> pack plain call1/call2
                let msg_plain = format!("{my_effective} {his_effective} AA00");
                pack77(&msg_plain, None, &unpack_ctx, hash_table).ok()
            });

        if let Some(packed) = packed_opt {
            let mut scrambled = packed.bits;
            for (b, &r) in scrambled.iter_mut().zip(FT4_RVEC.iter()) {
                *b ^= r;
            }
            let cw = encode174_91(&scrambled);
            for (dst, &c) in apbits.iter_mut().zip(cw.iter()) {
                *dst = if c == 1 { 1.0 } else { -1.0 };
            }
            for i in 0..28 {
                let ru_bit = packed.bits[i] ^ FT4_RVEC[i + 1];
                apmy_ru[i] = if ru_bit == 1 { 1.0 } else { -1.0 };
                let fd_bit = packed.bits[29 + i] ^ FT4_RVEC[28 + i];
                aphis_fd[i] = if fd_bit == 1 { 1.0 } else { -1.0 };
            }
        } else {
            return Self {
                enabled: qso.ap_enabled,
                cq_only: true,
                contest: qso.contest,
                qso_progress: qso.qso_progress.min(5),
                rx_freq_hz: qso.rx_freq_hz,
                no_my_call: true,
                no_his_call: true,
                apbits,
                apmy_ru,
                aphis_fd,
            };
        }

        if no_his_call {
            apbits[29] = 99.0;
        }

        Self {
            enabled: qso.ap_enabled,
            cq_only: qso.ap_cq_only,
            contest: qso.contest,
            qso_progress: qso.qso_progress.min(5),
            rx_freq_hz: qso.rx_freq_hz,
            no_my_call,
            no_his_call,
            apbits,
            apmy_ru,
            aphis_fd,
        }
    }
}

/// Returns the ordered list of blind (`1..=3`) and optional AP passes matching
/// `wsjtx/lib/ft4_decode.f90:145-153,328-347`.
pub fn ft4_ap_schedule(depth: u8, ap_set: &Ft4ApSet) -> Vec<Ft4PassSpec> {
    let mut passes = vec![
        Ft4PassSpec {
            llr_col: 0,
            ap_type: Ft4ApPassType::None,
        },
        Ft4PassSpec {
            llr_col: 1,
            ap_type: Ft4ApPassType::None,
        },
        Ft4PassSpec {
            llr_col: 2,
            ap_type: Ft4ApPassType::None,
        },
    ];

    if depth <= 1 || !ap_set.enabled {
        return passes;
    }

    let qso_passes: &[Ft4ApPassType] = match ap_set.qso_progress {
        0 => &[Ft4ApPassType::Cq, Ft4ApPassType::MyCall],
        1 | 2 => &[Ft4ApPassType::MyCall, Ft4ApPassType::MyCallHisCall],
        3 | 4 => &[
            Ft4ApPassType::MyCallHisCall,
            Ft4ApPassType::MyCallHisCallRr73,
        ],
        _ => &[
            Ft4ApPassType::MyCallHisCall,
            Ft4ApPassType::Cq,
            Ft4ApPassType::MyCall,
        ],
    };

    for &ap_type in qso_passes {
        passes.push(Ft4PassSpec {
            llr_col: 2,
            ap_type,
        });
    }
    passes
}

/// Populates `llr` and `apmask` for a single pass of the FT4 candidate decode loop.
///
/// Returns `false` if this AP pass is inapplicable given QSO state or frequency tolerance
/// (`±50 Hz` around `ap_set.rx_freq_hz` for `a3`/`a6`).
pub fn build_ft4_ap_pass(
    spec: Ft4PassSpec,
    llrs: &[[f32; LDPC_N]; 3],
    f1_hz: f32,
    tables: &Ft4ApTables,
    ap_set: &Ft4ApSet,
    llr_out: &mut [f32; LDPC_N],
    apmask_out: &mut [u8; LDPC_N],
) -> bool {
    *llr_out = llrs[spec.llr_col];
    apmask_out.fill(0);

    if spec.ap_type == Ft4ApPassType::None {
        return true;
    }

    let apmag = llrs[0]
        .iter()
        .fold(0.0_f32, |m, &v| m.max(v.abs()))
        * 1.1;

    match spec.ap_type {
        Ft4ApPassType::None => unreachable!(),
        Ft4ApPassType::Cq => {
            let prefix = tables.cq_prefix_for(ap_set.contest);
            apmask_out[..29].fill(1);
            for i in 0..29 {
                llr_out[i] = apmag * prefix[i];
            }
            true
        }
        Ft4ApPassType::MyCall => {
            if ap_set.cq_only || ap_set.no_my_call {
                return false;
            }
            if ap_set.contest == ContestMode::RttyRoundup {
                apmask_out[1..29].fill(1);
                for i in 0..28 {
                    llr_out[1 + i] = apmag * ap_set.apmy_ru[i];
                }
            } else {
                apmask_out[..28].fill(1);
                for i in 0..28 {
                    llr_out[i] = apmag * ap_set.apbits[i];
                }
            }
            true
        }
        Ft4ApPassType::MyCallHisCall => {
            if ap_set.cq_only
                || ap_set.no_my_call
                || ap_set.no_his_call
                || (f1_hz - ap_set.rx_freq_hz).abs() > NAPWID_HZ
            {
                return false;
            }
            match ap_set.contest {
                ContestMode::FieldDay => {
                    apmask_out[..28].fill(1);
                    apmask_out[28..56].fill(1);
                    for i in 0..28 {
                        llr_out[i] = apmag * ap_set.apbits[i];
                        llr_out[28 + i] = apmag * ap_set.aphis_fd[i];
                    }
                }
                ContestMode::RttyRoundup => {
                    apmask_out[1..29].fill(1);
                    apmask_out[29..57].fill(1);
                    for i in 0..28 {
                        llr_out[1 + i] = apmag * ap_set.apmy_ru[i];
                        llr_out[29 + i] = apmag * ap_set.apbits[29 + i];
                    }
                }
                _ => {
                    // Matches Fortran `apmask(1:58)=1` on standard messages (`ipa`/`ipb`/both callsigns).
                    apmask_out[..58].fill(1);
                    for i in 0..58 {
                        llr_out[i] = apmag * ap_set.apbits[i];
                    }
                }
            }
            true
        }
        Ft4ApPassType::MyCallHisCallRr73 => {
            if ap_set.cq_only
                || ap_set.no_my_call
                || ap_set.no_his_call
                || (f1_hz - ap_set.rx_freq_hz).abs() > NAPWID_HZ
            {
                return false;
            }
            // Note Fortran sets `apmask(1:77)=1` and `llrd(1:77)=apmag*apbits(1:77)`.
            apmask_out[..77].fill(1);
            for i in 0..77 {
                llr_out[i] = apmag * ap_set.apbits[i];
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rvec::descramble_ft4;

    #[test]
    fn ap_schedule_matches_depth_and_qso_progress() {
        let ht = CallsignHashTable::new();
        let qso_base = QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            ap_enabled: true,
            ..Default::default()
        };

        let set_d0 = Ft4ApSet::from_context(Some(&qso_base), &ht);
        assert_eq!(ft4_ap_schedule(1, &set_d0).len(), 3);

        for (prog, expected) in [
            (
                0u8,
                vec![
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::Cq,
                    Ft4ApPassType::MyCall,
                ],
            ),
            (
                1,
                vec![
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::MyCall,
                    Ft4ApPassType::MyCallHisCall,
                ],
            ),
            (
                3,
                vec![
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::MyCallHisCall,
                    Ft4ApPassType::MyCallHisCallRr73,
                ],
            ),
            (
                5,
                vec![
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::None,
                    Ft4ApPassType::MyCallHisCall,
                    Ft4ApPassType::Cq,
                    Ft4ApPassType::MyCall,
                ],
            ),
        ] {
            let mut q = qso_base.clone();
            q.qso_progress = prog;
            let set = Ft4ApSet::from_context(Some(&q), &ht);
            let sched: Vec<_> = ft4_ap_schedule(3, &set)
                .into_iter()
                .map(|p| p.ap_type)
                .collect();
            assert_eq!(sched, expected, "qso_progress={prog}");
        }
    }

    #[test]
    fn ap_masks_match_rvec_domain_for_standard_fd_and_ru() {
        let ht = CallsignHashTable::new();
        let tables = Ft4ApTables::new();
        let llrs = [[2.0_f32; LDPC_N]; 3];
        let mut out_llr = [0.0_f32; LDPC_N];
        let mut mask = [0u8; LDPC_N];

        // 1. Standard QSO inside vs outside ±50 Hz
        let q_std = QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 3,
            rx_freq_hz: 1500.0,
            ap_enabled: true,
            contest: ContestMode::None,
            ..Default::default()
        };
        let set_std = Ft4ApSet::from_context(Some(&q_std), &ht);
        assert!(!build_ft4_ap_pass(
            Ft4PassSpec {
                llr_col: 2,
                ap_type: Ft4ApPassType::MyCallHisCall,
            },
            &llrs,
            1551.0,
            &tables,
            &set_std,
            &mut out_llr,
            &mut mask,
        ));
        assert!(build_ft4_ap_pass(
            Ft4PassSpec {
                llr_col: 2,
                ap_type: Ft4ApPassType::MyCallHisCallRr73,
            },
            &llrs,
            1525.0,
            &tables,
            &set_std,
            &mut out_llr,
            &mut mask,
        ));
        assert_eq!(mask[..77], [1u8; 77]);
        assert_eq!(mask[77..], [0u8; LDPC_N - 77]);

        // Unscrambling the hardcoded LLRs recovers the exact unscrambled RR73 message.
        let mut raw_bits77 = [0u8; 77];
        for i in 0..77 {
            raw_bits77[i] = u8::from(out_llr[i] > 0.0);
        }
        let bits77 = descramble_ft4(&raw_bits77);
        let packed = pack77(
            "K1ABC W9XYZ RR73",
            None,
            &UnpackContext::default(),
            &ht,
        )
        .unwrap();
        assert_eq!(bits77, packed.bits);

        // 2. RTTY Roundup shifts mycall into 1..29 and FD shifts hiscall into 28..56
        let mut q_ru = q_std.clone();
        q_ru.contest = ContestMode::RttyRoundup;
        let set_ru = Ft4ApSet::from_context(Some(&q_ru), &ht);
        assert!(build_ft4_ap_pass(
            Ft4PassSpec {
                llr_col: 2,
                ap_type: Ft4ApPassType::MyCall,
            },
            &llrs,
            2200.0,
            &tables,
            &set_ru,
            &mut out_llr,
            &mut mask,
        ));
        assert_eq!(mask[0], 0);
        assert_eq!(mask[1..29], [1u8; 28]);
        assert_eq!(mask[29], 0);

        let mut q_fd = q_std;
        q_fd.contest = ContestMode::FieldDay;
        let set_fd = Ft4ApSet::from_context(Some(&q_fd), &ht);
        assert!(build_ft4_ap_pass(
            Ft4PassSpec {
                llr_col: 2,
                ap_type: Ft4ApPassType::MyCallHisCall,
            },
            &llrs,
            1500.0,
            &tables,
            &set_fd,
            &mut out_llr,
            &mut mask,
        ));
        assert_eq!(mask[..56], [1u8; 56]);
        assert_eq!(mask[56], 0);
    }
}
