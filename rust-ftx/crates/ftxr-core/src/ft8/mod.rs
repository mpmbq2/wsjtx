// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Classic FT8 receive pipeline (Stage 2 MVP, Stage 3 weak-signal parity).

pub mod ap;
pub mod decoder;
pub mod ft8b;
pub mod params;
pub mod subtract;
pub mod sync8;
pub mod sync8d;

pub use ap::QsoProgress;
pub use decoder::{Ft8DecodeOptions, Ft8Decoder, Ft8SlotSession, Ft8StagedMode};
