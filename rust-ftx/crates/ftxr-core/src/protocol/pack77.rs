// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! 77-bit WSJT-X message encoder (`pack77`, `pack28`, `split77`, `fmtmsg`, `chkcall`, `stdcall`).

use crate::FtxrError;

use super::hashtable::{hash_char_index, ihashcall, CallsignHashTable};
use super::types::{
    MessageType, PackHint, PackedMessage, UnpackContext, UnpackMode, ARRL_SECTIONS, MAX_MSG_LEN,
    PAYLOAD_BITS, RTTY_MULTS,
};
use super::unpack77::unpack77;

pub(crate) const NTOKENS: u32 = 2_063_592;
pub(crate) const MAX22: u32 = 4_194_304;
pub(crate) const MAXGRID4: u32 = 32_400;
const NZZZ: u32 = 46_656;

const A1: &[u8; 37] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A2: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A3: &[u8; 10] = b"0123456789";
const A4: &[u8; 27] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";
pub(crate) const FREETEXT_ALPHABET: &[u8; 42] =
    b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?";

#[inline]
fn is_ascii_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[inline]
fn is_ascii_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

pub(crate) fn is_grid4(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
}

fn is_grid6_wspr(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() == 4 || b.len() == 6)
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && (b.len() == 4
            || ((b'A'..=b'X').contains(&b[4]) && (b'A'..=b'X').contains(&b[5])))
}

fn is_grid6_strict(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 6
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && (b'A'..=b'X').contains(&b[4])
        && (b'A'..=b'X').contains(&b[5])
}

/// Normalizes a message matching WSJT-X `fmtmsg.f90` (ASCII uppercase, trim trailing
/// whitespace, collapse repeated spaces).
pub fn fmtmsg(msg: &str) -> String {
    let mut upper: String = msg
        .chars()
        .take(MAX_MSG_LEN)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    while upper.ends_with(' ') {
        upper.pop();
    }
    let mut out = String::with_capacity(upper.len());
    let mut prev_space = false;
    for ch in upper.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Port of `wsjtx/lib/chkcall.f90`: checks whether `word` is a valid standard or compound
/// callsign and returns `Some(basecall)` on success.
pub fn chkcall(word: &str) -> Option<String> {
    let w = word.trim_end();
    let n1 = w.len();
    if n1 == 0 || n1 > 11 {
        return None;
    }
    if w.contains('.') || w.contains('+') || w.contains('-') || w.contains('?') {
        return None;
    }
    let slash_pos = w.find('/');
    if n1 > 6 && slash_pos.is_none() {
        return None;
    }

    let mut bc = &w[..w.len().min(6)];
    if let Some(i0) = slash_pos {
        let left_len = i0;
        let right_len = n1 - 1 - i0;
        if left_len.max(right_len) > 6 {
            return None;
        }
        if i0 >= 1 && i0 < n1 - 1 {
            if left_len <= right_len {
                bc = &w[i0 + 1..];
            } else {
                bc = &w[..i0];
            }
        }
    }

    let nbc = bc.len();
    if nbc == 0 || nbc > 6 {
        return None;
    }
    let b = bc.as_bytes();
    let c0_letter = is_ascii_upper(b[0]);
    let c1_letter = nbc >= 2 && is_ascii_upper(b[1]);
    if !c0_letter && !c1_letter {
        return None;
    }
    if b[0] == b'Q' && !bc.starts_with("QU1RK") {
        return None;
    }

    let mut i1: usize = 0; // 1-based position in bc (2 or 3)
    if nbc >= 2 && is_ascii_digit(b[1]) {
        i1 = 2;
    }
    if nbc >= 3 && is_ascii_digit(b[2]) {
        i1 = 3;
    }
    if i1 == 0 || i1 == nbc {
        return None;
    }

    let sfx = &b[i1..nbc];
    if sfx.is_empty() || sfx.len() > 3 || !sfx.iter().all(|&c| is_ascii_upper(c)) {
        return None;
    }

    Some(bc.to_string())
}

/// Port of `stdcall` (`wsjtx/lib/qra/q65/q65_set_list.f90:66-98`): checks if `call`
/// matches the WSJT-X standard callsign structure used by `pack28` / `ft8apset`.
pub fn stdcall(call: &str) -> bool {
    let w = call.trim_end();
    let b = w.as_bytes();
    let n = b.len();
    if n < 2 {
        return false;
    }
    let mut iarea: usize = 1; // 1-based
    for i in (2..=n).rev() {
        if is_ascii_digit(b[i - 1]) {
            iarea = i;
            break;
        }
    }
    let mut npdig = 0usize;
    let mut nplet = 0usize;
    for &c in &b[..iarea - 1] {
        if is_ascii_digit(c) {
            npdig += 1;
        }
        if is_ascii_upper(c) {
            nplet += 1;
        }
    }
    let mut nslet = 0usize;
    for &c in &b[iarea..n] {
        if is_ascii_upper(c) {
            nslet += 1;
        }
    }
    (2..=3).contains(&iarea) && nplet > 0 && npdig < iarea - 1 && nslet <= 3
}

/// Port of `split77` (`packjt77.f90:919-967`): uppercases ASCII, collapses blanks,
/// splits into up to 19 words (each max 13 chars), and applies the `CQ <dir> <call>` merge.
pub fn split77(msg: &str) -> (String, Vec<String>) {
    let trimmed = msg.trim_end_matches(' ');
    let mut collapsed = String::with_capacity(MAX_MSG_LEN);
    let mut words: Vec<String> = Vec::new();
    let mut prev_space = true;

    for ch in trimmed.chars().take(MAX_MSG_LEN) {
        let mut c = if ch == '\0' { ' ' } else { ch };
        if c == ' ' && prev_space {
            continue;
        }
        if c != ' ' && prev_space && words.len() < 19 {
            words.push(String::new());
        }
        c = c.to_ascii_uppercase();
        collapsed.push(c);
        if c != ' ' {
            if let Some(last) = words.last_mut() {
                if last.len() < 13 {
                    last.push(c);
                }
            }
        }
        prev_space = c == ' ';
    }

    if collapsed.ends_with(' ') {
        collapsed.pop();
    }

    if words.len() >= 3 && words[0] == "CQ" && chkcall(&words[2]).is_some() {
        let prefix_part: String = words[1].chars().take(10).collect();
        words[0] = format!("CQ_{prefix_part}");
        words.remove(1);
    }

    (collapsed, words)
}

#[inline]
fn alpha_index(table: &[u8], ch: u8) -> Option<u32> {
    table.iter().position(|&x| x == ch).map(|idx| idx as u32)
}

/// Port of `pack28` (`packjt77.f90:703-833`): packs a token, `<...>` 22-bit hash,
/// standard callsign, or nonstandard 22-bit hash into 28 bits.
pub fn pack28(
    token: &str,
    hash_table: Option<&CallsignHashTable>,
    my_call: Option<&str>,
) -> Option<u32> {
    let w = token.trim_end();
    if w.is_empty() {
        return None;
    }
    if w == "DE" {
        return Some(0);
    }
    if w == "QRZ" {
        return Some(1);
    }
    if w == "CQ" {
        return Some(2);
    }
    if let Some(rest) = w.strip_prefix("CQ_") {
        let b = rest.as_bytes();
        if (1..=4).contains(&b.len()) {
            let nlet = b.iter().filter(|&&c| is_ascii_upper(c)).count();
            let nnum = b.iter().filter(|&&c| is_ascii_digit(c)).count();
            if nnum == 3 && nlet == 0 && b.len() == 3 {
                let nqsy = rest.parse::<u32>().ok()?;
                return Some(3 + nqsy);
            }
            if (1..=4).contains(&nlet) && nnum == 0 && nlet == b.len() {
                let mut m = 0u32;
                let pad = 4 - b.len();
                for _ in 0..pad {
                    m *= 27;
                }
                for &c in b {
                    let j = (c - b'A') as u32 + 1;
                    m = 27 * m + j;
                }
                return Some(3 + 1000 + m);
            }
        }
    }

    if let Some(after_lt) = w.strip_prefix('<') {
        if let Some(gt) = after_lt.find('>') {
            if let Some(ht) = hash_table {
                ht.save_call(w, my_call);
            }
            let inner = &after_lt[..gt];
            let n22 = ihashcall(inner, 22);
            return Some((NTOKENS + n22) & ((1 << 28) - 1));
        }
    }

    let b = w.as_bytes();
    let n = b.len();
    let mut iarea: usize = 1; // 1-based
    if n >= 2 {
        for i in (2..=n).rev() {
            if is_ascii_digit(b[i - 1]) {
                iarea = i;
                break;
            }
        }
    }
    let mut npdig = 0usize;
    let mut nplet = 0usize;
    for &c in &b[..iarea.saturating_sub(1)] {
        if is_ascii_digit(c) {
            npdig += 1;
        }
        if is_ascii_upper(c) {
            nplet += 1;
        }
    }
    let mut nslet = 0usize;
    for &c in &b[iarea.min(n)..n] {
        if is_ascii_upper(c) {
            nslet += 1;
        }
    }

    let nonstandard = !(2..=3).contains(&iarea) || nplet == 0 || npdig >= iarea - 1 || nslet > 3;
    if nonstandard {
        if let Some(ht) = hash_table {
            ht.save_call(w, my_call);
        }
        let n22 = ihashcall(w, 22);
        return Some((NTOKENS + n22) & ((1 << 28) - 1));
    }

    // Standard callsign: verify characters fit the 6-char mixed-radix alphabet.
    let mut call6 = [b' '; 6];
    if iarea == 2 {
        let copy_len = n.min(5);
        call6[1..1 + copy_len].copy_from_slice(&b[..copy_len]);
    } else {
        let copy_len = n.min(6);
        call6[..copy_len].copy_from_slice(&b[..copy_len]);
    }

    let (Some(i1), Some(i2), Some(i3), Some(i4), Some(i5), Some(i6)) = (
        alpha_index(A1, call6[0]),
        alpha_index(A2, call6[1]),
        alpha_index(A3, call6[2]),
        alpha_index(A4, call6[3]),
        alpha_index(A4, call6[4]),
        alpha_index(A4, call6[5]),
    ) else {
        // Fallback to 22-bit hash if unexpected character (e.g. '/') appeared.
        if let Some(ht) = hash_table {
            ht.save_call(w, my_call);
        }
        let n22 = ihashcall(w, 22);
        return Some((NTOKENS + n22) & ((1 << 28) - 1));
    };

    if let Some(ht) = hash_table {
        ht.save_call(w, my_call);
    }

    let n28 = 36 * 10 * 27 * 27 * 27 * i1
        + 10 * 27 * 27 * 27 * i2
        + 27 * 27 * 27 * i3
        + 27 * 27 * i4
        + 27 * i5
        + i6
        + NTOKENS
        + MAX22;
    Some(n28 & ((1 << 28) - 1))
}

struct BitWriter {
    bits: [u8; PAYLOAD_BITS],
    pos: usize,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bits: [0u8; PAYLOAD_BITS],
            pos: 0,
        }
    }

    fn write_u64(&mut self, val: u64, width: usize) {
        for shift in (0..width).rev() {
            self.bits[self.pos] = ((val >> shift) & 1) as u8;
            self.pos += 1;
        }
    }

    fn write_u128(&mut self, val: u128, width: usize) {
        for shift in (0..width).rev() {
            self.bits[self.pos] = ((val >> shift) & 1) as u8;
            self.pos += 1;
        }
    }

    fn finish(self) -> [u8; PAYLOAD_BITS] {
        debug_assert_eq!(self.pos, PAYLOAD_BITS);
        self.bits
    }
}

fn pack_01_dxpedition(
    words: &[String],
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<[u8; PAYLOAD_BITS]> {
    if words.len() != 5 || words[1] != "RR73;" {
        return None;
    }
    let w4 = &words[3];
    if !w4.starts_with('<') {
        return None;
    }
    let gt = w4.find('>')?;
    let rpt: i32 = words[4].parse().ok()?;
    if rpt == -99 {
        return None;
    }
    let n5 = ((rpt + 30) / 2).clamp(0, 31) as u64;
    chkcall(&words[0])?;
    chkcall(&words[2])?;

    let n28a = pack28(&words[0], Some(hash_table), my_call)? as u64;
    let n28b = pack28(&words[2], Some(hash_table), my_call)? as u64;
    hash_table.save_call(w4, my_call);
    let inner = &w4[1..gt];
    let n10 = ihashcall(inner, 10) as u64;

    let mut bw = BitWriter::new();
    bw.write_u64(n28a, 28);
    bw.write_u64(n28b, 28);
    bw.write_u64(n10, 10);
    bw.write_u64(n5, 5);
    bw.write_u64(1, 3); // n3 = 1
    bw.write_u64(0, 3); // i3 = 0
    Some(bw.finish())
}

fn pack_03_field_day(
    words: &[String],
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<(u8, [u8; PAYLOAD_BITS])> {
    let nwords = words.len();
    if !(4..=5).contains(&nwords) {
        return None;
    }
    chkcall(&words[0])?;
    chkcall(&words[1])?;
    if nwords == 5 && words[2] != "R" {
        return None;
    }

    let sec_word = &words[nwords - 1];
    if !(2..=3).contains(&sec_word.len()) {
        return None;
    }
    let isec = ARRL_SECTIONS
        .iter()
        .position(|&s| s == sec_word)? as u64
        + 1;

    let exch = &words[nwords - 2];
    if exch.len() < 2 {
        return None;
    }
    let class_ch = exch.as_bytes()[exch.len() - 1];
    if !(b'A'..=b'H').contains(&class_ch) {
        return None;
    }
    let nclass = (class_ch - b'A') as u64;
    let ntx: i32 = exch[..exch.len() - 1].parse().ok()?;
    if !(1..=32).contains(&ntx) {
        return None;
    }

    let (n3, intx) = if ntx <= 16 {
        (3u8, (ntx - 1) as u64)
    } else {
        (4u8, (ntx - 17) as u64)
    };

    let n28a = pack28(&words[0], Some(hash_table), my_call)? as u64;
    let n28b = pack28(&words[1], Some(hash_table), my_call)? as u64;
    let ir = if nwords == 5 && words[2] == "R" { 1 } else { 0 };

    let mut bw = BitWriter::new();
    bw.write_u64(n28a, 28);
    bw.write_u64(n28b, 28);
    bw.write_u64(ir, 1);
    bw.write_u64(intx, 4);
    bw.write_u64(nclass, 3);
    bw.write_u64(isec, 7);
    bw.write_u64(n3 as u64, 3);
    bw.write_u64(0, 3); // i3 = 0
    Some((n3, bw.finish()))
}

enum TelemetryOutcome {
    Packed([u8; PAYLOAD_BITS]),
    ForceFreeText,
    NotTelemetry,
}

fn parse_fortran_z6(field: &str) -> Option<u64> {
    let s = field.trim_start();
    if s.is_empty() {
        return Some(0);
    }
    let hex = s.strip_prefix('+').unwrap_or(s);
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

fn pack_05_telemetry(msg_upper: &str) -> TelemetryOutcome {
    let trimmed = msg_upper.trim_start();
    let first_word = trimmed.split(' ').next().unwrap_or("");
    if first_word.is_empty() {
        return TelemetryOutcome::NotTelemetry;
    }
    let token: String = first_word.chars().take(18).collect();
    let padded = format!("{token:>18}");
    let (Some(t0), Some(t1), Some(t2)) = (
        parse_fortran_z6(&padded[0..6]),
        parse_fortran_z6(&padded[6..12]),
        parse_fortran_z6(&padded[12..18]),
    ) else {
        return TelemetryOutcome::NotTelemetry;
    };

    if t0 >= (1 << 23) {
        return TelemetryOutcome::ForceFreeText;
    }

    let mut bw = BitWriter::new();
    bw.write_u64(t0, 23);
    bw.write_u64(t1, 24);
    bw.write_u64(t2, 24);
    bw.write_u64(5, 3); // n3 = 5
    bw.write_u64(0, 3); // i3 = 0
    TelemetryOutcome::Packed(bw.finish())
}

fn pack_06_wspr(
    words: &[String],
    hint: Option<PackHint>,
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<[u8; PAYLOAD_BITS]> {
    let nwords = words.len();
    if nwords == 3 {
        let (m1, m2, m3) = (words[0].len(), words[1].len(), words[2].len());
        if (3..=6).contains(&m1) && m2 == 4 && (1..=2).contains(&m3) {
            if !is_grid4(&words[1]) {
                return None;
            }
            if !words[2].bytes().all(is_ascii_digit) {
                return None;
            }
            let n28 = pack28(&words[0], Some(hash_table), my_call)? as u64;
            let gb = words[1].as_bytes();
            let igrid4 = ((gb[0] - b'A') as u64) * 1800
                + ((gb[1] - b'A') as u64) * 100
                + ((gb[2] - b'0') as u64) * 10
                + ((gb[3] - b'0') as u64);
            let raw_dbm: i32 = words[2].parse().ok()?;
            let idbm = ((raw_dbm.clamp(0, 60) as f64) * 0.3).round() as u64;

            let mut bw = BitWriter::new();
            bw.write_u64(n28, 28);
            bw.write_u64(igrid4, 15);
            bw.write_u64(idbm, 5);
            bw.write_u64(0, 2);
            bw.write_u64(0, 21);
            bw.write_u64(6, 3); // n3 = 6
            bw.write_u64(0, 3); // i3 = 0
            return Some(bw.finish());
        }
    }

    if nwords == 2 {
        let (m1, m2) = (words[0].len(), words[1].len());
        if (5..=10).contains(&m1) && (1..=2).contains(&m2) && words[1].bytes().all(is_ascii_digit) {
            if let Some(slash) = words[0].find('/') {
                let w1b = words[0].as_bytes();
                let cond_len = slash >= 1 && slash < m1 - 1;
                let cond_sfx3 = !(slash + 4 == m1 && !is_ascii_digit(w1b[m1 - 1]));
                if cond_len && cond_sfx3 {
                    if let Some(bcall) = chkcall(&words[0]) {
                        let npfx: Option<u32> = if slash <= 3 {
                            let mut v = 0u32;
                            let mut ok = true;
                            for &c in &w1b[..slash] {
                                if let Some(idx) = alpha_index(A2, c) {
                                    v = 36 * v + idx;
                                } else {
                                    ok = false;
                                    break;
                                }
                            }
                            ok.then_some(v)
                        } else {
                            let sfx = &w1b[slash + 1..];
                            let v = match sfx.len() {
                                1 => alpha_index(A2, sfx[0]),
                                2 => match (alpha_index(A2, sfx[0]), alpha_index(A2, sfx[1])) {
                                    (Some(a), Some(b)) => Some(36 * a + b),
                                    _ => None,
                                },
                                3 if is_ascii_digit(sfx[2]) => {
                                    match (
                                        alpha_index(A2, sfx[0]),
                                        alpha_index(A2, sfx[1]),
                                        alpha_index(A2, sfx[2]),
                                    ) {
                                        (Some(a), Some(b), Some(c)) => {
                                            Some(36 * 10 * a + 10 * b + c)
                                        }
                                        _ => None,
                                    }
                                }
                                _ => None,
                            };
                            v.map(|x| x + NZZZ)
                        };

                        if let Some(npfx_val) = npfx {
                            let n28 = pack28(&bcall, Some(hash_table), my_call)? as u64;
                            let raw_dbm: i32 = words[1].parse().ok()?;
                            let idbm = ((raw_dbm.clamp(0, 60) as f64) * 0.3).round() as u64;

                            let mut bw = BitWriter::new();
                            bw.write_u64(n28, 28);
                            bw.write_u64(npfx_val as u64, 16);
                            bw.write_u64(idbm, 5);
                            bw.write_u64(1, 1);
                            bw.write_u64(0, 21);
                            bw.write_u64(6, 3); // n3 = 6
                            bw.write_u64(0, 3); // i3 = 0
                            return Some(bw.finish());
                        }
                    }
                }
            }
        }

        if hint == Some(PackHint::Wspr)
            && (5..=12).contains(&m1)
            && m2 <= 6
            && words[0].contains('<')
            && words[0].contains('>')
            && is_grid6_wspr(&words[1])
        {
            let n28 = pack28(&words[0], Some(hash_table), my_call)?;
            if n28 >= NTOKENS {
                let n22 = (n28 - NTOKENS) as u64;
                let gb = words[1].as_bytes();
                let mut igrid6 = ((gb[0] - b'A') as u64) * 18 * 10 * 10 * 25 * 25
                    + ((gb[1] - b'A') as u64) * 10 * 10 * 25 * 25
                    + ((gb[2] - b'0') as u64) * 10 * 25 * 25
                    + ((gb[3] - b'0') as u64) * 25 * 25;
                if gb.len() == 4 {
                    igrid6 += 24 * 25 + 24;
                } else {
                    igrid6 += ((gb[4] - b'A') as u64) * 25 + ((gb[5] - b'A') as u64);
                }
                let mut bw = BitWriter::new();
                bw.write_u64(n22, 22);
                bw.write_u64(igrid6, 25);
                bw.write_u64(2, 3);
                bw.write_u64(0, 21);
                bw.write_u64(6, 3); // n3 = 6
                bw.write_u64(0, 3); // i3 = 0
                return Some(bw.finish());
            }
        }
    }

    None
}

fn pack_1_2_standard(
    words: &[String],
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<(u8, [u8; PAYLOAD_BITS])> {
    let nwords = words.len();
    if !(2..=4).contains(&nwords) {
        return None;
    }

    let bcall_1 = chkcall(&words[0]);
    let bcall_2 = chkcall(&words[1]);

    let w1_special = words[0] == "DE"
        || words[0] == "CQ"
        || words[0] == "QRZ"
        || words[0].starts_with("CQ_");
    let w1_hashed = words[0].starts_with('<') && words[0].find('>').is_some_and(|gt| gt >= 4);
    let w2_hashed = words[1].starts_with('<') && words[1].find('>').is_some_and(|gt| gt >= 4);

    let ok1 = bcall_1.is_some() || w1_special || w1_hashed;
    let ok2 = bcall_2.is_some() || w2_hashed;
    if !ok1 || !ok2 {
        return None;
    }
    if (words[0].starts_with('<') && words[1].contains('/'))
        || (words[1].starts_with('<') && words[0].contains('/'))
    {
        return None;
    }
    if nwords == 2 && (!ok2 || words[1].find('/').is_some_and(|idx| idx >= 1)) {
        return None;
    }

    let mut ir = 0u64;
    let mut irpt = 0u32;
    if nwords > 2 {
        if nwords == 4 && words[2] != "R" {
            return None;
        }
        let last = &words[nwords - 1];
        if is_grid4(last) {
            if words[2] == "R" && nwords == 4 {
                ir = 1;
            }
        } else if last.starts_with('+') || last.starts_with('-') {
            ir = 0;
            let mut val: i32 = last.parse().ok()?;
            if (-50..=-31).contains(&val) {
                val += 101;
            }
            let r = val + 35;
            if r < 5 || MAXGRID4 + (r as u32) >= 32768 {
                return None;
            }
            irpt = r as u32;
        } else if last.starts_with("R+") || last.starts_with("R-") {
            ir = 1;
            let mut val: i32 = last[1..].parse().ok()?;
            if (-50..=-31).contains(&val) {
                val += 101;
            }
            let r = val + 35;
            if r < 5 || MAXGRID4 + (r as u32) >= 32768 {
                return None;
            }
            irpt = r as u32;
        } else if last == "RRR" {
            ir = 0;
            irpt = 2;
        } else if last == "RR73" {
            ir = 0;
            irpt = 3;
        } else if last == "73" {
            ir = 0;
            irpt = 4;
        } else {
            return None;
        }
    }

    let w1_has_p = words[0].len() >= 5 && words[0].ends_with("/P");
    let w2_has_p = words[1].len() >= 5 && words[1].ends_with("/P");
    let w1_has_r = words[0].len() >= 5 && words[0].ends_with("/R");
    let w2_has_r = words[1].len() >= 5 && words[1].ends_with("/R");

    let i3: u8 = if w1_has_p || w2_has_p { 2 } else { 1 };

    let c13_1 = if words[0].starts_with("CQ_") || words[0].starts_with('<') {
        words[0].as_str()
    } else if let Some(ref b) = bcall_1 {
        b.as_str()
    } else {
        words[0].as_str()
    };
    let c13_2 = if words[1].starts_with('<') {
        words[1].as_str()
    } else if let Some(ref b) = bcall_2 {
        b.as_str()
    } else {
        words[1].as_str()
    };

    let n28a = pack28(c13_1, Some(hash_table), my_call)? as u64;
    let n28b = pack28(c13_2, Some(hash_table), my_call)? as u64;
    let ipa = if w1_has_p || w1_has_r { 1u64 } else { 0u64 };
    let ipb = if w2_has_p || w2_has_r { 1u64 } else { 0u64 };

    let igrid4: u64 = if nwords == 2 {
        ir = 0;
        (MAXGRID4 + 1) as u64
    } else {
        let last = &words[nwords - 1];
        if is_grid4(last) {
            // Note: "RR73" satisfies is_grid4() and encodes as 32373, matching WSJT-X packjt77.f90:1287-1294.
            ir = if nwords == 4 && words[2] == "R" { 1 } else { 0 };
            let gb = last.as_bytes();
            ((gb[0] - b'A') as u64) * 1800
                + ((gb[1] - b'A') as u64) * 100
                + ((gb[2] - b'0') as u64) * 10
                + ((gb[3] - b'0') as u64)
        } else {
            (MAXGRID4 + irpt) as u64
        }
    };

    let mut bw = BitWriter::new();
    bw.write_u64(n28a, 28);
    bw.write_u64(ipa, 1);
    bw.write_u64(n28b, 28);
    bw.write_u64(ipb, 1);
    bw.write_u64(ir, 1);
    bw.write_u64(igrid4, 15);
    bw.write_u64(i3 as u64, 3);
    Some((i3, bw.finish()))
}

fn pack_3_rtty(
    words: &[String],
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<[u8; PAYLOAD_BITS]> {
    let nwords = words.len();
    if !(4..=6).contains(&nwords) {
        return None;
    }
    if words[0].starts_with('<') && words[1].starts_with('<') {
        return None;
    }
    let itu: usize = if words[0] == "TU;" { 1 } else { 0 };
    let rem = nwords - itu;
    if !(4..=5).contains(&rem) {
        return None;
    }
    if rem == 5 && words[itu + 2] != "R" {
        return None;
    }

    chkcall(&words[itu])?;
    chkcall(&words[itu + 1])?;

    let crpt = &words[nwords - 2];
    if crpt.len() < 3 || crpt.contains('-') || crpt.contains('+') {
        return None;
    }
    let cb = crpt.as_bytes();
    if !(cb[0] == b'5' && (b'2'..=b'9').contains(&cb[1]) && cb[2] == b'9' && crpt.len() == 3) {
        return None;
    }

    let rpt_val: i32 = crpt.parse().ok()?;
    let irpt = ((rpt_val - 509) / 10 - 2).clamp(0, 7) as u64;

    let exch_word = &words[nwords - 1];
    let imult = RTTY_MULTS.iter().position(|&m| m == exch_word);
    let nexch: u64 = if let Some(idx) = imult {
        8000 + (idx as u64) + 1
    } else {
        let nserial: i32 = exch_word.parse().ok()?;
        if !(1..=7999).contains(&nserial) {
            return None;
        }
        nserial as u64
    };

    let n28a = pack28(&words[itu], Some(hash_table), my_call)? as u64;
    let n28b = pack28(&words[itu + 1], Some(hash_table), my_call)? as u64;
    let ir = if words[itu + 2] == "R" { 1u64 } else { 0u64 };

    let mut bw = BitWriter::new();
    bw.write_u64(itu as u64, 1);
    bw.write_u64(n28a, 28);
    bw.write_u64(n28b, 28);
    bw.write_u64(ir, 1);
    bw.write_u64(irpt, 3);
    bw.write_u64(nexch, 13);
    bw.write_u64(3, 3); // i3 = 3
    Some(bw.finish())
}

fn encode_c11_base38(call: &str) -> u64 {
    let bytes: Vec<u8> = call.bytes().take(11).collect();
    let pad = 11usize.saturating_sub(bytes.len());
    let mut n58: u64 = 0;
    for _ in 0..pad {
        n58 *= 38;
    }
    for &b in &bytes {
        n58 = n58 * 38 + hash_char_index(b);
    }
    n58
}

fn pack_4_nonstandard(
    words: &[String],
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<[u8; PAYLOAD_BITS]> {
    let nwords = words.len();
    if !(2..=3).contains(&nwords) {
        return None;
    }

    let strip_angles = |s: &str| -> String {
        if s.starts_with('<') && s.ends_with('>') && s.len() >= 2 {
            s[1..s.len() - 1].to_string()
        } else if let Some(rest) = s.strip_prefix('<') {
            rest.trim_end_matches('>').to_string()
        } else {
            s.to_string()
        }
    };

    let call_1 = strip_angles(&words[0]);
    let call_2 = strip_angles(&words[1]);
    let b1 = chkcall(&call_1);
    let b2 = chkcall(&call_2);

    if let (Some(ref bc1), Some(ref bc2)) = (&b1, &b2) {
        if call_1 == *bc1 && call_2 == *bc2 {
            return None;
        }
    }

    let is_cq = words[0] == "CQ";
    if is_cq && words[1].len() <= 4 {
        return None;
    }

    let (n12, c11_src, iflip) = if is_cq {
        let hashes = hash_table.save_call(&words[1], my_call);
        let n12 = hashes.map_or(0, |h| h.h12 as u64);
        (n12, call_2.as_str(), 0u64)
    } else if words[0].starts_with('<') {
        let hashes = hash_table.save_call(&words[0], my_call);
        let n12 = hashes.map_or(0, |h| h.h12 as u64);
        (n12, call_2.as_str(), 0u64)
    } else if words[1].starts_with('<') {
        let hashes = hash_table.save_call(&words[1], my_call);
        let n12 = hashes.map_or(0, |h| h.h12 as u64);
        (n12, call_1.as_str(), 1u64)
    } else {
        return None;
    };

    let n58 = encode_c11_base38(c11_src);
    let (icq, nrpt) = if is_cq {
        (1u64, 0u64)
    } else {
        let r = if nwords == 3 {
            match words[2].as_str() {
                "RRR" => 1u64,
                "RR73" => 2u64,
                "73" => 3u64,
                _ => 0u64,
            }
        } else {
            0u64
        };
        (0u64, r)
    };

    let mut bw = BitWriter::new();
    bw.write_u64(n12, 12);
    bw.write_u64(n58, 58);
    bw.write_u64(iflip, 1);
    bw.write_u64(nrpt, 2);
    bw.write_u64(icq, 1);
    bw.write_u64(4, 3); // i3 = 4
    Some(bw.finish())
}

fn pack_5_euvhf_hash(
    words: &[String],
    hash_table: &CallsignHashTable,
    my_call: Option<&str>,
) -> Option<[u8; PAYLOAD_BITS]> {
    let nwords = words.len();
    if !(4..=5).contains(&nwords) {
        return None;
    }
    if !words[0].starts_with('<') || !words[1].starts_with('<') {
        return None;
    }
    let gt1 = words[0].find('>')?;
    let gt2 = words[1].find('>')?;
    if nwords == 5 && words[2] != "R" {
        return None;
    }

    let nx: i32 = words[nwords - 2].parse().ok()?;
    if !(520_001..=594_095).contains(&nx) {
        return None;
    }
    let grid6 = &words[nwords - 1];
    if !is_grid6_strict(grid6) {
        return None;
    }

    hash_table.save_call(&words[0], my_call);
    let n12 = ihashcall(&words[0][1..gt1], 12) as u64;
    hash_table.save_call(&words[1], my_call);
    let n22 = ihashcall(&words[1][1..gt2], 22) as u64;

    let ir = if nwords == 5 && words[2] == "R" {
        1u64
    } else {
        0u64
    };
    let irpt = (nx / 10_000 - 52) as u64;
    let iserial = ((nx % 10_000).min(2047)) as u64;

    let gb = grid6.as_bytes();
    let igrid6 = ((gb[0] - b'A') as u64) * 18 * 10 * 10 * 24 * 24
        + ((gb[1] - b'A') as u64) * 10 * 10 * 24 * 24
        + ((gb[2] - b'0') as u64) * 10 * 24 * 24
        + ((gb[3] - b'0') as u64) * 24 * 24
        + ((gb[4] - b'A') as u64) * 24
        + ((gb[5] - b'A') as u64);

    let mut bw = BitWriter::new();
    bw.write_u64(n12, 12);
    bw.write_u64(n22, 22);
    bw.write_u64(ir, 1);
    bw.write_u64(irpt, 3);
    bw.write_u64(iserial, 11);
    bw.write_u64(igrid6, 25);
    bw.write_u64(5, 3); // i3 = 5
    Some(bw.finish())
}

fn pack_00_freetext(msg_collapsed: &str) -> [u8; PAYLOAD_BITS] {
    let mut c13 = [b' '; 13];
    let prefix13: String = msg_collapsed.chars().take(13).collect();
    let trimmed = prefix13.trim_end_matches(' ');
    let src = trimmed.as_bytes();
    let pad = 13 - src.len();
    // Right-justify (Fortran adjustr) into 13 chars after trimming trailing spaces.
    c13[pad..].copy_from_slice(src);

    let mut val: u128 = 0;
    for &ch in &c13 {
        let j = FREETEXT_ALPHABET
            .iter()
            .position(|&x| x == ch)
            .unwrap_or(0) as u128;
        val = val * 42 + j;
    }

    let mut bw = BitWriter::new();
    bw.write_u128(val, 71);
    bw.write_u64(0, 3); // n3 = 0
    bw.write_u64(0, 3); // i3 = 0
    bw.finish()
}

/// Encodes a text message into 77 bits (`pack77` in `packjt77.f90`) and performs
/// the `nrx = 0` confirmation unpack (`msgsent`).
pub fn pack77(
    msg: &str,
    hint: Option<PackHint>,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<PackedMessage, FtxrError> {
    let mut work = msg.to_string();
    if work.starts_with("$DX") {
        let dx_base = ctx.dx_base.as_deref().unwrap_or("").trim_end();
        if let Some(sp) = work.find(' ') {
            work = format!("{dx_base} {}", &work[sp + 1..]);
        } else {
            work = dx_base.to_string();
        }
    }

    let my_call = ctx.my_call.as_deref();

    if hint == Some(PackHint::Telemetry) {
        let upper = fmtmsg(&work);
        let (msg_type, bits) = match pack_05_telemetry(&upper) {
            TelemetryOutcome::Packed(b) => (MessageType::Telemetry, b),
            TelemetryOutcome::ForceFreeText | TelemetryOutcome::NotTelemetry => {
                (MessageType::FreeText, pack_00_freetext(&upper))
            }
        };
        let (i3, n3) = msg_type.i3_n3();
        let msgsent = unpack77(&bits, UnpackMode::Tx, ctx, hash_table)
            .map(|u| u.text)
            .unwrap_or_default();
        return Ok(PackedMessage {
            bits,
            i3,
            n3,
            msg_type,
            msgsent,
        });
    }

    let (collapsed, words) = split77(&work);
    let skip_early = collapsed == "CQ"
        || collapsed.starts_with("CQ ")
        || collapsed == "DE"
        || collapsed.starts_with("DE ")
        || collapsed == "QRZ"
        || collapsed.starts_with("QRZ ");

    let (msg_type, bits) = 'dispatch: {
        if !skip_early {
            if let Some(b) = pack_01_dxpedition(&words, hash_table, my_call) {
                break 'dispatch (MessageType::Dxpedition, b);
            }
            if let Some((n3, b)) = pack_03_field_day(&words, hash_table, my_call) {
                let mt = if n3 == 3 {
                    MessageType::FieldDayLow
                } else {
                    MessageType::FieldDayHigh
                };
                break 'dispatch (mt, b);
            }
            if words.len() < 2 {
                match pack_05_telemetry(&collapsed) {
                    TelemetryOutcome::Packed(b) => break 'dispatch (MessageType::Telemetry, b),
                    TelemetryOutcome::ForceFreeText => {
                        break 'dispatch (MessageType::FreeText, pack_00_freetext(&collapsed));
                    }
                    TelemetryOutcome::NotTelemetry => {}
                }
            }
        }

        if let Some(b) = pack_06_wspr(&words, hint, hash_table, my_call) {
            break 'dispatch (MessageType::Wspr, b);
        }
        if let Some((i3, b)) = pack_1_2_standard(&words, hash_table, my_call) {
            let mt = if i3 == 2 {
                MessageType::EuVhfPortable
            } else {
                MessageType::Standard
            };
            break 'dispatch (mt, b);
        }
        if let Some(b) = pack_3_rtty(&words, hash_table, my_call) {
            break 'dispatch (MessageType::ArrlRtty, b);
        }
        if let Some(b) = pack_4_nonstandard(&words, hash_table, my_call) {
            break 'dispatch (MessageType::Nonstandard, b);
        }
        if let Some(b) = pack_5_euvhf_hash(&words, hash_table, my_call) {
            break 'dispatch (MessageType::EuVhfHashed, b);
        }
        (MessageType::FreeText, pack_00_freetext(&collapsed))
    };

    let (i3, n3) = msg_type.i3_n3();
    let unpacked = unpack77(&bits, UnpackMode::Tx, ctx, hash_table).map_err(|_| {
        FtxrError::InvalidMessage(format!("message {msg:?} failed TX unpack verification"))
    })?;

    Ok(PackedMessage {
        bits,
        i3,
        n3,
        msg_type,
        msgsent: unpacked.text,
    })
}
