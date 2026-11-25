// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 single-slot A Priori (`a1`–`a6`) hypothesis generator (Stage 3; Stage 2 stub).

use crate::{ApTag, ContestMode};

/// Stage 2 placeholder for Stage 3 AP context.
#[derive(Debug, Clone, Default)]
pub struct ApContext {
    pub contest: ContestMode,
    pub ap_enabled: bool,
}

/// Stage 2 placeholder for Stage 3 AP candidate outcome.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApCandidateResult {
    pub ap_tag: ApTag,
}
