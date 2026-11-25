// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 protocol constants verified against `wsjtx/lib/ft8/ft8_params.f90` and `genft8.f90`.

/// Baseband RX sample rate in Hz.
pub const SAMPLE_RATE_HZ: u32 = 12_000;
/// Information + CRC-14 bits (`77 + 14`).
pub const KK: usize = 91;
/// LDPC(174,91) codeword length in bits.
pub const N: usize = 174;
/// Number of data symbols (3 bits/symbol, 8-GFSK).
pub const ND: usize = 58;
/// Number of Costas sync symbols (3 × 7).
pub const NS: usize = 21;
/// Total channel symbols (`NS + ND = 79`).
pub const NN: usize = NS + ND;
/// Samples per symbol at 12,000 Hz (`0.160 s`, `6.25 Hz` tone spacing).
pub const NSPS: usize = 1_920;
/// Active waveform samples at 12,000 Hz (`NSPS * NN = 151,680`, `12.64 s`).
pub const NZ: usize = NSPS * NN;
/// Total 15 s slot samples at 12,000 Hz (`180,000`).
pub const NMAX: usize = 15 * 12_000;
/// FFT length for coarse symbol spectra (`2 * NSPS = 3,840`).
pub const NFFT1: usize = 2 * NSPS;
/// Positive frequency bins in `NFFT1` (`NFFT1 / 2 = 1,920`).
pub const NH1: usize = NFFT1 / 2;
/// Coarse time-sync step size in samples (`NSPS / 4 = 480`).
pub const NSTEP: usize = NSPS / 4;
/// Number of 1/4-symbol spectra in a slot (`NMAX / NSTEP - 3 = 372`).
pub const NHSYM: usize = NMAX / NSTEP - 3;
/// Baseband downsample factor (`1920 / 32 = 60`, giving `200 Hz` sample rate).
pub const NDOWN: usize = 60;
/// Long real-to-complex FFT length in `ft8_downsample` (`df = 0.0625 Hz`).
pub const DOWN_NFFT1: usize = 192_000;
/// Baseband complex IFFT length in `ft8_downsample` (`192000 / 60 = 3200`).
pub const DOWN_NFFT2: usize = DOWN_NFFT1 / NDOWN;
/// Valid non-tail baseband sample count in `sync8d` and `ft8b` (`14.06 s` at `200 Hz`).
pub const NP2: usize = 2_812;
/// Subtraction FFT length (`NMAX = 180,000`).
pub const SUB_NFFT: usize = NMAX;
/// Subtraction `cos^2` low-pass filter span (`-2000..=+2000`, `4001` taps).
pub const SUB_NFILT: usize = 4_000;
/// GFSK Gaussian filter bandwidth-symbol-time product (`BT`).
pub const BT: f32 = 2.0;
/// Nominal TX start delay from UTC slot boundary (ms), matching `Modulator.cpp`.
pub const NOMINAL_TX_DELAY_MS: u32 = 500;
/// 7-symbol Costas synchronization array (repeated at symbols `0..7`, `36..43`, `72..79`).
pub const ICOS7: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];
/// WSJT-X non-standard 3-bit Gray mapping from 3 codeword bits to 8-FSK tone index.
pub const GRAYMAP: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
