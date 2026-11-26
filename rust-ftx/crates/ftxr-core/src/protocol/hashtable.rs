// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Thread-safe, session-scoped WSJT-X callsign hash table (`packjt77.f90`).
//!
//! Zero global/static mutable state: each `DecoderEngine` or caller owns a
//! `CallsignHashTable` (`Send + Sync`, `RwLock`-backed).

use std::sync::RwLock;

use super::types::PendingHashUpdates;

/// Maximum number of 22-bit hashed callsigns retained (`MAXHASH` in `packjt77.f90`).
pub const MAX_HASH22: usize = 1000;
/// Maximum number of recent callsigns retained (`MAXRECENT` in `packjt77.f90`).
pub const MAX_RECENT_CALLS: usize = 10;
/// 38-character base alphabet used by `ihashcall` and Type 4 `n58`.
pub const HASH_ALPHABET: &[u8; 38] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";
/// 64-bit multiplicative hash constant (`2419 + 62655 * 2^16 + 10 * 2^32`).
pub const HASH_MULT: u64 = 47_055_833_459;

/// Precomputed 10-, 12-, and 22-bit hashes for a callsign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallsignHashes {
    pub h10: u16,
    pub h12: u16,
    pub h22: u32,
}

#[inline]
pub(crate) fn hash_char_index(b: u8) -> u64 {
    match b {
        b' ' => 0,
        b'0'..=b'9' => (b - b'0' + 1) as u64,
        b'A'..=b'Z' => (b - b'A' + 11) as u64,
        b'/' => 37,
        b'a'..=b'z' => (b - b'a' + 11) as u64,
        _ => 0,
    }
}

/// Computes the 10-, 12-, or 22-bit WSJT-X callsign hash (`ihashcall` in `packjt77.f90`).
///
/// The callsign is left-justified and right-padded with spaces to 11 characters,
/// encoded in base 38, multiplied by `47_055_833_459` mod `2^64`, and shifted
/// right by `64 - bits`.
pub fn ihashcall(call: &str, bits: u8) -> u32 {
    if bits == 0 || bits > 32 {
        return 0;
    }
    let bytes = call.as_bytes();
    let mut n8: u64 = 0;
    for i in 0..11 {
        let ch = bytes.get(i).copied().unwrap_or(b' ');
        n8 = n8 * 38 + hash_char_index(ch);
    }
    ((HASH_MULT.wrapping_mul(n8)) >> (64 - bits)) as u32
}

/// Computes all three WSJT-X callsign hashes (`10`, `12`, `22` bits) for `call`.
pub fn compute_all_hashes(call: &str) -> CallsignHashes {
    let bytes = call.as_bytes();
    let mut n8: u64 = 0;
    for i in 0..11 {
        let ch = bytes.get(i).copied().unwrap_or(b' ');
        n8 = n8 * 38 + hash_char_index(ch);
    }
    let prod = HASH_MULT.wrapping_mul(n8);
    CallsignHashes {
        h10: (prod >> (64 - 10)) as u16,
        h12: (prod >> (64 - 12)) as u16,
        h22: (prod >> (64 - 22)) as u32,
    }
}

/// Sanitizes a raw callsign token for hash insertion following `save_hash_call`
/// (`packjt77.f90:154-160`).
pub(crate) fn sanitize_hash_call(raw_call: &str) -> Option<String> {
    if raw_call.is_empty() || raw_call.starts_with(' ') || raw_call.starts_with("<...>") {
        return None;
    }
    let s = raw_call.strip_prefix('<').unwrap_or(raw_call);
    let s = match s.find('>') {
        Some(idx) => &s[..idx],
        None => s,
    };
    // Match Fortran character*13 truncation + trim.
    let mut clean: String = s.chars().take(13).collect();
    while clean.ends_with(' ') {
        clean.pop();
    }
    if clean.len() < 3 {
        return None;
    }
    Some(clean)
}

const COMPACT_CALL_CAP: usize = 23;

/// Inline `Copy` callsign buffer avoiding per-entry heap allocations inside the hash table and pass snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactCall {
    bytes: [u8; COMPACT_CALL_CAP],
    len: u8,
}

impl Default for CompactCall {
    fn default() -> Self {
        Self {
            bytes: [0u8; COMPACT_CALL_CAP],
            len: 0,
        }
    }
}

impl CompactCall {
    pub fn from_str_clean(s: &str) -> Self {
        let b = s.as_bytes();
        let mut copy_len = b.len().min(COMPACT_CALL_CAP);
        while copy_len > 0 && !s.is_char_boundary(copy_len) {
            copy_len -= 1;
        }
        let mut bytes = [0u8; COMPACT_CALL_CAP];
        bytes[..copy_len].copy_from_slice(&b[..copy_len]);
        Self {
            bytes,
            len: copy_len as u8,
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}

/// Read-only callsign hash lookup trait shared between live `CallsignHashTable` and lock-free `PassHashSnapshot`.
pub trait CallsignLookup: Sync {
    fn lookup_10(&self, h10: u16) -> Option<String>;
    fn lookup_12(&self, h12: u16) -> Option<String>;
    fn lookup_22(&self, h22: u32) -> Option<String>;

    fn format_10(&self, h10: u16) -> String {
        match self.lookup_10(h10) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }

    fn format_12(&self, h12: u16) -> String {
        match self.lookup_12(h12) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }

    fn format_22(&self, h22: u32) -> String {
        match self.lookup_22(h22) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct HashTableInner {
    calls10: Box<[Option<CompactCall>; 1024]>,
    calls12: Box<[Option<CompactCall>; 4096]>,
    /// Front = most recent; max length `MAX_HASH22` (1000).
    calls22: Vec<(u32, CompactCall)>,
    recent_calls: Vec<CompactCall>,
}

impl HashTableInner {
    fn new() -> Self {
        Self {
            calls10: vec![None; 1024]
                .into_boxed_slice()
                .try_into()
                .expect("exact 1024 length"),
            calls12: vec![None; 4096]
                .into_boxed_slice()
                .try_into()
                .expect("exact 4096 length"),
            calls22: Vec::with_capacity(MAX_HASH22),
            recent_calls: Vec::with_capacity(MAX_RECENT_CALLS),
        }
    }

    fn clear(&mut self) -> usize {
        let count = self.entry_count();
        self.calls10.fill(None);
        self.calls12.fill(None);
        self.calls22.clear();
        self.recent_calls.clear();
        count
    }

    #[inline]
    fn entry_count(&self) -> usize {
        self.calls22.len()
    }

    fn clone_into(&self, dst: &mut HashTableInner) {
        dst.calls10.copy_from_slice(&self.calls10[..]);
        dst.calls12.copy_from_slice(&self.calls12[..]);
        dst.calls22.clear();
        dst.calls22.extend_from_slice(&self.calls22);
        dst.recent_calls.clear();
        dst.recent_calls.extend_from_slice(&self.recent_calls);
    }

    fn save_call(&mut self, raw_call: &str, my_call: Option<&str>) -> Option<CallsignHashes> {
        let clean = sanitize_hash_call(raw_call)?;
        let hashes = compute_all_hashes(&clean);
        let my_clean = my_call.map(|m| m.trim_end());
        let is_my_call = my_clean.is_some_and(|m| m == clean);
        let compact = CompactCall::from_str_clean(&clean);

        if !is_my_call {
            self.calls10[hashes.h10 as usize] = Some(compact);
            self.calls12[hashes.h12 as usize] = Some(compact);
        }

        if let Some((_, existing)) = self.calls22.iter_mut().find(|(h, _)| *h == hashes.h22) {
            *existing = compact;
        } else {
            if self.calls22.len() >= MAX_HASH22 {
                self.calls22.pop();
            }
            self.calls22.insert(0, (hashes.h22, compact));
        }

        Some(hashes)
    }

    fn add_recent(&mut self, call: &str) {
        let trimmed = call.trim_end();
        if trimmed.is_empty() {
            return;
        }
        // Check first MAX_RECENT_CALLS - 1 entries; if at the end (slot 9), add again.
        let already_recent = self
            .recent_calls
            .iter()
            .take(MAX_RECENT_CALLS - 1)
            .any(|c| c.as_str() == trimmed);
        if !already_recent {
            if self.recent_calls.len() >= MAX_RECENT_CALLS {
                self.recent_calls.pop();
            }
            self.recent_calls
                .insert(0, CompactCall::from_str_clean(trimmed));
        }
    }
}

/// Lock-free immutable snapshot of `CallsignHashTable` reused across decode passes with zero heap allocations.
#[derive(Debug, Clone)]
pub struct PassHashSnapshot {
    inner: HashTableInner,
}

impl Default for PassHashSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl PassHashSnapshot {
    pub fn new() -> Self {
        Self {
            inner: HashTableInner::new(),
        }
    }

    #[inline]
    pub fn lookup_10_str(&self, h10: u16) -> Option<&str> {
        if h10 >= 1024 {
            return None;
        }
        self.inner.calls10[h10 as usize].as_ref().map(CompactCall::as_str)
    }

    #[inline]
    pub fn lookup_12_str(&self, h12: u16) -> Option<&str> {
        if h12 >= 4096 {
            return None;
        }
        self.inner.calls12[h12 as usize].as_ref().map(CompactCall::as_str)
    }

    #[inline]
    pub fn lookup_22_str(&self, h22: u32) -> Option<&str> {
        self.inner
            .calls22
            .iter()
            .find(|(h, _)| *h == h22)
            .map(|(_, c)| c.as_str())
    }

    #[inline]
    pub fn format_10(&self, h10: u16) -> String {
        CallsignLookup::format_10(self, h10)
    }

    #[inline]
    pub fn format_12(&self, h12: u16) -> String {
        CallsignLookup::format_12(self, h12)
    }

    #[inline]
    pub fn format_22(&self, h22: u32) -> String {
        CallsignLookup::format_22(self, h22)
    }
}

impl CallsignLookup for PassHashSnapshot {
    #[inline]
    fn lookup_10(&self, h10: u16) -> Option<String> {
        self.lookup_10_str(h10).map(str::to_string)
    }

    #[inline]
    fn lookup_12(&self, h12: u16) -> Option<String> {
        self.lookup_12_str(h12).map(str::to_string)
    }

    #[inline]
    fn lookup_22(&self, h22: u32) -> Option<String> {
        self.lookup_22_str(h22).map(str::to_string)
    }
}

/// Thread-safe, session-scoped callsign hash table (`Send + Sync`, zero global state).
#[derive(Debug)]
pub struct CallsignHashTable {
    inner: RwLock<HashTableInner>,
}

impl Default for CallsignHashTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CallsignHashTable {
    /// Creates an empty hash table.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashTableInner::new()),
        }
    }

    /// Returns the number of tracked 22-bit hash entries in the current session.
    pub fn len(&self) -> usize {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        guard.entry_count()
    }

    /// Returns true if no hash entries are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears all 10-, 12-, and 22-bit hash entries and recent calls, returning entries cleared.
    pub fn clear(&self) -> usize {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        guard.clear()
    }

    /// Snapshots the table into a caller-owned `PassHashSnapshot` without heap allocations.
    pub fn snapshot_into(&self, dst: &mut PassHashSnapshot) {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        guard.clone_into(&mut dst.inner);
    }

    /// Allocates and returns a fresh immutable per-pass snapshot for lock-free worker reads.
    pub fn snapshot(&self) -> PassHashSnapshot {
        let mut snap = PassHashSnapshot::new();
        self.snapshot_into(&mut snap);
        snap
    }

    /// Exact port of `save_hash_call`: strips `<...>`, requires length >= 3,
    /// skips updating `calls10`/`calls12` when `clean == my_call`, and updates `calls22`.
    pub fn save_call(&self, raw_call: &str, my_call: Option<&str>) -> Option<CallsignHashes> {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        guard.save_call(raw_call, my_call)
    }

    /// Looks up a 10-bit hash, returning the bare callsign without `<>` if present.
    pub fn lookup_10(&self, h10: u16) -> Option<String> {
        if h10 >= 1024 {
            return None;
        }
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        guard.calls10[h10 as usize]
            .as_ref()
            .map(|c| c.as_str().to_string())
    }

    /// Looks up a 12-bit hash, returning the bare callsign without `<>` if present.
    pub fn lookup_12(&self, h12: u16) -> Option<String> {
        if h12 >= 4096 {
            return None;
        }
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        guard.calls12[h12 as usize]
            .as_ref()
            .map(|c| c.as_str().to_string())
    }

    /// Looks up a 22-bit hash, returning the bare callsign without `<>` if present.
    pub fn lookup_22(&self, h22: u32) -> Option<String> {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        guard
            .calls22
            .iter()
            .find(|(h, _)| *h == h22)
            .map(|(_, c)| c.as_str().to_string())
    }

    /// Formats a 10-bit hash lookup as `"<CALL>"` or `"<...>"`.
    pub fn format_10(&self, h10: u16) -> String {
        match self.lookup_10(h10) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }

    /// Formats a 12-bit hash lookup as `"<CALL>"` or `"<...>"`.
    pub fn format_12(&self, h12: u16) -> String {
        match self.lookup_12(h12) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }

    /// Formats a 22-bit hash lookup as `"<CALL>"` or `"<...>"`.
    pub fn format_22(&self, h22: u32) -> String {
        match self.lookup_22(h22) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }

    /// Records a callsign in the 10-entry recent callsign list (`add_call_to_recent_calls`).
    pub fn add_recent(&self, call: &str) {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        guard.add_recent(call);
    }

    /// Returns a snapshot of the recent callsign list (most recent first).
    pub fn recent_calls(&self) -> Vec<String> {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        guard
            .recent_calls
            .iter()
            .map(|c| c.as_str().to_string())
            .collect()
    }

    /// Applies deferred hash updates emitted by `unpack77_readonly` in deterministic order.
    pub fn apply_updates(&self, updates: &PendingHashUpdates, my_call: Option<&str>) {
        if updates.save_calls.is_empty() && updates.recent_calls.is_empty() {
            return;
        }
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        for call in &updates.recent_calls {
            guard.add_recent(call);
        }
        for call in &updates.save_calls {
            guard.save_call(call, my_call);
        }
    }

    /// Applies an ordered batch of deferred hash updates under one write lock for deterministic commit.
    pub fn commit_updates_batch(&self, batches: &[&PendingHashUpdates], my_call: Option<&str>) {
        if batches
            .iter()
            .all(|u| u.save_calls.is_empty() && u.recent_calls.is_empty())
        {
            return;
        }
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        for updates in batches {
            for call in &updates.recent_calls {
                guard.add_recent(call);
            }
            for call in &updates.save_calls {
                guard.save_call(call, my_call);
            }
        }
    }
}

impl CallsignLookup for CallsignHashTable {
    fn lookup_10(&self, h10: u16) -> Option<String> {
        CallsignHashTable::lookup_10(self, h10)
    }

    fn lookup_12(&self, h12: u16) -> Option<String> {
        CallsignHashTable::lookup_12(self, h12)
    }

    fn lookup_22(&self, h22: u32) -> Option<String> {
        CallsignHashTable::lookup_22(self, h22)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn ihashcall_known_vectors() {
        // Verify deterministic hash math and bit-width alignment.
        let h = compute_all_hashes("PJ4/K1ABC");
        assert_eq!(h.h10 as u32, ihashcall("PJ4/K1ABC", 10));
        assert_eq!(h.h12 as u32, ihashcall("PJ4/K1ABC", 12));
        assert_eq!(h.h22, ihashcall("PJ4/K1ABC", 22));
        assert!((h.h10 as u32) < 1024);
        assert!((h.h12 as u32) < 4096);
        assert!(h.h22 < (1 << 22));
        assert_eq!(ihashcall("PJ4/K1ABC", 10), ihashcall("PJ4/K1ABC  ", 10));
        assert_eq!(ihashcall("K1ABC", 22) >> 10, ihashcall("K1ABC", 12));
        assert_eq!(ihashcall("K1ABC", 12) >> 2, ihashcall("K1ABC", 10));
    }

    #[test]
    fn save_hash_mycall_exclusion_and_fifo_eviction() {
        let ht = CallsignHashTable::new();
        assert_eq!(ht.save_call("   ", None), None);
        assert_eq!(ht.save_call("<...>", None), None);
        assert_eq!(ht.save_call("AB", None), None);

        // my_call must NOT populate calls10/calls12, but DOES populate calls22.
        let my_h = ht.save_call("PJ4/K1ABC", Some("PJ4/K1ABC")).unwrap();
        assert_eq!(ht.lookup_10(my_h.h10), None);
        assert_eq!(ht.lookup_12(my_h.h12), None);
        assert_eq!(ht.lookup_22(my_h.h22).as_deref(), Some("PJ4/K1ABC"));

        // Different call updates all three tables.
        let other_h = ht.save_call("<KH1/KH7Z>", Some("PJ4/K1ABC")).unwrap();
        assert_eq!(ht.lookup_10(other_h.h10).as_deref(), Some("KH1/KH7Z"));
        assert_eq!(ht.lookup_12(other_h.h12).as_deref(), Some("KH1/KH7Z"));
        assert_eq!(ht.lookup_22(other_h.h22).as_deref(), Some("KH1/KH7Z"));
        assert_eq!(ht.format_22(other_h.h22), "<KH1/KH7Z>");
        assert_eq!(ht.format_22(0x3f_ffff), "<...>");

        // Verify 1001st distinct h22 entry evicts the oldest entry.
        let ht2 = CallsignHashTable::new();
        let mut first_h22 = None;
        let mut inserted = 0usize;
        let mut idx = 0u32;
        while inserted < MAX_HASH22 + 1 {
            let call = format!("X{idx:05}A");
            let h = ihashcall(&call, 22);
            if ht2.lookup_22(h).is_none() {
                ht2.save_call(&call, None);
                if first_h22.is_none() {
                    first_h22 = Some(h);
                }
                inserted += 1;
            }
            idx += 1;
        }
        assert_eq!(ht2.lookup_22(first_h22.unwrap()), None);
    }

    #[test]
    fn concurrent_read_write_stress() {
        let ht = Arc::new(CallsignHashTable::new());
        let mut handles = Vec::new();
        for t in 0..16u32 {
            let ht_c = Arc::clone(&ht);
            handles.push(thread::spawn(move || {
                for i in 0..200u32 {
                    let call = format!("K{t}AB{}", (b'A' + (i % 26) as u8) as char);
                    let h = ht_c.save_call(&call, None).unwrap();
                    let _ = ht_c.lookup_10(h.h10);
                    let _ = ht_c.lookup_12(h.h12);
                    let _ = ht_c.lookup_22(h.h22);
                    ht_c.add_recent(&call);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(!ht.recent_calls().is_empty());
    }

    #[test]
    fn snapshot_reuse_clear_count_and_batch_commit_determinism() {
        let ht = CallsignHashTable::new();
        assert_eq!(ht.len(), 0);
        assert!(ht.is_empty());

        let mut u0 = PendingHashUpdates::default();
        u0.save_calls.push("PJ4/K1ABC".to_string());
        let mut u1 = PendingHashUpdates::default();
        u1.save_calls.push("KH1/KH7Z".to_string());
        u1.recent_calls.push("KH1/KH7Z".to_string());

        ht.commit_updates_batch(&[&u0, &u1], None);
        assert_eq!(ht.len(), 2);

        let mut snap = PassHashSnapshot::new();
        ht.snapshot_into(&mut snap);
        let h_k1 = compute_all_hashes("PJ4/K1ABC");
        let h_kh = compute_all_hashes("KH1/KH7Z");
        assert_eq!(snap.lookup_22_str(h_k1.h22), Some("PJ4/K1ABC"));
        assert_eq!(snap.lookup_12_str(h_kh.h12), Some("KH1/KH7Z"));
        assert_eq!(snap.format_22(h_k1.h22), "<PJ4/K1ABC>");

        // Clearing returns exact previous entry count and empties snapshot on next copy.
        assert_eq!(ht.clear(), 2);
        assert_eq!(ht.len(), 0);
        assert_eq!(ht.clear(), 0);
        ht.snapshot_into(&mut snap);
        assert_eq!(snap.lookup_22_str(h_k1.h22), None);
    }
}
