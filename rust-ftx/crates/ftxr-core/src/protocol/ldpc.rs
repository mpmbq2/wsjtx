// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Systematic LDPC(174,91) encoder (`wsjtx/lib/ft8/encode174_91.f90`,
//! `encode174_91_nocrc.f90`, and `ldpc_174_91_c_generator.f90`).

use super::crc14::compute_crc14;

const HEX_GEN_ROWS: [&str; 83] = [
    "8329ce11bf31eaf509f27fc",
    "761c264e25c259335493132",
    "dc265902fb277c6410a1bdc",
    "1b3f417858cd2dd33ec7f62",
    "09fda4fee04195fd034783a",
    "077cccc11b8873ed5c3d48a",
    "29b62afe3ca036f4fe1a9da",
    "6054faf5f35d96d3b0c8c3e",
    "e20798e4310eed27884ae90",
    "775c9c08e80e26ddae56318",
    "b0b811028c2bf997213487c",
    "18a0c9231fc60adf5c5ea32",
    "76471e8302a0721e01b12b8",
    "ffbccb80ca8341fafb47b2e",
    "66a72a158f9325a2bf67170",
    "c4243689fe85b1c51363a18",
    "0dff739414d1a1b34b1c270",
    "15b48830636c8b99894972e",
    "29a89c0d3de81d665489b0e",
    "4f126f37fa51cbe61bd6b94",
    "99c47239d0d97d3c84e0940",
    "1919b75119765621bb4f1e8",
    "09db12d731faee0b86df6b8",
    "488fc33df43fbdeea4eafb4",
    "827423ee40b675f756eb5fe",
    "abe197c484cb74757144a9a",
    "2b500e4bc0ec5a6d2bdbdd0",
    "c474aa53d70218761669360",
    "8eba1a13db3390bd6718cec",
    "753844673a27782cc42012e",
    "06ff83a145c37035a5c1268",
    "3b37417858cc2dd33ec3f62",
    "9a4a5a28ee17ca9c324842c",
    "bc29f465309c977e89610a4",
    "2663ae6ddf8b5ce2bb29488",
    "46f231efe457034c1814418",
    "3fb2ce85abe9b0c72e06fbe",
    "de87481f282c153971a0a2e",
    "fcd7ccf23c69fa99bba1412",
    "f0261447e9490ca8e474cec",
    "4410115818196f95cdd7012",
    "088fc31df4bfbde2a4eafb4",
    "b8fef1b6307729fb0a078c0",
    "5afea7acccb77bbc9d99a90",
    "49a7016ac653f65ecdc9076",
    "1944d085be4e7da8d6cc7d0",
    "251f62adc4032f0ee714002",
    "56471f8702a0721e00b12b8",
    "2b8e4923f2dd51e2d537fa0",
    "6b550a40a66f4755de95c26",
    "a18ad28d4e27fe92a4f6c84",
    "10c2e586388cb82a3d80758",
    "ef34a41817ee02133db2eb0",
    "7e9c0c54325a9c15836e000",
    "3693e572d1fde4cdf079e86",
    "bfb2cec5abe1b0c72e07fbe",
    "7ee18230c583cccc57d4b08",
    "a066cb2fedafc9f52664126",
    "bb23725abc47cc5f4cc4cd2",
    "ded9dba3bee40c59b5609b4",
    "d9a7016ac653e6decdc9036",
    "9ad46aed5f707f280ab5fc4",
    "e5921c77822587316d7d3c2",
    "4f14da8242a8b86dca73352",
    "8b8b507ad467d4441df770e",
    "22831c9cf1169467ad04b68",
    "213b838fe2ae54c38ee7180",
    "5d926b6dd71f085181a4e12",
    "66ab79d4b29ee6e69509e56",
    "958148682d748a38dd68baa",
    "b8ce020cf069c32a723ab14",
    "f4331d6d461607e95752746",
    "6da23ba424b9596133cf9c8",
    "a636bcbc7b30c5fbeae67fe",
    "5cb0d86a07df654a9089a20",
    "f11f106848780fc9ecdd80a",
    "1fbb5364fb8d2c9d730d5ba",
    "fcb86bc70a50c9d02a5d034",
    "a534433029eac15f322e34c",
    "c989d9c7c3d3b8c55d75130",
    "7bb38b2f0186d46643ae962",
    "2644ebadeb44b9467d1f42c",
    "608cc857594bfbb55d69600",
];

const fn parse_hex_row(s: &str) -> u128 {
    let bytes = s.as_bytes();
    let mut val: u128 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let nibble = match bytes[i] {
            b'0'..=b'9' => (bytes[i] - b'0') as u128,
            b'a'..=b'f' => (bytes[i] - b'a' + 10) as u128,
            b'A'..=b'F' => (bytes[i] - b'A' + 10) as u128,
            _ => panic!("invalid hex digit in LDPC generator matrix"),
        };
        val = (val << 4) | nibble;
        i += 1;
    }
    // 23 hex digits = 92 bits; bit 0 is unused padding so shift right by 1
    // leaving column 0 at bit 90 and column 90 at bit 0.
    val >> 1
}

const fn build_gen_rows() -> [u128; 83] {
    let mut out = [0u128; 83];
    let mut r = 0;
    while r < 83 {
        out[r] = parse_hex_row(HEX_GEN_ROWS[r]);
        r += 1;
    }
    out
}

/// 83 systematic generator rows packed in bits `90..=0` (`col 0` at bit `90`, `col 90` at bit `0`).
pub const LDPC_GEN_ROWS: [u128; 83] = build_gen_rows();

/// Returns generator matrix entry `G[row][col]` (`0` or `1`) for `row < 83`, `col < 91`.
#[inline]
pub const fn generator_bit(row: usize, col: usize) -> u8 {
    ((LDPC_GEN_ROWS[row] >> (90 - col)) & 1) as u8
}

/// Appends the 14-bit CRC and systematically encodes a 77-bit payload into a 174-bit codeword.
pub fn encode174_91(msg77: &[u8; 77]) -> [u8; 174] {
    let crc = compute_crc14(msg77);
    let mut msg91 = [0u8; 91];
    msg91[..77].copy_from_slice(msg77);
    for i in 0..14 {
        msg91[77 + i] = ((crc >> (13 - i)) & 1) as u8;
    }
    encode174_91_nocrc(&msg91)
}

/// Systematically encodes a 91-bit `message || crc14` vector into a 174-bit LDPC codeword.
pub fn encode174_91_nocrc(msg91: &[u8; 91]) -> [u8; 174] {
    let mut cw = [0u8; 174];
    let mut msg_mask: u128 = 0;
    for (j, &bit) in msg91.iter().enumerate() {
        let b = bit & 1;
        cw[j] = b;
        if b != 0 {
            msg_mask |= 1u128 << (90 - j);
        }
    }
    for (i, &row_mask) in LDPC_GEN_ROWS.iter().enumerate() {
        cw[91 + i] = ((msg_mask & row_mask).count_ones() & 1) as u8;
    }
    cw
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1-based column indices per parity-check row (`Nm` from `ldpc_174_91_c_parity.f90`).
    #[rustfmt::skip]
    const PARITY_NM: [[u8; 7]; 83] = [
        [  4,  31,  59,  91,  92,  96, 153],
        [  5,  32,  60,  93, 115, 146,   0],
        [  6,  24,  61,  94, 122, 151,   0],
        [  7,  33,  62,  95,  96, 143,   0],
        [  8,  25,  63,  83,  93,  96, 148],
        [  6,  32,  64,  97, 126, 138,   0],
        [  5,  34,  65,  78,  98, 107, 154],
        [  9,  35,  66,  99, 139, 146,   0],
        [ 10,  36,  67, 100, 107, 126,   0],
        [ 11,  37,  67,  87, 101, 139, 158],
        [ 12,  38,  68, 102, 105, 155,   0],
        [ 13,  39,  69, 103, 149, 162,   0],
        [  8,  40,  70,  82, 104, 114, 145],
        [ 14,  41,  71,  88, 102, 123, 156],
        [ 15,  42,  59, 106, 123, 159,   0],
        [  1,  33,  72, 106, 107, 157,   0],
        [ 16,  43,  73, 108, 141, 160,   0],
        [ 17,  37,  74,  81, 109, 131, 154],
        [ 11,  44,  75, 110, 121, 166,   0],
        [ 45,  55,  64, 111, 130, 161, 173],
        [  8,  46,  71, 112, 119, 166,   0],
        [ 18,  36,  76,  89, 113, 114, 143],
        [ 19,  38,  77, 104, 116, 163,   0],
        [ 20,  47,  70,  92, 138, 165,   0],
        [  2,  48,  74, 113, 128, 160,   0],
        [ 21,  45,  78,  83, 117, 121, 151],
        [ 22,  47,  58, 118, 127, 164,   0],
        [ 16,  39,  62, 112, 134, 158,   0],
        [ 23,  43,  79, 120, 131, 145,   0],
        [ 19,  35,  59,  73, 110, 125, 161],
        [ 20,  36,  63,  94, 136, 161,   0],
        [ 14,  31,  79,  98, 132, 164,   0],
        [  3,  44,  80, 124, 127, 169,   0],
        [ 19,  46,  81, 117, 135, 167,   0],
        [  7,  49,  58,  90, 100, 105, 168],
        [ 12,  50,  61, 118, 119, 144,   0],
        [ 13,  51,  64, 114, 118, 157,   0],
        [ 24,  52,  76, 129, 148, 149,   0],
        [ 25,  53,  69,  90, 101, 130, 156],
        [ 20,  46,  65,  80, 120, 140, 170],
        [ 21,  54,  77, 100, 140, 171,   0],
        [ 35,  82, 133, 142, 171, 174,   0],
        [ 14,  30,  83, 113, 125, 170,   0],
        [  4,  29,  68, 120, 134, 173,   0],
        [  1,   4,  52,  57,  86, 136, 152],
        [ 26,  51,  56,  91, 122, 137, 168],
        [ 52,  84, 110, 115, 145, 168,   0],
        [  7,  50,  81,  99, 132, 173,   0],
        [ 23,  55,  67,  95, 172, 174,   0],
        [ 26,  41,  77, 109, 141, 148,   0],
        [  2,  27,  41,  61,  62, 115, 133],
        [ 27,  40,  56, 124, 125, 126,   0],
        [ 18,  49,  55, 124, 141, 167,   0],
        [  6,  33,  85, 108, 116, 156,   0],
        [ 28,  48,  70,  85, 105, 129, 158],
        [  9,  54,  63, 131, 147, 155,   0],
        [ 22,  53,  68, 109, 121, 174,   0],
        [  3,  13,  48,  78,  95, 123,   0],
        [ 31,  69, 133, 150, 155, 169,   0],
        [ 12,  43,  66,  89,  97, 135, 159],
        [  5,  39,  75, 102, 136, 167,   0],
        [  2,  54,  86, 101, 135, 164,   0],
        [ 15,  56,  87, 108, 119, 171,   0],
        [ 10,  44,  82,  91, 111, 144, 149],
        [ 23,  34,  71,  94, 127, 153,   0],
        [ 11,  49,  88,  92, 142, 157,   0],
        [ 29,  34,  87,  97, 147, 162,   0],
        [ 30,  50,  60,  86, 137, 142, 162],
        [ 10,  53,  66,  84, 112, 128, 165],
        [ 22,  57,  85,  93, 140, 159,   0],
        [ 28,  32,  72, 103, 132, 166,   0],
        [ 28,  29,  84,  88, 117, 143, 150],
        [  1,  26,  45,  80, 128, 147,   0],
        [ 17,  27,  89, 103, 116, 153,   0],
        [ 51,  57,  98, 163, 165, 172,   0],
        [ 21,  37,  73, 138, 152, 169,   0],
        [ 16,  47,  76, 130, 137, 154,   0],
        [  3,  24,  30,  72, 104, 139,   0],
        [  9,  40,  90, 106, 134, 151,   0],
        [ 15,  58,  60,  74, 111, 150, 163],
        [ 18,  42,  79, 144, 146, 152,   0],
        [ 25,  38,  65,  99, 122, 160,   0],
        [ 17,  42,  75, 129, 170, 172,   0],
    ];

    fn satisfies_parity(cw: &[u8; 174]) -> bool {
        PARITY_NM.iter().all(|row| {
            let sum: u8 = row
                .iter()
                .filter(|&&col| col > 0)
                .map(|&col| cw[(col - 1) as usize])
                .sum();
            (sum & 1) == 0
        })
    }

    #[test]
    fn generator_rows_satisfy_parity_equations() {
        assert_eq!(generator_bit(0, 0), 1);
        assert_eq!(generator_bit(0, 90), 0);

        let mut state: u64 = 0xDEAD_BEEF_CAFE_BABEu64;
        for _ in 0..500 {
            let mut msg77 = [0u8; 77];
            for b in msg77.iter_mut() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                *b = ((state >> 63) & 1) as u8;
            }
            let cw = encode174_91(&msg77);
            assert!(satisfies_parity(&cw), "codeword failed parity check");
        }
    }
}
