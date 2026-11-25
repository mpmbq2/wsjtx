// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT4 tone mapping and continuous-phase 4-GFSK (`BT = 1.0`) waveform synthesis
//! (`wsjtx/lib/ft4/genft4.f90` and `gen_ft4wave.f90`).

use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::ft4::params::{GRAYMAP, ICOS4_A, ICOS4_B, ICOS4_C, ICOS4_D, ND, NN, NN2};
use crate::protocol::ldpc::encode174_91;
use crate::protocol::rvec::scramble_ft4;

use super::gfsk::{
    apply_raised_cosine_ramps_complex, apply_raised_cosine_ramps_real, build_gfsk_pulse_table,
};

/// FT4 symbol duration in seconds (`0.048 s`).
pub const FT4_SYMBOL_SEC: f64 = 0.048;
/// FT4 Gaussian pulse `BT` product (`1.0`).
pub const FT4_BT: f64 = 1.0;

/// Computes samples per FT4 symbol for an arbitrary sample rate (`round(fs * 0.048)`).
#[inline]
pub fn ft4_nsps(sample_rate: u32) -> usize {
    ((sample_rate as f64) * FT4_SYMBOL_SEC).round() as usize
}

/// Encodes an unscrambled 77-bit payload (`rvec` scramble -> CRC-14 -> LDPC(174,91) ->
/// 2-bit Gray map -> Costas4 A/B/C/D sync) into 103 FT4 channel tone symbols (`0..=3`).
///
/// The 2 outer ramp symbols are synthesized during waveform generation (`105 * nsps`).
pub fn encode_ft4_tones_from_77bits(msg77_unscrambled: &[u8; 77]) -> [u8; NN] {
    let scrambled = scramble_ft4(msg77_unscrambled);
    let codeword = encode174_91(&scrambled);

    let mut itmp = [0u8; ND];
    for (i, dst) in itmp.iter_mut().enumerate() {
        let indx = ((codeword[2 * i] as usize) << 1) | (codeword[2 * i + 1] as usize);
        *dst = GRAYMAP[indx];
    }

    let mut tones = [0u8; NN];
    tones[0..4].copy_from_slice(&ICOS4_A);
    tones[4..33].copy_from_slice(&itmp[0..29]);
    tones[33..37].copy_from_slice(&ICOS4_B);
    tones[37..66].copy_from_slice(&itmp[29..58]);
    tones[66..70].copy_from_slice(&ICOS4_C);
    tones[70..99].copy_from_slice(&itmp[58..87]);
    tones[99..103].copy_from_slice(&ICOS4_D);
    tones
}

fn gen_ft4_dphi(tones: &[u8; NN], af_hz: f64, sample_rate: u32) -> (usize, Vec<f64>) {
    let nsps = ft4_nsps(sample_rate);
    let pulse = build_gfsk_pulse_table(FT4_BT, nsps);
    // Zero-padded outside the 103 data+sync symbols (`gen_ft4wave.f90:24-30`).
    let mut dphi = vec![0.0_f64; NN2 * nsps];
    let dphi_peak = TAU / (nsps as f64);

    for (j, &tone) in tones.iter().enumerate() {
        let ib = j * nsps;
        let scale = dphi_peak * (tone as f64);
        for (dst, &p) in dphi[ib..ib + 3 * nsps].iter_mut().zip(pulse.iter()) {
            *dst += scale * p;
        }
    }

    let dphi0 = TAU * af_hz / (sample_rate as f64);
    for x in &mut dphi {
        *x += dphi0;
    }

    (nsps, dphi)
}

/// Synthesizes a real `f32` active FT4 GFSK waveform (`105 * nsps` samples, `sin(phi)`).
pub fn gen_ft4_wave_real(tones: &[u8; NN], af_hz: f64, sample_rate: u32) -> Vec<f32> {
    let (nsps, dphi) = gen_ft4_dphi(tones, af_hz, sample_rate);
    let nwave = NN2 * nsps;
    let mut wave = Vec::with_capacity(nwave);
    let mut phi = 0.0_f64;

    for &dp in &dphi {
        wave.push(phi.sin() as f32);
        phi = (phi + dp).rem_euclid(TAU);
    }

    apply_raised_cosine_ramps_real(&mut wave, nsps);
    wave
}

/// Synthesizes a complex `Complex32` analytic FT4 GFSK waveform (`105 * nsps` samples,
/// `cos(phi) + i*sin(phi)`) used both for oracle comparison and Stage 4 subtraction.
pub fn gen_ft4_wave_complex(tones: &[u8; NN], af_hz: f64, sample_rate: u32) -> Vec<Complex32> {
    let (nsps, dphi) = gen_ft4_dphi(tones, af_hz, sample_rate);
    let nwave = NN2 * nsps;
    let mut cwave = Vec::with_capacity(nwave);
    let mut phi = 0.0_f64;

    for &dp in &dphi {
        cwave.push(Complex32::new(phi.cos() as f32, phi.sin() as f32));
        phi = (phi + dp).rem_euclid(TAU);
    }

    apply_raised_cosine_ramps_complex(&mut cwave, nsps);
    cwave
}
