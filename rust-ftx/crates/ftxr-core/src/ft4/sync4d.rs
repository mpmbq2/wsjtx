// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 4×4×4×4 Costas fine time/frequency synchronization (`sync4d.f90`).

use std::f32::consts::TAU;

use num_complex::Complex32;

use crate::dsp::twkfreq::twkfreq1;

use super::params::{FS_DOWNSAMPLE, ICOS4_A, ICOS4_B, ICOS4_C, ICOS4_D, NDMAX, NSS, SYNC_POSITIONS};

const HALF_SYM_SAMPLES: usize = 2 * NSS; // 64 decimated samples across 4 Costas symbols
const IDF_MIN: i32 = -16;
const IDF_MAX: i32 = 16;
const IDF_COUNT: usize = (IDF_MAX - IDF_MIN + 1) as usize; // 33

/// Precomputed conjugate Costas reference waveforms already frequency-tweaked for all
/// `idf in -16..=+16` (`ctwk2(idf) * csync[block]`, conjugated for inner product).
#[derive(Debug, Clone)]
pub struct Ft4SyncTables {
    /// `[block][idf_idx][sample_idx]` where `block in 0..4`, `idf_idx = (idf + 16) as usize`.
    tweaked_conj: [[[Complex32; HALF_SYM_SAMPLES]; IDF_COUNT]; 4],
}

impl Default for Ft4SyncTables {
    fn default() -> Self {
        Self::new()
    }
}

impl Ft4SyncTables {
    pub fn new() -> Self {
        let icos = [ICOS4_A, ICOS4_B, ICOS4_C, ICOS4_D];
        let mut csync = [[Complex32::ZERO; HALF_SYM_SAMPLES]; 4];
        let samples_per_sym = NSS / 2; // 16 decimated samples/symbol

        for (b, pattern) in icos.iter().enumerate() {
            let mut phi = 0.0_f32;
            let mut k = 0usize;
            for &tone in pattern {
                let dphi = 2.0 * TAU * (tone as f32) / (NSS as f32);
                for _ in 0..samples_per_sym {
                    csync[b][k] = Complex32::new(phi.cos(), phi.sin());
                    phi = (phi + dphi) % TAU;
                    k += 1;
                }
            }
        }

        let ones = [Complex32::new(1.0, 0.0); HALF_SYM_SAMPLES];
        let mut ctwk = [Complex32::ZERO; HALF_SYM_SAMPLES];
        let fs_decimated = FS_DOWNSAMPLE * 0.5; // 333.3333 Hz

        let mut tweaked_conj = [[[Complex32::ZERO; HALF_SYM_SAMPLES]; IDF_COUNT]; 4];
        for idf in IDF_MIN..=IDF_MAX {
            let idf_idx = (idf - IDF_MIN) as usize;
            twkfreq1(
                &ones,
                fs_decimated,
                &[idf as f32, 0.0, 0.0, 0.0, 0.0],
                &mut ctwk,
            );
            for b in 0..4 {
                for j in 0..HALF_SYM_SAMPLES {
                    tweaked_conj[b][idf_idx][j] = (ctwk[j] * csync[b][j]).conj();
                }
            }
        }

        Self { tweaked_conj }
    }

    /// Computes normalized 4-block Costas synchronization score matching `wsjtx/lib/ft4/sync4d.f90`.
    pub fn sync4d(&self, cd0: &[Complex32; NDMAX], i0: i32, idf: i32) -> f32 {
        let idf_idx = (idf - IDF_MIN).clamp(0, (IDF_COUNT - 1) as i32) as usize;
        let refs = &self.tweaked_conj;
        let span = 2 * HALF_SYM_SAMPLES as i32 - 1; // 127
        let nmax_last = (NDMAX as i32) - 1; // 4031

        let i1 = i0 + (SYNC_POSITIONS[0] * NSS) as i32; // i0
        let i2 = i0 + (SYNC_POSITIONS[1] * NSS) as i32; // i0 + 1056
        let i3 = i0 + (SYNC_POSITIONS[2] * NSS) as i32; // i0 + 2112
        let i4 = i0 + (SYNC_POSITIONS[3] * NSS) as i32; // i0 + 3168

        // Block 1 (i1 = i0): handles negative start boundary i1 < 0.
        let z1 = if i1 >= 0 && i1 + span <= nmax_last {
            dot_strided(cd0, i1 as usize, &refs[0][idf_idx])
        } else if i1 < 0 {
            let end_idx = i1 + span;
            let npts = end_idx / 2;
            if npts > 16 && end_idx >= 0 {
                let ref_start = (HALF_SYM_SAMPLES as i32 - 1 - npts) as usize;
                let mut acc = Complex32::ZERO;
                let mut cd_idx = 0usize;
                for &r in &refs[0][idf_idx][ref_start..] {
                    acc += cd0[cd_idx] * r;
                    cd_idx += 2;
                }
                acc
            } else {
                Complex32::ZERO
            }
        } else {
            Complex32::ZERO
        };

        // Block 2 (i2 = i0 + 1056) and Block 3 (i3 = i0 + 2112).
        let z2 = if i2 >= 0 && i2 + span <= nmax_last {
            dot_strided(cd0, i2 as usize, &refs[1][idf_idx])
        } else {
            Complex32::ZERO
        };
        let z3 = if i3 >= 0 && i3 + span <= nmax_last {
            dot_strided(cd0, i3 as usize, &refs[2][idf_idx])
        } else {
            Complex32::ZERO
        };

        // Block 4 (i4 = i0 + 3168): handles end-of-buffer truncation i4 + 127 > 4031.
        let z4 = if i4 >= 0 && i4 + span <= nmax_last {
            dot_strided(cd0, i4 as usize, &refs[3][idf_idx])
        } else if i4 >= 0 && i4 < NDMAX as i32 {
            let npts = (NDMAX as i32 - i4) / 2;
            if npts > 16 {
                let mut acc = Complex32::ZERO;
                let mut cd_idx = i4 as usize;
                for &r in &refs[3][idf_idx][..(npts as usize)] {
                    acc += cd0[cd_idx] * r;
                    cd_idx += 2;
                }
                acc
            } else {
                Complex32::ZERO
            }
        } else {
            Complex32::ZERO
        };

        (z1.norm() + z2.norm() + z3.norm() + z4.norm()) / (HALF_SYM_SAMPLES as f32)
    }
}

#[inline]
fn dot_strided(cd0: &[Complex32; NDMAX], start: usize, ref_conj: &[Complex32; HALF_SYM_SAMPLES]) -> Complex32 {
    let mut acc = Complex32::ZERO;
    let slice = &cd0[start..start + HALF_SYM_SAMPLES * 2 - 1];
    for (k, &r) in ref_conj.iter().enumerate() {
        acc += slice[k * 2] * r;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::downsample::{Ft4DownsampleScratch, Ft4Downsampler, Ft4PassSpectrum};
    use crate::dsp::fft::Ft4FftPlans;
    use crate::ft4::params::NMAX;
    use crate::modulate::ft4::{encode_ft4_tones_from_77bits, gen_ft4_wave_real};
    use crate::protocol::hashtable::CallsignHashTable;
    use crate::protocol::pack77::pack77;
    use crate::protocol::types::UnpackContext;

    #[test]
    fn sync4d_peaks_at_expected_dt_and_handles_negative_and_trailing_boundaries() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();
        let tones = encode_ft4_tones_from_77bits(&packed.bits);
        let wave = gen_ft4_wave_real(&tones, 1500.0, 12_000);

        let plans = Ft4FftPlans::new();
        let down = Ft4Downsampler::new();
        let mut d_scratch = Ft4DownsampleScratch::new(&plans);
        let mut spectrum = Ft4PassSpectrum::new();
        let sync_tables = Ft4SyncTables::new();

        // 1. Nominal start at sample 6000 (0.5 s => DT=0 => ibest ≈ 6000/18 + 32 = 365;
        //    actually ramp is 1 symbol (576 samples = 32 downsampled) so sync block starts at +1 sym!).
        let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        dd[6000..6000 + wave.len()].copy_from_slice(&wave);
        down.compute_pass_spectrum(&dd, &plans, &mut d_scratch, &mut spectrum);
        let mut cd0 = [Complex32::ZERO; NDMAX];
        down.downsample_from_spectrum(&spectrum, 1500.0, &plans, &mut d_scratch, &mut cd0);
        let sum2: f32 = cd0.iter().map(|c| c.norm_sqr()).sum::<f32>() / (NDMAX as f32);
        let inv_rms = 1.0 / sum2.sqrt();
        for c in &mut cd0 {
            *c *= inv_rms;
        }

        let expected_i0 = (6000 + 576) / 18; // 365
        let mut best_i0 = 0;
        let mut best_s = -1.0_f32;
        for i0 in 350..=380 {
            let s = sync_tables.sync4d(&cd0, i0, 0);
            if s > best_s {
                best_s = s;
                best_i0 = i0;
            }
        }
        assert!(
            (best_i0 - expected_i0).abs() <= 1 && best_s > 2.0,
            "expected peak near {expected_i0}, got {best_i0} with sync={best_s}"
        );

        // 2. Negative start offset boundary check (i0 in -93..-1 and below -128 -> zero, no panic)
        let mut dd_early: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
        // Shift wave earlier by 1440 samples (so data symbol 0 starts at 576 - 1440 = -864 => i0 = -48).
        dd_early[..(wave.len() - 1440)].copy_from_slice(&wave[1440..]);
        down.compute_pass_spectrum(&dd_early, &plans, &mut d_scratch, &mut spectrum);
        down.downsample_from_spectrum(&spectrum, 1500.0, &plans, &mut d_scratch, &mut cd0);
        let s_neg = sync_tables.sync4d(&cd0, -48, 0);
        assert!(s_neg.is_finite() && s_neg > 0.0);
        // Zero-signal extreme negative and trailing offsets return exact 0.0 without out-of-bounds.
        let zeros = [Complex32::ZERO; NDMAX];
        assert_eq!(sync_tables.sync4d(&zeros, -200, 0), 0.0);
        assert_eq!(sync_tables.sync4d(&zeros, -344, 0), 0.0);

        // 3. Trailing i4 + 127 > 4031 boundary check
        let s_late = sync_tables.sync4d(&cd0, 800, 0);
        assert!(s_late.is_finite());
        let s_extreme = sync_tables.sync4d(&cd0, 1017, 16);
        assert!(s_extreme.is_finite());
    }
}
