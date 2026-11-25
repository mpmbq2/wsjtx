// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Differential codec verification against `ft8code` and `ft4code` (`codec-diff`).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use ftxr_core::modulate::ft4::encode_ft4_tones_from_77bits;
use ftxr_core::modulate::ft8::encode_ft8_tones_from_77bits;
use ftxr_core::protocol::crc14::compute_crc14;
use ftxr_core::protocol::hashtable::CallsignHashTable;
use ftxr_core::protocol::ldpc::encode174_91;
use ftxr_core::protocol::pack77::{fmtmsg, pack77};
use ftxr_core::protocol::rvec::scramble_ft4;
use ftxr_core::protocol::types::{UnpackContext, ARRL_SECTIONS, RTTY_MULTS};
use ftxr_core::Mode;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use thiserror::Error;

use crate::sim::{SimError, SimRunner};

#[derive(Debug, Error)]
pub enum CodecOracleError {
    #[error("simulator error: {0}")]
    Sim(#[from] SimError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to parse {tool} output for {msg:?}: {reason}")]
    Parse {
        tool: &'static str,
        msg: String,
        reason: String,
    },
    #[error("codec differential mismatch ({mismatches} failure(s)); first: {first_detail}")]
    Mismatch {
        mismatches: usize,
        first_detail: String,
    },
}

#[derive(Debug, Serialize)]
pub struct CodecDiffReport {
    pub seed: u64,
    pub per_mode_fuzz_count: usize,
    pub elapsed_ms: f64,
    pub ft8_golden_vectors: usize,
    pub ft4_golden_vectors: usize,
    pub ft8_fuzz_checked: usize,
    pub ft4_fuzz_checked: usize,
    pub ft8_type_histogram: BTreeMap<String, usize>,
    pub ft4_type_histogram: BTreeMap<String, usize>,
    pub mismatches: usize,
}

#[derive(Debug)]
struct GoldenRow {
    msg: String,
    msgsent: String,
    bad: bool,
    i3: u8,
    n3: u8,
}

#[derive(Debug)]
struct SingleOracleCode {
    msgsent: String,
    i3: u8,
    n3: u8,
    msg77_unscrambled: [u8; 77],
    msg77_scrambled: Option<[u8; 77]>,
    crc14: [u8; 14],
    parity83: [u8; 83],
    tones: Vec<u8>,
}

fn parse_golden_table(stdout: &str, tool: &'static str) -> Result<Vec<GoldenRow>, CodecOracleError> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < 86 || chars[2] != '.' {
            continue;
        }
        if !chars[0..2].iter().all(|c| c.is_ascii_digit() || *c == ' ') {
            continue;
        }
        let msg: String = chars[4..41].iter().collect();
        let msgsent: String = chars[42..79].iter().collect();
        let bad = chars[80] == '*';
        let i3 = chars[83]
            .to_digit(10)
            .ok_or_else(|| CodecOracleError::Parse {
                tool,
                msg: line.to_string(),
                reason: "invalid i3 digit".into(),
            })? as u8;
        let n3 = if i3 == 0 {
            chars[85]
                .to_digit(10)
                .ok_or_else(|| CodecOracleError::Parse {
                    tool,
                    msg: line.to_string(),
                    reason: "invalid n3 digit".into(),
                })? as u8
        } else {
            0
        };
        rows.push(GoldenRow {
            msg: msg.trim_end().to_string(),
            msgsent: msgsent.trim_end().to_string(),
            bad,
            i3,
            n3,
        });
    }
    if rows.is_empty() {
        return Err(CodecOracleError::Parse {
            tool,
            msg: "<table?>".into(),
            reason: "no rows parsed".into(),
        });
    }
    Ok(rows)
}

fn parse_bit_array<const N: usize>(line: &str, tool: &'static str, msg: &str) -> Result<[u8; N], CodecOracleError> {
    let trimmed = line.trim();
    if trimmed.len() != N {
        return Err(CodecOracleError::Parse {
            tool,
            msg: msg.to_string(),
            reason: format!("expected {N} bits, got {} ({trimmed:?})", trimmed.len()),
        });
    }
    let mut out = [0u8; N];
    for (i, ch) in trimmed.bytes().enumerate() {
        out[i] = match ch {
            b'0' => 0,
            b'1' => 1,
            _ => {
                return Err(CodecOracleError::Parse {
                    tool,
                    msg: msg.to_string(),
                    reason: format!("non-binary bit at {i}"),
                })
            }
        };
    }
    Ok(out)
}

fn parse_single_oracle(
    stdout: &str,
    mode: Mode,
    msg: &str,
) -> Result<SingleOracleCode, CodecOracleError> {
    let tool = match mode {
        Mode::Ft8 => "ft8code",
        Mode::Ft4 => "ft4code",
    };
    let rows = parse_golden_table(stdout, tool)?;
    let row = &rows[0];

    let lines: Vec<&str> = stdout.lines().collect();
    let find_next = |needle: &str| -> Result<&str, CodecOracleError> {
        let pos = lines
            .iter()
            .position(|l| l.contains(needle))
            .ok_or_else(|| CodecOracleError::Parse {
                tool,
                msg: msg.to_string(),
                reason: format!("missing section {needle:?}"),
            })?;
        lines
            .get(pos + 1)
            .copied()
            .ok_or_else(|| CodecOracleError::Parse {
                tool,
                msg: msg.to_string(),
                reason: format!("missing value line after {needle:?}"),
            })
    };

    let (msg77_unscrambled, msg77_scrambled) = match mode {
        Mode::Ft8 => {
            let l = find_next("Source-encoded message, 77 bits:")?;
            (parse_bit_array::<77>(l, tool, msg)?, None)
        }
        Mode::Ft4 => {
            let l_un = find_next("Source-encoded message before scrambling, 77 bits:")?;
            let l_sc = find_next("Source-encoded message after scrambling, 77 bits:")?;
            (
                parse_bit_array::<77>(l_un, tool, msg)?,
                Some(parse_bit_array::<77>(l_sc, tool, msg)?),
            )
        }
    };

    let crc14 = parse_bit_array::<14>(find_next("14-bit CRC:")?, tool, msg)?;
    let parity83 = parse_bit_array::<83>(find_next("83 Parity bits:")?, tool, msg)?;

    let tone_hdr = match mode {
        Mode::Ft8 => "Channel symbols (79 tones):",
        Mode::Ft4 => "Channel symbols (105 tones):",
    };
    let hdr_pos = lines
        .iter()
        .position(|l| l.contains(tone_hdr))
        .ok_or_else(|| CodecOracleError::Parse {
            tool,
            msg: msg.to_string(),
            reason: format!("missing {tone_hdr:?}"),
        })?;
    let tone_line = lines
        .get(hdr_pos + 2)
        .copied()
        .ok_or_else(|| CodecOracleError::Parse {
            tool,
            msg: msg.to_string(),
            reason: "missing tone line".into(),
        })?;

    let mut tones: Vec<u8> = Vec::new();
    for ch in tone_line.bytes() {
        if ch.is_ascii_digit() {
            tones.push(ch - b'0');
        }
    }
    match mode {
        Mode::Ft8 => {
            if tones.len() != 79 {
                return Err(CodecOracleError::Parse {
                    tool,
                    msg: msg.to_string(),
                    reason: format!("expected 79 FT8 tones, got {}", tones.len()),
                });
            }
        }
        Mode::Ft4 => {
            if tones.len() != 105 {
                return Err(CodecOracleError::Parse {
                    tool,
                    msg: msg.to_string(),
                    reason: format!("expected 105 FT4 tones, got {}", tones.len()),
                });
            }
            // Strip leading and trailing ramp '0' symbols to compare 103 data/sync symbols.
            tones = tones[1..104].to_vec();
        }
    }

    Ok(SingleOracleCode {
        msgsent: row.msgsent.clone(),
        i3: row.i3,
        n3: row.n3,
        msg77_unscrambled,
        msg77_scrambled,
        crc14,
        parity83,
        tones,
    })
}

fn rand_std_call(rng: &mut ChaCha8Rng) -> String {
    let pfx_style = rng.gen_range(0..4);
    let mut s = String::new();
    let letter = |r: &mut ChaCha8Rng| (b'A' + r.gen_range(0..26)) as char;
    let alnum = |r: &mut ChaCha8Rng| {
        let v = r.gen_range(0..36);
        if v < 10 {
            (b'0' + v) as char
        } else {
            (b'A' + (v - 10)) as char
        }
    };
    match pfx_style {
        0 => {
            // 1 letter (not Q) + digit
            let mut c = letter(rng);
            while c == 'Q' {
                c = letter(rng);
            }
            s.push(c);
            s.push((b'0' + rng.gen_range(0..10)) as char);
        }
        1 => {
            // 2 letters (not Q) + digit
            let mut c1 = letter(rng);
            while c1 == 'Q' {
                c1 = letter(rng);
            }
            s.push(c1);
            s.push(letter(rng));
            s.push((b'0' + rng.gen_range(0..10)) as char);
        }
        2 => {
            // letter (not Q) + alnum + digit
            let mut c1 = letter(rng);
            while c1 == 'Q' {
                c1 = letter(rng);
            }
            s.push(c1);
            s.push(alnum(rng));
            s.push((b'0' + rng.gen_range(0..10)) as char);
        }
        _ => {
            // digit + letter + digit
            s.push((b'0' + rng.gen_range(0..10)) as char);
            s.push(letter(rng));
            s.push((b'0' + rng.gen_range(0..10)) as char);
        }
    }
    let sfx_len = rng.gen_range(1..=3);
    for _ in 0..sfx_len {
        s.push(letter(rng));
    }
    s
}

fn rand_nonstd_call(rng: &mut ChaCha8Rng) -> String {
    match rng.gen_range(0..4) {
        0 => {
            let pfx_len = rng.gen_range(2..=4);
            let mut p = String::new();
            for i in 0..pfx_len {
                if i == pfx_len - 1 {
                    p.push((b'0' + rng.gen_range(0..10)) as char);
                } else {
                    p.push((b'A' + rng.gen_range(0..26)) as char);
                }
            }
            format!("{p}/{}", rand_std_call(rng))
        }
        1 => format!("{}/QRP", rand_std_call(rng)),
        2 => {
            // Special event callsign (>6 chars, no slash)
            format!(
                "YW{}{}FIFA",
                rng.gen_range(1..10),
                rng.gen_range(0..10)
            )
        }
        _ => format!(
            "3DA0{}{}",
            (b'A' + rng.gen_range(0..26)) as char,
            (b'A' + rng.gen_range(0..26)) as char
        ),
    }
}

fn rand_grid4(rng: &mut ChaCha8Rng) -> String {
    format!(
        "{}{}{}{}",
        (b'A' + rng.gen_range(0..18)) as char,
        (b'A' + rng.gen_range(0..18)) as char,
        rng.gen_range(0..10),
        rng.gen_range(0..10),
    )
}

fn rand_grid6(rng: &mut ChaCha8Rng) -> String {
    format!(
        "{}{}{}",
        rand_grid4(rng),
        (b'A' + rng.gen_range(0..24)) as char,
        (b'A' + rng.gen_range(0..24)) as char,
    )
}

fn adversarial_messages() -> Vec<String> {
    vec![
        "K1ABC W9XYZ RR73".into(),
        "CQ KH1/KH7Z".into(),
        "K1ABC FN42 30".into(),
        "K1ABC FN42 73".into(),
        "PJ4/K1ABC 10".into(),
        "K1ABC/QRP 23".into(),
        "K1ABC/7A 37".into(),
        "CQ ABCD".into(),
        "CQ DX KH1/KH7Z".into(),
        "3DA0XYZ W9XYZ -10".into(),
        "  cq   k1abc   fn42  ".into(),
        "0".into(),
        "7FFFFFFFFFFFFFFFFF".into(),
        "800000000000000000".into(),
        "000000000000000001".into(),
        "CQ 000 K1ABC FN42".into(),
        "CQ 999 K1ABC FN42".into(),
        "CQ ZZZZ K1ABC FN42".into(),
        "DE K1ABC FN42".into(),
        "QRZ K1ABC FN42".into(),
        "K1ABC/R W9XYZ/R R -50".into(),
        "K1ABC W9XYZ R+49".into(),
        "PA3XYZ/P GM4ABC/P R JO22".into(),
        "TU; W9XYZ K1ABC R 599 DX".into(),
        "W9XYZ K1ABC 529 7999".into(),
        "<W9XYZ> PJ4/K1ABC 73".into(),
        "PJ4/K1ABC <W9XYZ> RRR".into(),
        "<PA3XYZ> <G4ABC/P> R 594095 IO91NP".into(),
        "K1ABC RR73; W9XYZ <KH1/KH7Z> -30".into(),
        "K1ABC RR73; W9XYZ <KH1/KH7Z> +32".into(),
        "A B C +-./? 9".into(),
    ]
}

fn generate_fuzz_messages(seed: u64, count: usize) -> Vec<String> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut msgs = adversarial_messages();

    while msgs.len() < count {
        let idx = msgs.len();
        let kind = idx % 10;
        let m = match kind {
            // 0.0 Free text
            0 => {
                const FT: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?";
                let len = rng.gen_range(1..=13);
                let mut s = String::with_capacity(len);
                for i in 0..len {
                    let ch = FT[rng.gen_range(0..FT.len())] as char;
                    if i == 0 && ch == ' ' {
                        s.push('A');
                    } else {
                        s.push(ch);
                    }
                }
                if s.trim().is_empty() {
                    "73 GL".to_string()
                } else {
                    s
                }
            }
            // 0.1 DXpedition
            1 => {
                let c1 = rand_std_call(&mut rng);
                let c2 = rand_std_call(&mut rng);
                let fox = rand_nonstd_call(&mut rng);
                let rpt = rng.gen_range(-30..=32);
                format!("{c1} RR73; {c2} <{fox}> {rpt:+03}")
            }
            // 0.3 / 0.4 Field Day
            2 => {
                let c1 = rand_std_call(&mut rng);
                let c2 = rand_std_call(&mut rng);
                let ntx = rng.gen_range(1..=32);
                let class = (b'A' + rng.gen_range(0..8)) as char;
                let sec = ARRL_SECTIONS[rng.gen_range(0..ARRL_SECTIONS.len())];
                let r = if rng.gen_bool(0.5) { "R " } else { "" };
                format!("{c1} {c2} {r}{ntx}{class} {sec}")
            }
            // 0.5 Telemetry
            3 => {
                if rng.gen_bool(0.15) {
                    "800000000000000000".to_string()
                } else {
                    let t0: u32 = rng.gen_range(0..(1 << 23));
                    let t1: u32 = rng.gen_range(0..(1 << 24));
                    let t2: u32 = rng.gen_range(0..(1 << 24));
                    let raw = format!("{t0:06X}{t1:06X}{t2:06X}");
                    let trimmed = raw.trim_start_matches('0');
                    if trimmed.is_empty() {
                        "0".to_string()
                    } else {
                        trimmed.to_string()
                    }
                }
            }
            // 0.6 WSPR Type 1 & 2
            4 => {
                let dbm = rng.gen_range(0..=60);
                if rng.gen_bool(0.5) {
                    format!("{} {} {dbm}", rand_std_call(&mut rng), rand_grid4(&mut rng))
                } else if rng.gen_bool(0.5) {
                    let pfx = format!("W{}", rng.gen_range(0..10));
                    format!("{pfx}/{} {dbm}", rand_std_call(&mut rng))
                } else {
                    format!("{}/P {dbm}", rand_std_call(&mut rng))
                }
            }
            // 1 Standard msg
            5 => {
                let w1 = match rng.gen_range(0..6) {
                    0 => "CQ".to_string(),
                    1 => format!("CQ {:03}", rng.gen_range(0..1000)),
                    2 => {
                        let len = rng.gen_range(1..=4);
                        let mut dir = String::from("CQ ");
                        for _ in 0..len {
                            dir.push((b'A' + rng.gen_range(0..26)) as char);
                        }
                        dir
                    }
                    3 => "DE".to_string(),
                    4 => "QRZ".to_string(),
                    _ => {
                        let mut c = rand_std_call(&mut rng);
                        if rng.gen_bool(0.2) {
                            c.push_str("/R");
                        }
                        c
                    }
                };
                let mut w2 = if rng.gen_bool(0.15) && w1 != "CQ" && !w1.starts_with("CQ ") && !w1.contains('/') {
                    format!("<{}>", rand_nonstd_call(&mut rng))
                } else {
                    let mut c = rand_std_call(&mut rng);
                    if rng.gen_bool(0.2) && !w1.starts_with('<') {
                        c.push_str("/R");
                    }
                    c
                };
                if w1.starts_with('<') && w2.contains('/') {
                    w2 = rand_std_call(&mut rng);
                }
                match rng.gen_range(0..6) {
                    0 => {
                        if w2.contains('/') {
                            format!("{w1} {}", rand_std_call(&mut rng))
                        } else {
                            format!("{w1} {w2}")
                        }
                    }
                    1 => {
                        let r = if rng.gen_bool(0.3) && w1 != "CQ" && !w1.starts_with("CQ ") {
                            "R "
                        } else {
                            ""
                        };
                        format!("{w1} {w2} {r}{}", rand_grid4(&mut rng))
                    }
                    2 if !w1.starts_with("CQ") => {
                        let snr: i32 = rng.gen_range(-50..=49);
                        let r = if rng.gen_bool(0.4) { "R" } else { "" };
                        format!("{w1} {w2} {r}{snr:+03}")
                    }
                    3 if !w1.starts_with("CQ") => format!("{w1} {w2} RRR"),
                    4 if !w1.starts_with("CQ") => format!("{w1} {w2} RR73"),
                    _ if !w1.starts_with("CQ") => format!("{w1} {w2} 73"),
                    _ => format!("{w1} {w2} {}", rand_grid4(&mut rng)),
                }
            }
            // 2 EU VHF /P
            6 => {
                let mut c1 = rand_std_call(&mut rng);
                let mut c2 = rand_std_call(&mut rng);
                if rng.gen_bool(0.5) {
                    c1.push_str("/P");
                } else {
                    c2.push_str("/P");
                }
                match rng.gen_range(0..3) {
                    0 => format!("{c1} {c2} {}", rand_grid4(&mut rng)),
                    1 => format!("{c1} {c2} R {}", rand_grid4(&mut rng)),
                    _ => format!("{c1} {c2} RR73"),
                }
            }
            // 3 ARRL RTTY
            7 => {
                let tu = if rng.gen_bool(0.5) { "TU; " } else { "" };
                let c1 = rand_std_call(&mut rng);
                let c2 = rand_std_call(&mut rng);
                let r = if rng.gen_bool(0.5) { "R " } else { "" };
                let rpt = 509 + 10 * rng.gen_range(2..=9);
                let exch = if rng.gen_bool(0.5) {
                    RTTY_MULTS[rng.gen_range(0..RTTY_MULTS.len())].to_string()
                } else {
                    format!("{:04}", rng.gen_range(1..=7999))
                };
                format!("{tu}{c1} {c2} {r}{rpt} {exch}")
            }
            // 4 Nonstandard
            8 => {
                let ns = rand_nonstd_call(&mut rng);
                let sc = rand_std_call(&mut rng);
                let rpt = match rng.gen_range(0..4) {
                    0 => "",
                    1 => " RRR",
                    2 => " RR73",
                    _ => " 73",
                };
                match rng.gen_range(0..3) {
                    0 => format!("CQ {ns}"),
                    1 => format!("<{sc}> {ns}{rpt}"),
                    _ => format!("{ns} <{sc}>{rpt}"),
                }
            }
            // 5 EU VHF Hash
            _ => {
                let c1 = rand_nonstd_call(&mut rng);
                let c2 = rand_nonstd_call(&mut rng);
                let r = if rng.gen_bool(0.5) { "R " } else { "" };
                let rpt = rng.gen_range(52..=59);
                let ser = rng.gen_range(1..=4095);
                format!("<{c1}> <{c2}> {r}{rpt}{ser:04} {}", rand_grid6(&mut rng))
            }
        };
        msgs.push(m);
    }
    msgs.truncate(count);
    msgs
}

fn verify_golden_suite(sim: &SimRunner, mode: Mode) -> Result<usize, CodecOracleError> {
    let (tool, stdout) = match mode {
        Mode::Ft8 => ("ft8code", sim.run_ft8code(&["-T"])?),
        Mode::Ft4 => ("ft4code", sim.run_ft4code(&["-t"])?),
    };
    let rows = parse_golden_table(&stdout, tool)?;
    let ht = CallsignHashTable::new();
    let ctx = UnpackContext::default();

    for (idx, row) in rows.iter().enumerate() {
        let packed = pack77(&row.msg, None, &ctx, &ht).map_err(|e| CodecOracleError::Mismatch {
            mismatches: 1,
            first_detail: format!("{tool} golden row {}: {e}", idx + 1),
        })?;
        let bad_rust = row.msg != packed.msgsent;
        if packed.i3 != row.i3
            || packed.n3 != row.n3
            || packed.msgsent != row.msgsent
            || bad_rust != row.bad
        {
            return Err(CodecOracleError::Mismatch {
                mismatches: 1,
                first_detail: format!(
                    "{tool} golden row {} {:?}: oracle=({}.{}, {:?}, bad={}) vs rust=({}.{}, {:?}, bad={})",
                    idx + 1,
                    row.msg,
                    row.i3,
                    row.n3,
                    row.msgsent,
                    row.bad,
                    packed.i3,
                    packed.n3,
                    packed.msgsent,
                    bad_rust
                ),
            });
        }
    }

    Ok(rows.len())
}

fn verify_single_message(
    sim: &SimRunner,
    mode: Mode,
    msg: &str,
) -> Result<String, CodecOracleError> {
    let stdout = match mode {
        Mode::Ft8 => sim.run_ft8code(&[msg])?,
        Mode::Ft4 => sim.run_ft4code(&[msg])?,
    };
    let oracle = parse_single_oracle(&stdout, mode, msg)?;

    // Mirror ft8code/ft4code calling fmtmsg on the CLI argument before pack77.
    let norm = fmtmsg(msg);
    let ht = CallsignHashTable::new();
    let ctx = UnpackContext::default();
    let pack_res = pack77(&norm, None, &ctx, &ht);
    if oracle.msgsent == "*** bad message ***" {
        return match pack_res {
            Err(_) => Ok("invalid".to_string()),
            Ok(p) => Err(CodecOracleError::Mismatch {
                mismatches: 1,
                first_detail: format!(
                    "{mode:?} {msg:?}: oracle rejected (*** bad message ***) but rust packed {:?}",
                    p.msgsent
                ),
            }),
        };
    }
    let packed = pack_res.map_err(|e| CodecOracleError::Mismatch {
        mismatches: 1,
        first_detail: format!("{mode:?} {msg:?}: rust pack77 error {e}"),
    })?;

    if packed.i3 != oracle.i3 || packed.n3 != oracle.n3 || packed.msgsent != oracle.msgsent {
        return Err(CodecOracleError::Mismatch {
            mismatches: 1,
            first_detail: format!(
                "{mode:?} {msg:?}: header/msgsent mismatch oracle=({}.{}, {:?}) rust=({}.{}, {:?})",
                oracle.i3, oracle.n3, oracle.msgsent, packed.i3, packed.n3, packed.msgsent
            ),
        });
    }

    if packed.bits != oracle.msg77_unscrambled {
        return Err(CodecOracleError::Mismatch {
            mismatches: 1,
            first_detail: format!("{mode:?} {msg:?}: 77-bit payload mismatch"),
        });
    }

    let (fec_input, rust_tones): ([u8; 77], Vec<u8>) = match mode {
        Mode::Ft8 => {
            let t = encode_ft8_tones_from_77bits(&packed.bits);
            (packed.bits, t.to_vec())
        }
        Mode::Ft4 => {
            let sc = scramble_ft4(&packed.bits);
            if Some(sc) != oracle.msg77_scrambled {
                return Err(CodecOracleError::Mismatch {
                    mismatches: 1,
                    first_detail: format!("{mode:?} {msg:?}: FT4 scrambled 77-bit mismatch"),
                });
            }
            let t = encode_ft4_tones_from_77bits(&packed.bits);
            (sc, t.to_vec())
        }
    };

    let crc_val = compute_crc14(&fec_input);
    let mut crc_bits = [0u8; 14];
    for (i, bit) in crc_bits.iter_mut().enumerate() {
        *bit = ((crc_val >> (13 - i)) & 1) as u8;
    }
    if crc_bits != oracle.crc14 {
        return Err(CodecOracleError::Mismatch {
            mismatches: 1,
            first_detail: format!("{mode:?} {msg:?}: CRC-14 mismatch"),
        });
    }

    let cw = encode174_91(&fec_input);
    if cw[91..174] != oracle.parity83 {
        return Err(CodecOracleError::Mismatch {
            mismatches: 1,
            first_detail: format!("{mode:?} {msg:?}: 83 LDPC parity bits mismatch"),
        });
    }

    if rust_tones != oracle.tones {
        return Err(CodecOracleError::Mismatch {
            mismatches: 1,
            first_detail: format!("{mode:?} {msg:?}: channel tones mismatch"),
        });
    }

    Ok(format!("{}.{}", packed.i3, packed.n3))
}

pub fn run_codec_diff(
    sim: &SimRunner,
    seed: u64,
    count: usize,
    out_json: Option<&Path>,
) -> Result<CodecDiffReport, CodecOracleError> {
    let started = Instant::now();

    let ft8_golden = verify_golden_suite(sim, Mode::Ft8)?;
    println!("[PASS] ft8code -T stateful golden suite ({ft8_golden}/49 exact matches)");

    let ft4_golden = verify_golden_suite(sim, Mode::Ft4)?;
    println!("[PASS] ft4code -t stateful golden suite ({ft4_golden}/47 exact matches)");

    let ft8_msgs = generate_fuzz_messages(seed, count);
    let ft4_msgs = generate_fuzz_messages(seed ^ 0x9E37_79B9_7F4A_7C15, count);

    let workers = std::thread::available_parallelism()
        .map(|n| n.get().min(4))
        .unwrap_or(2);

    let run_batch = |mode: Mode, msgs: &[String]| -> Result<BTreeMap<String, usize>, CodecOracleError> {
        let next_idx = AtomicUsize::new(0);
        let hist_parts: Vec<Result<BTreeMap<String, usize>, CodecOracleError>> =
            std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(workers);
                for _ in 0..workers {
                    handles.push(s.spawn(|| {
                        let mut local_hist: BTreeMap<String, usize> = BTreeMap::new();
                        loop {
                            let i = next_idx.fetch_add(1, Ordering::Relaxed);
                            if i >= msgs.len() {
                                break;
                            }
                            let tag = verify_single_message(sim, mode, &msgs[i])?;
                            *local_hist.entry(tag).or_insert(0) += 1;
                        }
                        Ok(local_hist)
                    }));
                }
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

        let mut merged = BTreeMap::new();
        for part in hist_parts {
            for (k, v) in part? {
                *merged.entry(k).or_insert(0) += v;
            }
        }
        Ok(merged)
    };

    let ft8_hist = run_batch(Mode::Ft8, &ft8_msgs)?;
    println!(
        "[PASS] ft8code differential fuzz ({}/{} exact bit+tone parity): {:?}",
        ft8_msgs.len(),
        ft8_msgs.len(),
        ft8_hist
    );

    let ft4_hist = run_batch(Mode::Ft4, &ft4_msgs)?;
    println!(
        "[PASS] ft4code differential fuzz ({}/{} exact bit+tone parity): {:?}",
        ft4_msgs.len(),
        ft4_msgs.len(),
        ft4_hist
    );

    let report = CodecDiffReport {
        seed,
        per_mode_fuzz_count: count,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        ft8_golden_vectors: ft8_golden,
        ft4_golden_vectors: ft4_golden,
        ft8_fuzz_checked: ft8_msgs.len(),
        ft4_fuzz_checked: ft4_msgs.len(),
        ft8_type_histogram: ft8_hist,
        ft4_type_histogram: ft4_hist,
        mismatches: 0,
    };

    if let Some(out_path) = out_json {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(out_path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
        println!("Wrote codec-diff report to {}", out_path.display());
    }

    Ok(report)
}
