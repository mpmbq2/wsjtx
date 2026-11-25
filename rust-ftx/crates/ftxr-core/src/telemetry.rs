// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Optional structured decode telemetry types (populated in Stage 5).

use serde::{Deserialize, Serialize};

/// Forward error correction path that successfully decoded a codeword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FecMethod {
    BeliefPropagation,
    Osd { order: u8 },
}

/// Optional rich diagnostic telemetry attached to a decoded frame when enabled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeTelemetry {
    pub fec_method: FecMethod,
    pub sync_score: f32,
    pub subtraction_pass: u8,
    pub hard_errors: u16,
    pub osd_distance: f32,
    pub quality: f32,
    pub freq_drift_hz: Option<f32>,
    pub qsb_envelope: Option<Vec<f32>>,
}
