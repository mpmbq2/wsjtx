// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 fine Costas time/frequency synchronization (`sync8d.f90`).

use std::f32::consts::TAU;

use num_complex::Complex32;

use super::params::{DOWN_NFFT2, ICOS7, NP2};

/// Precomputed 7×32 Costas reference tone waveforms for `sync8d`.
#[derive(Debug, Clone)]
pub struct Sync8dTables {
    csync: [[Complex32; 32]; 7],
}

impl Default for Sync8dTables {
    fn default() -> Self {
        Self::new()
    }
}

impl Sync8dTables {
    pub fn new() -> Self {
        let mut csync = [[Complex32::ZERO; 32]; 7];
        for (i, row) in csync.iter_mut().enumerate() {
            let mut phi = 0.0_f32;
            let dphi = TAU * (ICOS7[i] as f32) / 32.0;
            for cell in row.iter_mut() {
                *cell = Complex32::new(phi.cos(), phi.sin());
                phi = (phi + dphi) % TAU;
            }
        }
        Self { csync }
    }

    /// Computes fine Costas sync power at baseband sample offset `i0` (`200 Hz` sample rate)
    /// with optional 32-sample complex frequency-tweak vector `ctwk`.
    pub fn sync8d(
        &self,
        cd0: &[Complex32; DOWN_NFFT2],
        i0: isize,
        ctwk: Option<&[Complex32; 32]>,
    ) -> f32 {
        let np2_max = (NP2 as isize) - 1;
        let mut sync = 0.0_f32;

        for (i, csync_row) in self.csync.iter().enumerate() {
            let i1 = i0 + (i as isize) * 32;
            let i2 = i1 + 36 * 32;
            let i3 = i1 + 72 * 32;

            let mut target = *csync_row;
            if let Some(twk) = ctwk {
                for (dst, &t) in target.iter_mut().zip(twk.iter()) {
                    *dst *= t;
                }
            }

            for idx in [i1, i2, i3] {
                if idx >= 0 && idx + 31 <= np2_max {
                    let start = idx as usize;
                    let mut z = Complex32::ZERO;
                    for (sample, &ref_c) in cd0[start..start + 32].iter().zip(target.iter()) {
                        z += *sample * ref_c.conj();
                    }
                    sync += z.norm_sqr();
                }
            }
        }

        sync
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
    use crate::dsp::fft::Ft8FftPlans;
    use crate::ft8::params::NMAX;
    use crate::modulate::ft8::{encode_ft8_tones_from_77bits, gen_ft8_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn sync8d_peaks_sharply_at_expected_dt_offset() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ N0AAA EM10", None, &ctx, &ht).unwrap();
        let tones = encode_ft8_tones_from_77bits(&packed.bits);
        let wave = gen_ft8_wave_real(&tones, 1500.0, 12_000);

        // Place active waveform starting at 6000 samples (0.5 s => DT = 0.0 => ibest = 100).
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        dd[6000..6000 + wave.len()].copy_from_slice(&wave);

        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let mut snapshot = Ft8SpectrumSnapshot::new();
        downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);

        let mut ifft_scratch =
            vec![Complex32::ZERO; plans.c2c_inv_3200.get_inplace_scratch_len()];
        let mut cd0 = [Complex32::ZERO; DOWN_NFFT2];
        downsampler.downsample_from_snapshot(&snapshot, 1500.0, &plans, &mut ifft_scratch, &mut cd0);

        let tables = Sync8dTables::new();
        let mut best_i0 = 0isize;
        let mut best_sync = -1.0_f32;
        for i0 in 80..=120 {
            let s = tables.sync8d(&cd0, i0, None);
            if s > best_sync {
                best_sync = s;
                best_i0 = i0;
            }
        }
        // `xdt_raw = (ibest - 1) * 0.005`, so 0.5 s corresponds to ibest = 100 or 101.
        assert!((99..=102).contains(&best_i0), "unexpected peak i0={best_i0}");
    }
}
