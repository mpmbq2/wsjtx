// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! 77-bit WSJT-X message decoder (`unpack77`, `unpack77_readonly`, `unpack28`, `callok`).

use crate::FtxrError;

use super::hashtable::{compute_all_hashes, sanitize_hash_call, CallsignHashTable, HASH_ALPHABET};
use super::pack77::{FREETEXT_ALPHABET, MAX22, MAXGRID4, NTOKENS};
use super::types::{
    MessageType, PendingHashUpdates, UnpackContext, UnpackMode, UnpackedMessage, ARRL_SECTIONS,
    PAYLOAD_BITS, RTTY_MULTS,
};

const C1: &[u8; 37] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const C2: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const C3: &[u8; 10] = b"0123456789";
const C4: &[u8; 27] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A2: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const NZZZ: u32 = 46_656;

/// Port of `callok` (`wsjtx/lib/77bit/packjt77.f90:3452-3500`) used by `unpack28`.
pub fn callok(call: &str) -> bool {
    let w = call.trim_end();
    let b = w.as_bytes();
    let n = b.len();
    if n < 3 || b[0] == b'Q' {
        return false;
    }

    let mut i0 = 0usize; // 1-based
    for i in (1..=n).rev() {
        if b[i - 1].is_ascii_digit() {
            i0 = i;
            break;
        }
    }
    if i0 != 2 && i0 != 3 {
        return false;
    }

    let pfx = &b[..(i0 - 1).min(2)];
    let sfx_end = (i0 + 3).min(n);
    let sfx = &b[i0..sfx_end];

    let nlp = pfx.iter().filter(|&&c| c.is_ascii_alphabetic()).count();
    if nlp == 0 {
        return false;
    }

    sfx.iter().all(|&c| c.is_ascii_alphabetic())
}

/// Port of `unpack28` (`wsjtx/lib/77bit/packjt77.f90:836-917`).
pub fn unpack28(n28: u32, hash_table: &CallsignHashTable) -> (String, bool) {
    let mut success = true;
    let mut c13: String;

    if n28 < NTOKENS {
        if n28 == 0 {
            c13 = "DE".to_string();
            return (c13, true);
        }
        if n28 == 1 {
            c13 = "QRZ".to_string();
            return (c13, true);
        }
        if n28 == 2 {
            c13 = "CQ".to_string();
            return (c13, true);
        }
        if n28 <= 1002 {
            c13 = format!("CQ_{:03}", n28 - 3);
            return (c13, true);
        }
        if n28 <= 532_443 {
            let mut n = n28 - 1003;
            let i1 = (n / (27 * 27 * 27)) as usize;
            n -= 27 * 27 * 27 * (i1 as u32);
            let i2 = (n / (27 * 27)) as usize;
            n -= 27 * 27 * (i2 as u32);
            let i3 = (n / 27) as usize;
            let i4 = (n - 27 * (i3 as u32)) as usize;
            let raw = [C4[i1] as char, C4[i2] as char, C4[i3] as char, C4[i4] as char];
            let s: String = raw.into_iter().collect();
            let trimmed = s.trim_start().trim_end();
            c13 = format!("CQ_{trimmed}");
            return (c13, true);
        }
        // Reserved gap 532_444..=2_063_591 falls through to "<...>" with success = true,
        // matching Fortran `packjt77.f90:878-887`.
        return ("<...>".to_string(), true);
    }

    let rem = n28 - NTOKENS;
    if rem < MAX22 {
        return (hash_table.format_22(rem), true);
    }

    let mut n = rem - MAX22;
    let i1 = (n / (36 * 10 * 27 * 27 * 27)) as usize;
    n -= 36 * 10 * 27 * 27 * 27 * (i1 as u32);
    let i2 = (n / (10 * 27 * 27 * 27)) as usize;
    n -= 10 * 27 * 27 * 27 * (i2 as u32);
    let i3 = (n / (27 * 27 * 27)) as usize;
    n -= 27 * 27 * 27 * (i3 as u32);
    let i4 = (n / (27 * 27)) as usize;
    n -= 27 * 27 * (i4 as u32);
    let i5 = (n / 27) as usize;
    let i6 = (n - 27 * (i5 as u32)) as usize;

    if i1 >= C1.len()
        || i2 >= C2.len()
        || i3 >= C3.len()
        || i4 >= C4.len()
        || i5 >= C4.len()
        || i6 >= C4.len()
    {
        return ("QU1RK".to_string(), false);
    }

    let raw = [
        C1[i1] as char,
        C2[i2] as char,
        C3[i3] as char,
        C4[i4] as char,
        C4[i5] as char,
        C4[i6] as char,
    ];
    let s: String = raw.into_iter().collect();
    c13 = s.trim_start().trim_end().to_string();

    if !callok(&c13) || c13.contains(' ') {
        c13 = "QU1RK".to_string();
        success = false;
    }

    (c13, success)
}

#[inline]
fn read_bits_u64(bits: &[u8; PAYLOAD_BITS], start: usize, len: usize) -> u64 {
    let mut v = 0u64;
    for &b in &bits[start..start + len] {
        v = (v << 1) | ((b & 1) as u64);
    }
    v
}

#[inline]
fn read_bits_u128(bits: &[u8; PAYLOAD_BITS], start: usize, len: usize) -> u128 {
    let mut v = 0u128;
    for &b in &bits[start..start + len] {
        v = (v << 1) | ((b & 1) as u128);
    }
    v
}

fn unpacktext77(bits71: u128) -> Option<String> {
    let mut v = bits71;
    let mut chars = [b' '; 13];
    for i in (0..13).rev() {
        let rem = (v % 42) as usize;
        v /= 42;
        chars[i] = FREETEXT_ALPHABET[rem];
    }
    let s = std::str::from_utf8(&chars).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn to_grid4(n: u32) -> Option<String> {
    if n >= MAXGRID4 {
        return None;
    }
    let mut rem = n;
    let j1 = rem / 1800;
    rem -= j1 * 1800;
    let j2 = rem / 100;
    rem -= j2 * 100;
    let j3 = rem / 10;
    let j4 = rem - j3 * 10;
    if j1 > 17 || j2 > 17 || j3 > 9 || j4 > 9 {
        return None;
    }
    Some(format!(
        "{}{}{}{}",
        (b'A' + j1 as u8) as char,
        (b'A' + j2 as u8) as char,
        (b'0' + j3 as u8) as char,
        (b'0' + j4 as u8) as char,
    ))
}

fn to_grid6_base24(n: u32) -> Option<String> {
    if n > 18_662_399 {
        return None;
    }
    let mut rem = n;
    let j1 = rem / (18 * 10 * 10 * 24 * 24);
    rem -= j1 * 18 * 10 * 10 * 24 * 24;
    let j2 = rem / (10 * 10 * 24 * 24);
    rem -= j2 * 10 * 10 * 24 * 24;
    let j3 = rem / (10 * 24 * 24);
    rem -= j3 * 10 * 24 * 24;
    let j4 = rem / (24 * 24);
    rem -= j4 * 24 * 24;
    let j5 = rem / 24;
    let j6 = rem - j5 * 24;
    if j1 > 17 || j2 > 17 || j3 > 9 || j4 > 9 || j5 > 23 || j6 > 23 {
        return None;
    }
    Some(format!(
        "{}{}{}{}{}{}",
        (b'A' + j1 as u8) as char,
        (b'A' + j2 as u8) as char,
        (b'0' + j3 as u8) as char,
        (b'0' + j4 as u8) as char,
        (b'A' + j5 as u8) as char,
        (b'A' + j6 as u8) as char,
    ))
}

fn to_grid_wspr_base25(n: u32) -> Option<String> {
    let mut rem = n;
    let j1 = rem / (18 * 10 * 10 * 25 * 25);
    if j1 > 17 {
        return None;
    }
    rem -= j1 * 18 * 10 * 10 * 25 * 25;
    let j2 = rem / (10 * 10 * 25 * 25);
    if j2 > 17 {
        return None;
    }
    rem -= j2 * 10 * 10 * 25 * 25;
    let j3 = rem / (10 * 25 * 25);
    if j3 > 9 {
        return None;
    }
    rem -= j3 * 10 * 25 * 25;
    let j4 = rem / (25 * 25);
    if j4 > 9 {
        return None;
    }
    rem -= j4 * 25 * 25;
    let j5 = rem / 25;
    let j6 = rem - j5 * 25;
    if j5 > 24 || j6 > 24 {
        return None;
    }
    let mut out = format!(
        "{}{}{}{}",
        (b'A' + j1 as u8) as char,
        (b'A' + j2 as u8) as char,
        (b'0' + j3 as u8) as char,
        (b'0' + j4 as u8) as char,
    );
    if j5 != 24 || j6 != 24 {
        out.push((b'A' + j5 as u8) as char);
        out.push((b'A' + j6 as u8) as char);
    }
    Some(out)
}

/// Pure read-only 77-bit message decoder safe for concurrent Rayon workers.
///
/// Emits deferred hash-table mutations in `UnpackedMessage::pending_hashes`.
pub fn unpack77_readonly(
    bits: &[u8; PAYLOAD_BITS],
    mode: UnpackMode,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<UnpackedMessage, FtxrError> {
    if bits.iter().any(|&b| b > 1) {
        return Err(FtxrError::UnpackFailed);
    }

    let n3 = read_bits_u64(bits, 71, 3) as u8;
    let i3 = read_bits_u64(bits, 74, 3) as u8;
    let msg_type = MessageType::from_i3_n3(i3, n3).ok_or(FtxrError::UnpackFailed)?;

    let mut pending = PendingHashUpdates::default();

    let my_clean = ctx
        .my_call
        .as_deref()
        .and_then(sanitize_hash_call)
        .filter(|c| c.len() >= 3);
    let my_hashes = my_clean.as_deref().map(|c| {
        pending.save_calls.push(c.to_string());
        compute_all_hashes(c)
    });

    let dx_clean = ctx
        .dx_call
        .as_deref()
        .and_then(sanitize_hash_call)
        .filter(|c| c.len() >= 3);
    let dx_hashes = dx_clean.as_deref().map(compute_all_hashes);

    let text: String = match (i3, n3) {
        (0, 0) => {
            let val = read_bits_u128(bits, 0, 71);
            unpacktext77(val).ok_or(FtxrError::UnpackFailed)?
        }
        (0, 1) => {
            let n28a = read_bits_u64(bits, 0, 28) as u32;
            let n28b = read_bits_u64(bits, 28, 28) as u32;
            let n10 = read_bits_u64(bits, 56, 10) as u16;
            let n5 = read_bits_u64(bits, 66, 5) as i32;

            let (c1, ok1) = unpack28(n28a, hash_table);
            let (c2, ok2) = unpack28(n28b, hash_table);
            if !ok1 || n28a <= 2 || !ok2 || n28b <= 2 {
                return Err(FtxrError::UnpackFailed);
            }

            let mut c3 = hash_table.format_10(n10);
            if mode == UnpackMode::Rx {
                if let (Some(dx), Some(dh)) = (&dx_clean, dx_hashes) {
                    if dh.h10 == n10 {
                        c3 = format!("<{dx}>");
                    }
                }
            } else if let (Some(my), Some(mh)) = (&my_clean, my_hashes) {
                if mh.h10 == n10 {
                    c3 = format!("<{my}>");
                }
            }

            let irpt = 2 * n5 - 30;
            let crpt = format!("{irpt:+03}");
            format!("{c1} RR73; {c2} {c3} {crpt}")
        }
        (0, 3 | 4) => {
            let n28a = read_bits_u64(bits, 0, 28) as u32;
            let n28b = read_bits_u64(bits, 28, 28) as u32;
            let ir = read_bits_u64(bits, 56, 1);
            let intx = read_bits_u64(bits, 57, 4) as u32;
            let nclass = read_bits_u64(bits, 61, 3) as u8;
            let isec = read_bits_u64(bits, 64, 7) as usize;

            if isec == 0 || isec > ARRL_SECTIONS.len() {
                return Err(FtxrError::UnpackFailed);
            }
            let (c1, ok1) = unpack28(n28a, hash_table);
            let (c2, ok2) = unpack28(n28b, hash_table);
            if !ok1 || n28a <= 2 || !ok2 || n28b <= 2 {
                return Err(FtxrError::UnpackFailed);
            }

            let ntx = intx + 1 + if n3 == 4 { 16 } else { 0 };
            let class_ch = (b'A' + nclass) as char;
            let sec = ARRL_SECTIONS[isec - 1];
            if ir == 1 {
                format!("{c1} {c2} R {ntx}{class_ch} {sec}")
            } else {
                format!("{c1} {c2} {ntx}{class_ch} {sec}")
            }
        }
        (0, 5) => {
            let t0 = read_bits_u64(bits, 0, 23);
            let t1 = read_bits_u64(bits, 23, 24);
            let t2 = read_bits_u64(bits, 47, 24);
            let hex = format!("{t0:06X}{t1:06X}{t2:06X}");
            hex.trim_start_matches('0').to_string()
        }
        (0, 6) => {
            let j48 = read_bits_u64(bits, 47, 1);
            let j49 = read_bits_u64(bits, 48, 1);
            let j50 = read_bits_u64(bits, 49, 1);
            let itype = if j50 == 1 {
                2
            } else if j49 == 0 {
                1
            } else if j48 == 0 {
                3
            } else {
                return Err(FtxrError::UnpackFailed);
            };

            match itype {
                1 => {
                    let n28 = read_bits_u64(bits, 0, 28) as u32;
                    let igrid4 = read_bits_u64(bits, 28, 15) as u32;
                    let raw_dbm = read_bits_u64(bits, 43, 5) as f32;
                    let idbm = (raw_dbm * 10.0 / 3.0).round() as i32;
                    if !(0..=60).contains(&idbm) {
                        return Err(FtxrError::UnpackFailed);
                    }
                    let (c1, ok1) = unpack28(n28, hash_table);
                    let grid4 = to_grid4(igrid4).ok_or(FtxrError::UnpackFailed)?;
                    if !ok1 {
                        return Err(FtxrError::UnpackFailed);
                    }
                    pending.save_calls.push(c1.clone());
                    format!("{c1} {grid4} {idbm}")
                }
                2 => {
                    let n28 = read_bits_u64(bits, 0, 28) as u32;
                    let npfx_raw = read_bits_u64(bits, 28, 16) as u32;
                    let raw_dbm = read_bits_u64(bits, 44, 5) as f32;
                    let idbm = (raw_dbm * 10.0 / 3.0).round() as i32;
                    if !(0..=60).contains(&idbm) {
                        return Err(FtxrError::UnpackFailed);
                    }
                    let (c1, ok1) = unpack28(n28, hash_table);
                    if !ok1 {
                        return Err(FtxrError::UnpackFailed);
                    }
                    let call_1a = if npfx_raw < NZZZ {
                        let mut npfx = npfx_raw;
                        let mut buf = [b' '; 3];
                        for i in (0..3).rev() {
                            buf[i] = A2[(npfx % 36) as usize];
                            npfx /= 36;
                            if npfx == 0 {
                                break;
                            }
                        }
                        let pfx = std::str::from_utf8(&buf).unwrap_or("").trim_start();
                        format!("{pfx}/{c1}")
                    } else {
                        let npfx = npfx_raw - NZZZ;
                        let sfx = if npfx <= 35 {
                            format!("{}", A2[npfx as usize] as char)
                        } else if npfx <= 1295 {
                            format!(
                                "{}{}",
                                A2[(npfx / 36) as usize] as char,
                                A2[(npfx % 36) as usize] as char
                            )
                        } else if npfx <= 12959 {
                            format!(
                                "{}{}{}",
                                A2[(npfx / 360) as usize] as char,
                                A2[((npfx / 10) % 36) as usize] as char,
                                A2[(npfx % 10) as usize] as char
                            )
                        } else {
                            return Err(FtxrError::UnpackFailed);
                        };
                        format!("{c1}/{sfx}")
                    };
                    pending.save_calls.push(call_1a.clone());
                    format!("{call_1a} {idbm}")
                }
                3 => {
                    let n22 = read_bits_u64(bits, 0, 22) as u32;
                    let igrid6 = read_bits_u64(bits, 22, 25) as u32;
                    let (c1, ok1) = unpack28(n22 + NTOKENS, hash_table);
                    let grid6 = to_grid_wspr_base25(igrid6).ok_or(FtxrError::UnpackFailed)?;
                    if !ok1 {
                        return Err(FtxrError::UnpackFailed);
                    }
                    format!("{c1} {grid6}")
                }
                _ => return Err(FtxrError::UnpackFailed),
            }
        }
        (1 | 2, _) => {
            let n28a = read_bits_u64(bits, 0, 28) as u32;
            let ipa = read_bits_u64(bits, 28, 1);
            let n28b = read_bits_u64(bits, 29, 28) as u32;
            let ipb = read_bits_u64(bits, 57, 1);
            let ir = read_bits_u64(bits, 58, 1);
            let igrid4 = read_bits_u64(bits, 59, 15) as u32;

            let (mut c1, mut ok1) = unpack28(n28a, hash_table);
            if mode == UnpackMode::Rx {
                if let (Some(my), Some(mh)) = (&my_clean, my_hashes) {
                    if n28a >= NTOKENS && mh.h22 == (n28a - NTOKENS) {
                        c1 = format!("<{my}>");
                        ok1 = true;
                    }
                }
            }
            let (mut c2, ok2) = unpack28(n28b, hash_table);
            if !ok1 || !ok2 {
                return Err(FtxrError::UnpackFailed);
            }

            if c1.starts_with("CQ_") {
                c1.replace_range(2..3, " ");
            }
            let suffix = if i3 == 1 { "/R" } else { "/P" };
            if !c1.contains('<') && c1.len() >= 3 {
                if ipa == 1 {
                    c1.push_str(suffix);
                }
                pending.recent_calls.push(c1.clone());
            }
            if !c2.contains('<') && c2.len() >= 3 {
                if ipb == 1 {
                    c2.push_str(suffix);
                }
                pending.recent_calls.push(c2.clone());
                // Only hash the "from" call (packjt77.f90:559).
                pending.save_calls.push(c2.clone());
            }

            if igrid4 <= MAXGRID4 {
                let grid4 = to_grid4(igrid4).ok_or(FtxrError::UnpackFailed)?;
                let out = if ir == 0 {
                    format!("{c1} {c2} {grid4}")
                } else {
                    format!("{c1} {c2} R {grid4}")
                };
                if out.starts_with("CQ ") && ir == 1 {
                    return Err(FtxrError::UnpackFailed);
                }
                out
            } else {
                let irpt = igrid4 - MAXGRID4;
                let out = match irpt {
                    1 => format!("{c1} {c2}"),
                    2 => format!("{c1} {c2} RRR"),
                    3 => format!("{c1} {c2} RR73"),
                    4 => format!("{c1} {c2} 73"),
                    _ => {
                        let mut isnr = (irpt as i32) - 35;
                        if isnr > 50 {
                            isnr -= 101;
                        }
                        let crpt = format!("{isnr:+03}");
                        if ir == 0 {
                            format!("{c1} {c2} {crpt}")
                        } else {
                            format!("{c1} {c2} R{crpt}")
                        }
                    }
                };
                if out.starts_with("CQ ") && irpt >= 2 {
                    return Err(FtxrError::UnpackFailed);
                }
                out
            }
        }
        (3, _) => {
            let itu = read_bits_u64(bits, 0, 1);
            let n28a = read_bits_u64(bits, 1, 28) as u32;
            let n28b = read_bits_u64(bits, 29, 28) as u32;
            let ir = read_bits_u64(bits, 57, 1);
            let irpt = read_bits_u64(bits, 58, 3) as u8;
            let nexch = read_bits_u64(bits, 61, 13) as usize;

            let (c1, ok1) = unpack28(n28a, hash_table);
            let (c2, ok2) = unpack28(n28b, hash_table);
            if !ok1 || !ok2 {
                return Err(FtxrError::UnpackFailed);
            }

            let crpt = format!("5{}9", irpt + 2);
            let exch = if nexch > 8000 {
                let imult = nexch - 8000;
                if (1..=RTTY_MULTS.len()).contains(&imult) {
                    RTTY_MULTS[imult - 1].to_string()
                } else {
                    return Err(FtxrError::UnpackFailed);
                }
            } else if (1..=7999).contains(&nexch) {
                format!("{nexch:04}")
            } else {
                return Err(FtxrError::UnpackFailed);
            };

            let tu_pfx = if itu == 1 { "TU; " } else { "" };
            let r_str = if ir == 1 { " R " } else { " " };
            format!("{tu_pfx}{c1} {c2}{r_str}{crpt} {exch}")
        }
        (4, _) => {
            let n12 = read_bits_u64(bits, 0, 12) as u16;
            let mut n58 = read_bits_u64(bits, 12, 58);
            let iflip = read_bits_u64(bits, 70, 1);
            let nrpt = read_bits_u64(bits, 71, 2);
            let icq = read_bits_u64(bits, 73, 1);

            let mut c11 = [b' '; 11];
            for i in (0..11).rev() {
                let j = (n58 % 38) as usize;
                c11[i] = HASH_ALPHABET[j];
                n58 /= 38;
            }
            let literal = std::str::from_utf8(&c11)
                .unwrap_or("")
                .trim_start()
                .trim_end()
                .to_string();

            let hashed = hash_table.format_12(n12);
            let (call_1, call_2) = if iflip == 0 {
                let mut c1 = hashed;
                let c2 = literal;
                if !c2.is_empty() {
                    pending.recent_calls.push(c2.clone());
                    pending.save_calls.push(c2.clone());
                }
                if mode == UnpackMode::Rx {
                    if let (Some(my), Some(mh)) = (&my_clean, my_hashes) {
                        if mh.h12 == n12 {
                            let dx_match = dx_clean.as_deref().is_some_and(|dx| dx == c2);
                            if dx_match || c1.contains("<...>") {
                                c1 = format!("<{my}>");
                            }
                        }
                    }
                }
                (c1, c2)
            } else {
                let c1 = literal;
                let mut c2 = hashed;
                if !c1.is_empty() {
                    pending.recent_calls.push(c1.clone());
                }
                if mode == UnpackMode::Tx {
                    if let (Some(my), Some(mh)) = (&my_clean, my_hashes) {
                        if mh.h12 == n12 {
                            c2 = format!("<{my}>");
                        }
                    }
                }
                (c1, c2)
            };

            if icq == 0 {
                match nrpt {
                    0 => format!("{call_1} {call_2}"),
                    1 => format!("{call_1} {call_2} RRR"),
                    2 => format!("{call_1} {call_2} RR73"),
                    _ => format!("{call_1} {call_2} 73"),
                }
            } else {
                format!("CQ {call_2}")
            }
        }
        (5, _) => {
            let n12 = read_bits_u64(bits, 0, 12) as u16;
            let n22 = read_bits_u64(bits, 12, 22) as u32;
            let ir = read_bits_u64(bits, 34, 1);
            let irpt = read_bits_u64(bits, 35, 3) as u32;
            let iserial = read_bits_u64(bits, 38, 11) as u32;
            let igrid6 = read_bits_u64(bits, 49, 25) as u32;

            let grid6 = to_grid6_base24(igrid6).ok_or(FtxrError::UnpackFailed)?;
            let mut c1 = hash_table.format_12(n12);
            if let (Some(my), Some(mh)) = (&my_clean, my_hashes) {
                if mh.h12 == n12 {
                    c1 = format!("<{my}>");
                }
            }
            let c2 = hash_table.format_22(n22);
            let nrs = 52 + irpt;
            let cexch = format!("{nrs:02}{iserial:04}");
            if ir == 0 {
                format!("{c1} {c2} {cexch} {grid6}")
            } else {
                format!("{c1} {c2} R {cexch} {grid6}")
            }
        }
        _ => return Err(FtxrError::UnpackFailed),
    };

    if text.starts_with("CQ <") {
        return Err(FtxrError::UnpackFailed);
    }

    Ok(UnpackedMessage {
        text,
        i3,
        n3,
        msg_type,
        pending_hashes: pending,
    })
}

/// Decodes a 77-bit payload and immediately commits any learned callsign hashes
/// and recent calls into `hash_table`.
pub fn unpack77(
    bits: &[u8; PAYLOAD_BITS],
    mode: UnpackMode,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<UnpackedMessage, FtxrError> {
    let unpacked = unpack77_readonly(bits, mode, ctx, hash_table)?;
    let my_clean = ctx
        .my_call
        .as_deref()
        .and_then(sanitize_hash_call)
        .filter(|c| c.len() >= 3);
    hash_table.apply_updates(&unpacked.pending_hashes, my_clean.as_deref());
    Ok(unpacked)
}
