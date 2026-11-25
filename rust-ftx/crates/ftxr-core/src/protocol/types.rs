// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! 77-bit message protocol types and lookup tables (`packjt77.f90`).

/// Maximum character length of a WSJT-X 77-bit text message.
pub const MAX_MSG_LEN: usize = 37;
/// Number of payload bits in an FT8/FT4 message.
pub const PAYLOAD_BITS: usize = 77;
/// Number of CRC-14 bits appended to the 77-bit payload.
pub const CRC14_BITS: usize = 14;
/// Total systematic information + CRC bits (`77 + 14`).
pub const LDPC_K: usize = 91;
/// Number of LDPC parity bits (`174 - 91`).
pub const LDPC_M: usize = 83;
/// Total LDPC(174,91) codeword length in bits.
pub const LDPC_N: usize = 174;

/// WSJT-X 77-bit protocol message type (`i3` and `n3` discriminators).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// `i3=0, n3=0`: 13-character base-42 free text.
    FreeText,
    /// `i3=0, n3=1`: DXpedition (`CALL1 RR73; CALL2 <FOX> RPT`).
    Dxpedition,
    /// `i3=0, n3=3`: ARRL Field Day (1..=16 transmitters).
    FieldDayLow,
    /// `i3=0, n3=4`: ARRL Field Day (17..=32 transmitters).
    FieldDayHigh,
    /// `i3=0, n3=5`: Telemetry (up to 18 hex digits, MSB < 2^23).
    Telemetry,
    /// `i3=0, n3=6`: WSPR 50-bit message (Types 1, 2, 3).
    Wspr,
    /// `i3=1`: Standard message (optional `/R` suffix).
    Standard,
    /// `i3=2`: EU VHF contest message (optional `/P` suffix).
    EuVhfPortable,
    /// `i3=3`: ARRL RTTY Roundup exchange.
    ArrlRtty,
    /// `i3=4`: Nonstandard callsign + 12-bit hashed callsign.
    Nonstandard,
    /// `i3=5`: EU VHF contest exchange with 12-bit & 22-bit hashed calls and 6-char grid.
    EuVhfHashed,
}

impl MessageType {
    /// Returns the canonical `(i3, n3)` tuple (`n3 = 0` when `i3 >= 1`).
    pub const fn i3_n3(self) -> (u8, u8) {
        match self {
            Self::FreeText => (0, 0),
            Self::Dxpedition => (0, 1),
            Self::FieldDayLow => (0, 3),
            Self::FieldDayHigh => (0, 4),
            Self::Telemetry => (0, 5),
            Self::Wspr => (0, 6),
            Self::Standard => (1, 0),
            Self::EuVhfPortable => (2, 0),
            Self::ArrlRtty => (3, 0),
            Self::Nonstandard => (4, 0),
            Self::EuVhfHashed => (5, 0),
        }
    }

    /// Maps an `(i3, n3)` discriminator pair to a `MessageType`, returning `None` for
    /// deprecated `0.2`, undefined `0.7`, or undefined `i3 >= 6`.
    pub const fn from_i3_n3(i3: u8, n3: u8) -> Option<Self> {
        match i3 {
            0 => match n3 {
                0 => Some(Self::FreeText),
                1 => Some(Self::Dxpedition),
                3 => Some(Self::FieldDayLow),
                4 => Some(Self::FieldDayHigh),
                5 => Some(Self::Telemetry),
                6 => Some(Self::Wspr),
                _ => None,
            },
            1 => Some(Self::Standard),
            2 => Some(Self::EuVhfPortable),
            3 => Some(Self::ArrlRtty),
            4 => Some(Self::Nonstandard),
            5 => Some(Self::EuVhfHashed),
            _ => None,
        }
    }
}

/// Optional encoder hint matching WSJT-X `pack77` `i3_hint`/`n3_hint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackHint {
    /// Force Telemetry (`i3=0, n3=5`) check first (falls back to `0.0` if invalid).
    Telemetry,
    /// Enable WSPR Type 3 (`i3=0, n3=6`) encoding for `<CALL> GRID4/6`.
    Wspr,
}

/// Controls `my_call`/`dx_call` hash-override rules during `unpack77` (`nrx = 0` vs `nrx = 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackMode {
    /// TX confirmation mode (`nrx = 0`).
    Tx,
    /// RX decode mode (`nrx = 1`).
    Rx,
}

/// Optional callsign context passed to `pack77` and `unpack77`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnpackContext {
    /// Local station callsign (`mycall13` in WSJT-X).
    pub my_call: Option<String>,
    /// Target DX station callsign (`dxcall13` in WSJT-X).
    pub dx_call: Option<String>,
    /// Base DX callsign used for `$DX` substitution in `pack77` (`dxbase` in WSJT-X).
    pub dx_base: Option<String>,
}

/// Result of packing a message into 77 bits (`pack77` + `nrx = 0` verification unpack).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedMessage {
    /// 77 payload bits (`0` or `1` per entry), MSB-first.
    pub bits: [u8; PAYLOAD_BITS],
    pub i3: u8,
    pub n3: u8,
    pub msg_type: MessageType,
    /// Unpacked confirmation string (`msgsent` from `nrx = 0`).
    pub msgsent: String,
}

/// Deferred hash-table mutations emitted by `unpack77_readonly` so parallel workers
/// never mutate shared hash tables out of order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingHashUpdates {
    /// Callsigns to pass to `CallsignHashTable::save_call` in candidate order.
    pub save_calls: Vec<String>,
    /// Callsigns to pass to `CallsignHashTable::add_recent` in candidate order.
    pub recent_calls: Vec<String>,
}

/// Result of unpacking a 77-bit payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackedMessage {
    pub text: String,
    pub i3: u8,
    pub n3: u8,
    pub msg_type: MessageType,
    pub pending_hashes: PendingHashUpdates,
}

/// 86 ARRL Field Day sections (`packjt77.f90:300-308`, 1-based index `1..=86`).
#[rustfmt::skip]
pub const ARRL_SECTIONS: [&str; 86] = [
    "AB", "AK", "AL", "AR", "AZ", "BC", "CO", "CT", "DE", "EB",
    "EMA", "ENY", "EPA", "EWA", "GA", "GH", "IA", "ID", "IL", "IN",
    "KS", "KY", "LA", "LAX", "NS", "MB", "MDC", "ME", "MI", "MN",
    "MO", "MS", "MT", "NC", "ND", "NE", "NFL", "NH", "NL", "NLI",
    "NM", "NNJ", "NNY", "TER", "NTX", "NV", "OH", "OK", "ONE", "ONN",
    "ONS", "OR", "ORG", "PAC", "PR", "QC", "RI", "SB", "SC", "SCV",
    "SD", "SDG", "SF", "SFL", "SJV", "SK", "SNJ", "STX", "SV", "TN",
    "UT", "VA", "VI", "VT", "WCF", "WI", "WMA", "WNY", "WPA", "WTX",
    "WV", "WWA", "WY", "DX", "PE", "NB",
];

/// 171 ARRL RTTY Roundup multipliers (`packjt77.f90:310-327`, 1-based index `1..=171`).
#[rustfmt::skip]
pub const RTTY_MULTS: [&str; 171] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA",
    "HI", "ID", "IL", "IN", "IA", "KS", "KY", "LA", "ME", "MD",
    "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ",
    "NM", "NY", "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC",
    "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV", "WI", "WY",
    "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NWT", "NF",
    "LB", "NU", "YT", "PEI", "DC", "DR", "FR", "GD", "GR", "OV",
    "ZH", "ZL", "X01", "X02", "X03", "X04", "X05", "X06", "X07", "X08",
    "X09", "X10", "X11", "X12", "X13", "X14", "X15", "X16", "X17", "X18",
    "X19", "X20", "X21", "X22", "X23", "X24", "X25", "X26", "X27", "X28",
    "X29", "X30", "X31", "X32", "X33", "X34", "X35", "X36", "X37", "X38",
    "X39", "X40", "X41", "X42", "X43", "X44", "X45", "X46", "X47", "X48",
    "X49", "X50", "X51", "X52", "X53", "X54", "X55", "X56", "X57", "X58",
    "X59", "X60", "X61", "X62", "X63", "X64", "X65", "X66", "X67", "X68",
    "X69", "X70", "X71", "X72", "X73", "X74", "X75", "X76", "X77", "X78",
    "X79", "X80", "X81", "X82", "X83", "X84", "X85", "X86", "X87", "X88",
    "X89", "X90", "X91", "X92", "X93", "X94", "X95", "X96", "X97", "X98",
    "X99",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn lookup_tables_valid_and_unique() {
        assert_eq!(ARRL_SECTIONS.len(), 86);
        let sec_set: HashSet<&str> = ARRL_SECTIONS.iter().copied().collect();
        assert_eq!(sec_set.len(), 86);

        assert_eq!(RTTY_MULTS.len(), 171);
        let mult_set: HashSet<&str> = RTTY_MULTS.iter().copied().collect();
        assert_eq!(mult_set.len(), 171);
    }

    #[test]
    fn message_type_i3_n3_roundtrip_and_rejects_invalid() {
        let all = [
            MessageType::FreeText,
            MessageType::Dxpedition,
            MessageType::FieldDayLow,
            MessageType::FieldDayHigh,
            MessageType::Telemetry,
            MessageType::Wspr,
            MessageType::Standard,
            MessageType::EuVhfPortable,
            MessageType::ArrlRtty,
            MessageType::Nonstandard,
            MessageType::EuVhfHashed,
        ];
        for mt in all {
            let (i3, n3) = mt.i3_n3();
            assert_eq!(MessageType::from_i3_n3(i3, n3), Some(mt));
        }
        assert_eq!(MessageType::from_i3_n3(0, 2), None);
        assert_eq!(MessageType::from_i3_n3(0, 7), None);
        assert_eq!(MessageType::from_i3_n3(6, 0), None);
        assert_eq!(MessageType::from_i3_n3(7, 0), None);
    }
}
