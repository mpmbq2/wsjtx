// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Classic FT4 receive pipeline (Stage 4 parity).

pub mod ap;
pub mod bitmetrics;
pub mod candidates;
pub mod decoder;
pub mod params;
pub mod subtract;
pub mod sync4d;
