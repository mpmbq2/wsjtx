// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Bit-packed GF(2) Ordered Statistics Decoder `osd174_91` (Stage 3; Stage 2 stub).

use super::bp::LDPC_N;

/// Reusable scratch buffers for `osd174_91` (populated in Stage 3).
#[derive(Debug, Clone, Default)]
pub struct OsdScratch {
    _stage3: (),
}

impl OsdScratch {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Result returned by `osd174_91` (populated in Stage 3).
#[derive(Debug, Clone, Copy)]
pub struct OsdResult {
    pub message91: [u8; 91],
    pub cw: [u8; LDPC_N],
    pub nhardmin: i32,
    pub crc_ok: bool,
    pub dmin: f32,
    pub winning_order: u8,
}

/// Stage 2 stub for `osd174_91` (always returns `None` until Stage 3).
pub fn osd174_91(
    _llr: &[f32; LDPC_N],
    _keff: usize,
    _apmask: &[u8; LDPC_N],
    _norder: u8,
    _scratch: &mut OsdScratch,
) -> Option<OsdResult> {
    None
}
