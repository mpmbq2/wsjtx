// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! LDPC(174,91) Belief Propagation (Stage 2) and Ordered Statistics Decoding (Stage 3).

pub mod bp;
pub mod decode174_91;
pub mod osd;

pub use bp::{BpDecodeOutput, LdpcDecoded, LDPC_K, LDPC_M, LDPC_N};
pub use decode174_91::decode174_91;
pub use osd::{osd174_91, OsdResult, OsdScratch};
