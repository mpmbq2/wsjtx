// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 77-bit payload scrambling vector (`rvec`), verified against
//! `wsjtx/lib/ft4/genft4.f90:33-35` and `wsjtx/lib/ft4/ft4code.f90:17-19`.

/// 77-bit XOR scrambling vector applied before CRC-14 / LDPC(174,91) encoding in FT4.
#[rustfmt::skip]
pub const FT4_RVEC: [u8; 77] = [
    0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0,
    1, 0, 0, 1, 0, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 1,
    0, 1, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1, 0, 0, 0, 1, 0, 1,
];

/// XORs a 77-bit FT4 payload with `FT4_RVEC` (self-inverse: used for both scrambling and descrambling).
pub fn xor_rvec(bits: &[u8; 77]) -> [u8; 77] {
    let mut out = [0u8; 77];
    for (dst, (&b, &r)) in out.iter_mut().zip(bits.iter().zip(FT4_RVEC.iter())) {
        *dst = (b ^ r) & 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rvec_xor_is_self_inverse() {
        let mut msg = [0u8; 77];
        for (i, bit) in msg.iter_mut().enumerate() {
            *bit = (i % 2) as u8;
        }
        let scrambled = xor_rvec(&msg);
        assert_ne!(scrambled, msg);
        assert_eq!(xor_rvec(&scrambled), msg);
    }
}
