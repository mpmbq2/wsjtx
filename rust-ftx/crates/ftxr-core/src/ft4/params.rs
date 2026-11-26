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

/// Downsampled samples per FT4 symbol (`NSPS / NDOWN = 576 / 18 = 32`).
pub const NSS: usize = NSPS / NDOWN;

/// Total downsampled complex samples over `NMAX` (`72_576 / 18 = 4_032`).
pub const NDMAX: usize = NMAX / NDOWN;

/// Downsampled complex samples per FT4 frame (`NN * NSS = 103 * 32 = 3_296`).
pub const NFRAME_DOWN: usize = NN * NSS;

/// Total channel bits per FT4 frame (`2 * NN = 206`).
pub const NBITS: usize = 2 * NN;

/// LDPC codeword length (`174`).
pub const NCW: usize = N;

/// Maximum number of frequency candidates returned by `getcandidates4`.
pub const MAXCAND: usize = 200;

/// Minimum spectral sync ratio threshold in `getcandidates4`.
pub const SYNCMIN: f32 = 1.18;

/// Minimum `sync4d` correlation threshold in `ft4_decode`.
pub const SYNC4D_MIN: f32 = 1.2;

/// Empirical LLR scaling factor applied to raw bit metrics before LDPC/OSD (`scalefac = 2.83`).
pub const SCALEFAC: f32 = 2.83;

/// Half-width in Hz around `rx_freq_hz` for AP `a3`/`a6` passes and `maxosd = 3` (`napwid = 50`).
pub const NAPWID_HZ: f32 = 50.0;

/// Half-width in Hz around `rx_freq_hz` prioritized first in candidate list (`getcandidates4`).
pub const NFQSO_PRIOR_HZ: f32 = 20.0;

/// Minimum and maximum candidate audio frequency bounds in `getcandidates4` (Hz).
pub const MIN_FREQ_HZ: f32 = 200.0;
pub const MAX_FREQ_HZ: f32 = 4910.0;

/// FT4 frequency offset applied in `getcandidates4` (`-1.5 * 12000.0 / 576.0 = -31.25 Hz`).
pub const F_OFFSET_HZ: f32 = -1.5 * (SAMPLE_RATE_HZ as f32) / (NSPS as f32);

/// FT4 tone spacing / baud rate in Hz (`12000 / 576 = 20.833334 Hz`).
pub const F_BAUD: f32 = (SAMPLE_RATE_HZ as f32) / (NSPS as f32);

/// Coarse spectrogram frequency bin width in Hz (`12000 / 2304 = 5.2083335 Hz`).
pub const DF_SPECTRUM: f32 = (SAMPLE_RATE_HZ as f32) / (NFFT1 as f32);

/// Full-length FFT frequency bin width in Hz (`12000 / 72576 = 0.16534391 Hz`).
pub const DF_DOWNSAMPLE: f32 = (SAMPLE_RATE_HZ as f32) / (NMAX as f32);

/// Downsampled complex sample rate in Hz (`12000 / 18 = 666.6667 Hz`).
pub const FS_DOWNSAMPLE: f32 = (SAMPLE_RATE_HZ as f32) / (NDOWN as f32);

/// Literal divisor used in WSJT-X `ft4_decode.f90` for reported `dt` (`ibest / 666.67 - 0.5`).
pub const DT_RATE_LITERAL: f32 = 666.67;

/// Nominal DT offset in seconds subtracted in `ft4_decode.f90`.
pub const DT_OFFSET_S: f32 = 0.5;

/// Empirical SNR calibration offset in dB (`10*log10(snr) - 14.8`).
pub const SNR_OFFSET_DB: f32 = 14.8;

/// Lower floor for reported FT4 SNR in dB (`-21`).
pub const SNR_FLOOR_DB: f32 = -21.0;

/// Minimum hard-decision Costas symbol matches required in `get_ft4_bitmetrics` (`8` of `16`).
pub const NSYNC_MIN: usize = 8;

/// Minimum hard-decision Costas bit matches required in `ft4_decode` (`20` of `32`).
pub const NSYNC_QUAL_MIN: usize = 20;

/// Low-pass moving-average window length in `subtractft4` (`NFILT = 1400`).
pub const SUBTRACT_NFILT: usize = 1400;

/// Cyclic shift applied to `cw` filter kernel in `subtractft4` (`NFILT / 2 + 1 = 701`).
pub const SUBTRACT_SHIFT: usize = SUBTRACT_NFILT / 2 + 1;

/// Hard-decision Gray-mapped bits corresponding to `ICOS4_A` (`[0, 1, 3, 2]`).
pub const SYNC_BITS_A: [u8; 8] = [0, 0, 0, 1, 1, 0, 1, 1];
/// Hard-decision Gray-mapped bits corresponding to `ICOS4_B` (`[1, 0, 2, 3]`).
pub const SYNC_BITS_B: [u8; 8] = [0, 1, 0, 0, 1, 1, 1, 0];
/// Hard-decision Gray-mapped bits corresponding to `ICOS4_C` (`[2, 3, 1, 0]`).
pub const SYNC_BITS_C: [u8; 8] = [1, 1, 1, 0, 0, 1, 0, 0];
/// Hard-decision Gray-mapped bits corresponding to `ICOS4_D` (`[3, 2, 0, 1]`).
pub const SYNC_BITS_D: [u8; 8] = [1, 0, 1, 1, 0, 0, 0, 1];

/// Symbol offsets of the four 4-symbol Costas arrays in an FT4 frame (`NN = 103`).
pub const SYNC_POSITIONS: [usize; 4] = [0, 33, 66, 99];

/// Three `ib` search windows evaluated per candidate in `ft4_decode.f90`.
pub const DT_SEGMENTS: [(i32, i32); 3] = [(108, 560), (560, 1012), (-344, 108)];

