// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 protocol constants verified against `wsjtx/lib/ft4/ft4_params.f90` and `genft4.f90`.

/// Baseband RX sample rate in Hz.
pub const SAMPLE_RATE_HZ: u32 = 12_000;
/// Information + CRC-14 bits (`77 + 14`).
pub const KK: usize = 91;
/// LDPC(174,91) codeword length in bits.
pub const N: usize = 174;
/// Number of data symbols (2 bits/symbol, 4-GFSK).
pub const ND: usize = 87;
/// Number of Costas sync symbols (4 × 4).
pub const NS: usize = 16;
/// Sync and data symbols (`NS + ND = 103`).
pub const NN: usize = NS + ND;
/// Total channel symbols including leading and trailing ramp symbols (`NN + 2 = 105`).
pub const NN2: usize = NS + ND + 2;
/// Samples per symbol at 12,000 Hz (`0.048 s`, `20.8333... Hz` tone spacing).
pub const NSPS: usize = 576;
/// Sync + data samples at 12,000 Hz (`NSPS * NN = 59,328`, `4.944 s`).
pub const NZ: usize = NSPS * NN;
/// Total shaped waveform samples including ramps at 12,000 Hz (`NSPS * NN2 = 60,480`, `5.040 s`).
pub const NZ2: usize = NSPS * NN2;
/// RX window length in samples at 12,000 Hz (`21 * 3456 = 72,576`, `6.048 s`).
pub const NMAX: usize = 21 * 3_456;
/// FFT length for coarse symbol spectra (`2,304`).
pub const NFFT1: usize = 2_304;
/// Positive frequency bins in `NFFT1` (`NFFT1 / 2 = 1,152`).
pub const NH1: usize = NFFT1 / 2;
/// Coarse time-sync step size in samples (`NSPS = 576`).
pub const NSTEP: usize = NSPS;
/// Number of coarse symbol spectra (`(NMAX - NFFT1) / NSTEP = 122`).
pub const NHSYM: usize = (NMAX - NFFT1) / NSTEP;
/// Baseband downsample factor (`576 / 32 = 18`, giving `666.667 Hz` sample rate).
pub const NDOWN: usize = 18;
/// GFSK Gaussian filter bandwidth-symbol-time product (`BT`).
pub const BT: f32 = 1.0;
/// Nominal TX start delay from UTC slot boundary (ms), matching `Modulator.cpp`.
pub const NOMINAL_TX_DELAY_MS: u32 = 300;
/// FT4 4-symbol Costas sync array A.
pub const ICOS4_A: [u8; 4] = [0, 1, 3, 2];
/// FT4 4-symbol Costas sync array B.
pub const ICOS4_B: [u8; 4] = [1, 0, 2, 3];
/// FT4 4-symbol Costas sync array C.
pub const ICOS4_C: [u8; 4] = [2, 3, 1, 0];
/// FT4 4-symbol Costas sync array D.
pub const ICOS4_D: [u8; 4] = [3, 2, 0, 1];
/// 2-bit Gray mapping from 2 codeword bits to 4-FSK tone index.
pub const GRAYMAP: [u8; 4] = [0, 1, 3, 2];
