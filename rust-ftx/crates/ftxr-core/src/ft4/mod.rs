// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 physical-layer parameters, Costas synchronization, bit-metric extraction, AP passes,
//! subtraction, and single-slot RX decoder (`wsjtx/lib/ft4/`).

pub mod ap;
pub mod bitmetrics;
pub mod candidates;
pub mod decoder;
pub mod params;
pub mod subtract;
pub mod sync4d;

pub use decoder::{Ft4DecodeOptions, Ft4Decoder, Ft4Plans, Ft4Probe};
