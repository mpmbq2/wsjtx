// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Shared DSP building blocks (FFT, windows, baseline estimation, frequency shift, downsampling).

pub mod baseline;
pub mod downsample;
pub mod fft;
pub mod pool;
pub mod twkfreq;
pub mod window;

pub use pool::{
    CandidateScratch, CandidateScratchPool, FftPlanCache, PassScratch, PassScratchPool,
    PooledCandidateScratch, PooledPassScratch, PrecomputedTables,
};
