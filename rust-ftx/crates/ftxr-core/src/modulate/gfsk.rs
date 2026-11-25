// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! GFSK pulse shaping, raised-cosine ramps, and UTC-aligned delay/offset/fade timing.

use std::f64::consts::{LN_2, PI};

use num_complex::Complex32;

use crate::ModulateConfig;

/// Gaussian frequency-smoothing pulse (`wsjtx/lib/ft2/gfsk_pulse.f90`):
/// `g(b, t) = 0.5 * (erf(c * b * (t + 0.5)) - erf(c * b * (t - 0.5)))`
/// where `c = pi * sqrt(2 / ln(2))`.
#[inline]
pub fn gfsk_pulse(bt: f64, t: f64) -> f64 {
    let c = PI * (2.0 / LN_2).sqrt();
    0.5 * (libm::erf(c * bt * (t + 0.5)) - libm::erf(c * bt * (t - 0.5)))
}

/// Builds a `3 * nsps` frequency-smoothing pulse lookup table on WSJT-X's exact grid
/// `t[idx] = ((idx + 1) - 1.5 * nsps) / nsps`.
pub fn build_gfsk_pulse_table(bt: f64, nsps: usize) -> Vec<f64> {
    let len = 3 * nsps;
    let nsps_f = nsps as f64;
    let mut pulse = Vec::with_capacity(len);
    for idx in 0..len {
        let t = ((idx + 1) as f64 - 1.5 * nsps_f) / nsps_f;
        pulse.push(gfsk_pulse(bt, t));
    }
    pulse
}

/// Applies raised-cosine ramp-up and ramp-down envelopes over `nramp` leading and
/// trailing samples of a real waveform.
pub fn apply_raised_cosine_ramps_real(wave: &mut [f32], nramp: usize) {
    let n = wave.len();
    if nramp == 0 || n < 2 * nramp {
        return;
    }
    let nramp_f = nramp as f64;
    for i in 0..nramp {
        let w_up = 0.5 * (1.0 - (PI * (i as f64) / nramp_f).cos());
        let w_dn = 0.5 * (1.0 + (PI * (i as f64) / nramp_f).cos());
        wave[i] *= w_up as f32;
        wave[n - nramp + i] *= w_dn as f32;
    }
}

/// Applies raised-cosine ramp-up and ramp-down envelopes over `nramp` leading and
/// trailing samples of a complex analytic waveform.
pub fn apply_raised_cosine_ramps_complex(cwave: &mut [Complex32], nramp: usize) {
    let n = cwave.len();
    if nramp == 0 || n < 2 * nramp {
        return;
    }
    let nramp_f = nramp as f64;
    for i in 0..nramp {
        let w_up = (0.5 * (1.0 - (PI * (i as f64) / nramp_f).cos())) as f32;
        let w_dn = (0.5 * (1.0 + (PI * (i as f64) / nramp_f).cos())) as f32;
        cwave[i] *= w_up;
        cwave[n - nramp + i] *= w_dn;
    }
}

/// Applies `start_delay_ms`, `offset_ms` UTC-aligned late-TX slicing with click-free
/// raised-cosine fade-in, and `amplitude` scaling.
pub fn apply_delay_offset_and_scale(
    mut active: Vec<f32>,
    config: &ModulateConfig,
    late_fade_samples: usize,
) -> Vec<f32> {
    let fs = config.sample_rate as f64;
    let delay_samples = ((config.start_delay_ms as f64) * fs / 1000.0).round() as usize;
    let offset_samples = ((config.offset_ms as f64) * fs / 1000.0).round() as usize;
    let amp = config.amplitude;

    if offset_samples < delay_samples {
        let lead_zeros = delay_samples - offset_samples;
        let mut out = vec![0.0_f32; lead_zeros + active.len()];
        for (dst, &src) in out[lead_zeros..].iter_mut().zip(active.iter()) {
            *dst = src * amp;
        }
        out
    } else if offset_samples == delay_samples {
        if (amp - 1.0).abs() > f32::EPSILON {
            for s in &mut active {
                *s *= amp;
            }
        }
        active
    } else {
        let skip = offset_samples - delay_samples;
        if skip >= active.len() {
            return Vec::new();
        }
        let mut tail = active.split_off(skip);
        let n_fade = late_fade_samples.max(1).min(tail.len());
        let n_fade_f = n_fade as f64;
        for (i, s) in tail.iter_mut().take(n_fade).enumerate() {
            let env = 0.5 * (1.0 - (PI * (i as f64) / n_fade_f).cos());
            *s *= (env as f32) * amp;
        }
        if (amp - 1.0).abs() > f32::EPSILON {
            for s in tail.iter_mut().skip(n_fade) {
                *s *= amp;
            }
        }
        tail
    }
}
