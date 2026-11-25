// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 tone mapping and continuous-phase 8-GFSK (`BT = 2.0`) waveform synthesis
//! (`wsjtx/lib/ft8/genft8.f90` and `gen_ft8wave.f90`).

use std::f64::consts::TAU;

use num_complex::Complex32;

use crate::ft8::params::{GRAYMAP, ICOS7, ND, NN};
use crate::protocol::ldpc::encode174_91;

use super::gfsk::{
    apply_raised_cosine_ramps_complex, apply_raised_cosine_ramps_real, build_gfsk_pulse_table,
};

/// FT8 symbol duration in seconds (`0.160 s`).
pub const FT8_SYMBOL_SEC: f64 = 0.160;
/// FT8 Gaussian pulse `BT` product (`2.0`).
pub const FT8_BT: f64 = 2.0;

/// Computes samples per FT8 symbol for an arbitrary sample rate (`round(fs * 0.160)`).
#[inline]
pub fn ft8_nsps(sample_rate: u32) -> usize {
    ((sample_rate as f64) * FT8_SYMBOL_SEC).round() as usize
}

/// Encodes a 77-bit payload (CRC-14 + LDPC(174,91) + 3-bit Gray map + Costas7 sync)
/// into 79 FT8 channel tone symbols (`0..=7`).
pub fn encode_ft8_tones_from_77bits(msg77: &[u8; 77]) -> [u8; NN] {
    let codeword = encode174_91(msg77);
    let mut tones = [0u8; NN];

    tones[0..7].copy_from_slice(&ICOS7);
    tones[36..43].copy_from_slice(&ICOS7);
    tones[72..79].copy_from_slice(&ICOS7);

    for j in 0..ND {
        let i = 3 * j;
        let k = if j < 29 { 7 + j } else { 14 + j };
        let indx = ((codeword[i] as usize) << 2)
            | ((codeword[i + 1] as usize) << 1)
            | (codeword[i + 2] as usize);
        tones[k] = GRAYMAP[indx];
    }

    tones
}

fn gen_ft8_dphi(tones: &[u8; NN], af_hz: f64, sample_rate: u32) -> (usize, Vec<f64>) {
    let nsps = ft8_nsps(sample_rate);
    let pulse = build_gfsk_pulse_table(FT8_BT, nsps);
    let mut dphi = vec![0.0_f64; (NN + 2) * nsps];
    let dphi_peak = TAU / (nsps as f64);

    for (j, &tone) in tones.iter().enumerate() {
        let ib = j * nsps;
        let scale = dphi_peak * (tone as f64);
        for (dst, &p) in dphi[ib..ib + 3 * nsps].iter_mut().zip(pulse.iter()) {
            *dst += scale * p;
        }
    }

    // Boundary dummy symbols extending first and last tone across edges (`gen_ft8wave.f90:42-43`).
    let lead_scale = dphi_peak * (tones[0] as f64);
    for (dst, &p) in dphi[..2 * nsps].iter_mut().zip(pulse[nsps..].iter()) {
        *dst += lead_scale * p;
    }
    let tail_scale = dphi_peak * (tones[NN - 1] as f64);
    for (dst, &p) in dphi[NN * nsps..(NN + 2) * nsps]
        .iter_mut()
        .zip(pulse[..2 * nsps].iter())
    {
        *dst += tail_scale * p;
    }

    let dphi0 = TAU * af_hz / (sample_rate as f64);
    for x in &mut dphi {
        *x += dphi0;
    }

    (nsps, dphi)
}

/// Synthesizes a real `f32` active FT8 GFSK waveform (`79 * nsps` samples, `sin(phi)`).
pub fn gen_ft8_wave_real(tones: &[u8; NN], af_hz: f64, sample_rate: u32) -> Vec<f32> {
    let (nsps, dphi) = gen_ft8_dphi(tones, af_hz, sample_rate);
    let nwave = NN * nsps;
    let mut wave = Vec::with_capacity(nwave);
    let mut phi = 0.0_f64;

    for &dp in &dphi[nsps..nsps + nwave] {
        wave.push(phi.sin() as f32);
        phi = (phi + dp).rem_euclid(TAU);
    }

    let nramp = ((nsps as f64) / 8.0).round() as usize;
    apply_raised_cosine_ramps_real(&mut wave, nramp);
    wave
}

/// Synthesizes a complex `Complex32` analytic FT8 GFSK waveform (`79 * nsps` samples,
/// `cos(phi) + i*sin(phi)`) used both for oracle comparison and Stage 2 subtraction.
pub fn gen_ft8_wave_complex(tones: &[u8; NN], af_hz: f64, sample_rate: u32) -> Vec<Complex32> {
    let (nsps, dphi) = gen_ft8_dphi(tones, af_hz, sample_rate);
    let nwave = NN * nsps;
    let mut cwave = Vec::with_capacity(nwave);
    let mut phi = 0.0_f64;

    for &dp in &dphi[nsps..nsps + nwave] {
        cwave.push(Complex32::new(phi.cos() as f32, phi.sin() as f32));
        phi = (phi + dp).rem_euclid(TAU);
    }

    let nramp = ((nsps as f64) / 8.0).round() as usize;
    apply_raised_cosine_ramps_complex(&mut cwave, nramp);
    cwave
}
