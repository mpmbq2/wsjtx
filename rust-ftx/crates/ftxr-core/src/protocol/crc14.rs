// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Exact WSJT-X CRC-14 (`0x2757` / `0x6757`) over the 96-bit augmented layout.
//!
//! Layout (`wsjtx/lib/ft8/encode174_91.f90`, `wsjtx/lib/crc14.cpp`):
//! - Bits `0..=76`  (77 bits): payload
//! - Bits `77..=81` (5 bits):  `00000` zero padding
//! - Bits `82..=95` (14 bits): `0` on encode, or received 14-bit CRC on verify

/// Truncated 14-bit CRC polynomial (`0x2757`, implicit `x^14` term).
pub const CRC14_POLY: u16 = 0x2757;
/// Full 15-bit CRC generator polynomial (`0x6757`).
pub const CRC14_FULL_POLY: u16 = 0x6757;

const fn build_crc14_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut h = 0usize;
    while h < 256 {
        let mut reg = (h as u16) << 6;
        let mut bit = 0;
        while bit < 8 {
            let msb = (reg >> 13) & 1;
            reg = (reg << 1) & 0x3FFF;
            if msb != 0 {
                reg ^= CRC14_POLY;
            }
            bit += 1;
        }
        table[h] = reg;
        h += 1;
    }
    table
}

const CRC14_TABLE: [u16; 256] = build_crc14_table();

/// Packs 77 MSB-first payload bits into 12 bytes with 5 zero-pad bits + 14 zero CRC bits.
pub fn pack_msg77_to_bytes12(msg77: &[u8; 77]) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    for (i, &bit) in msg77.iter().enumerate() {
        if (bit & 1) != 0 {
            bytes[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    bytes
}

/// Packs 91 MSB-first bits (`77 msg + 14 crc`) into the 12-byte augmented CRC-14 layout
/// (`77 msg | 5 zero-pad | 14 crc`).
pub fn pack_msg91_to_bytes12(msg91: &[u8; 91]) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    for i in 0..77 {
        if (msg91[i] & 1) != 0 {
            bytes[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    // Bits 77..=81 remain zero; bits 82..=95 receive msg91[77..91].
    for i in 0..14 {
        if (msg91[77 + i] & 1) != 0 {
            let dst = 82 + i;
            bytes[dst / 8] |= 1 << (7 - (dst % 8));
        }
    }
    bytes
}

/// Computes `boost::augmented_crc<14, 0x2757>` over 12 bytes using the const 256-entry table.
#[inline]
pub fn crc14_bytes12(bytes: &[u8; 12]) -> u16 {
    let mut reg: u16 = 0;
    for &b in bytes {
        let top = (reg >> 6) as usize;
        reg = (((reg & 0x3F) << 8) | (b as u16)) ^ CRC14_TABLE[top];
    }
    reg & 0x3FFF
}

/// Bit-by-bit reference CRC-14 computation over 12 bytes (`boost::augmented_crc` equivalent).
pub fn crc14_bytes12_bitwise(bytes: &[u8; 12]) -> u16 {
    let mut reg: u16 = 0;
    for &b in bytes {
        for shift in (0..8).rev() {
            let bit = ((b >> shift) & 1) as u16;
            let msb = (reg >> 13) & 1;
            reg = ((reg << 1) | bit) & 0x3FFF;
            if msb != 0 {
                reg ^= CRC14_POLY;
            }
        }
    }
    reg & 0x3FFF
}

/// Computes the 14-bit WSJT-X CRC for a 77-bit payload (`0..16384`).
pub fn compute_crc14(msg77: &[u8; 77]) -> u16 {
    let bytes = pack_msg77_to_bytes12(msg77);
    crc14_bytes12(&bytes)
}

/// Verifies the 14-bit CRC inside a 91-bit `message || crc14` slice (`chkcrc14a` / `crc14_check`).
pub fn verify_crc14(msg91: &[u8; 91]) -> bool {
    let bytes = pack_msg91_to_bytes12(msg91);
    crc14_bytes12(&bytes) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_bitwise_across_10000_random_payloads() {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next_u64 = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for _ in 0..10_000 {
            let mut msg77 = [0u8; 77];
            let a = next_u64();
            let b = next_u64();
            for (i, bit) in msg77.iter_mut().take(64).enumerate() {
                *bit = ((a >> i) & 1) as u8;
            }
            for (i, bit) in msg77.iter_mut().skip(64).take(13).enumerate() {
                *bit = ((b >> i) & 1) as u8;
            }
            let bytes = pack_msg77_to_bytes12(&msg77);
            let t = crc14_bytes12(&bytes);
            let bw = crc14_bytes12_bitwise(&bytes);
            assert_eq!(t, bw);

            let mut msg91 = [0u8; 91];
            msg91[..77].copy_from_slice(&msg77);
            for i in 0..14 {
                msg91[77 + i] = ((t >> (13 - i)) & 1) as u8;
            }
            assert!(verify_crc14(&msg91));
        }
    }

    #[test]
    fn single_bit_flip_detected_everywhere() {
        let mut msg77 = [0u8; 77];
        for (i, b) in msg77.iter_mut().enumerate() {
            *b = ((i * 7 + 3) % 2) as u8;
        }
        let crc = compute_crc14(&msg77);
        let mut msg91 = [0u8; 91];
        msg91[..77].copy_from_slice(&msg77);
        for i in 0..14 {
            msg91[77 + i] = ((crc >> (13 - i)) & 1) as u8;
        }
        assert!(verify_crc14(&msg91));

        for pos in 0..91 {
            let mut corrupted = msg91;
            corrupted[pos] ^= 1;
            assert!(!verify_crc14(&corrupted), "bit flip at {pos} went undetected");
        }
    }
}
