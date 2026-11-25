// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! FT8 and FT4 GFSK transmit waveform modulation.

pub mod ft4;
pub mod ft8;
pub mod gfsk;

use crate::protocol::hashtable::CallsignHashTable;
use crate::protocol::pack77::pack77;
use crate::protocol::types::UnpackContext;
use crate::{FtxrError, Mode, ModulateConfig};

use self::ft4::{encode_ft4_tones_from_77bits, ft4_nsps, gen_ft4_wave_real};
use self::ft8::{encode_ft8_tones_from_77bits, ft8_nsps, gen_ft8_wave_real};
use self::gfsk::apply_delay_offset_and_scale;

/// Encoded channel symbol tones (`79` for FT8, `103` data/sync for FT4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodedTones {
    Ft8([u8; 79]),
    Ft4([u8; 103]),
}

/// Complete modulated TX waveform and metadata produced by `modulate_message`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModulatedWaveform {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub mode: Mode,
    pub msgsent: String,
    pub tones: EncodedTones,
    pub i3: u8,
    pub n3: u8,
}

fn validate_config(config: &ModulateConfig) -> Result<(), FtxrError> {
    if config.sample_rate < 8_000 {
        return Err(FtxrError::InvalidSampleRate(config.sample_rate));
    }
    let min_hz = 50.0_f32;
    let max_hz = 4500.0_f32.min((config.sample_rate as f32) * 0.5 - 100.0);
    if !config.af_hz.is_finite() || config.af_hz < min_hz || config.af_hz > max_hz {
        return Err(FtxrError::InvalidFrequency {
            af_hz: config.af_hz,
            min_hz,
            max_hz,
        });
    }
    if !config.amplitude.is_finite() || config.amplitude <= 0.0 || config.amplitude > 1.0 {
        return Err(FtxrError::InvalidConfig(format!(
            "amplitude {} must be in (0.0, 1.0]",
            config.amplitude
        )));
    }
    Ok(())
}

/// Encodes and GFSK-modulates a text message according to `config`.
pub fn modulate_message(
    msg: &str,
    config: &ModulateConfig,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<ModulatedWaveform, FtxrError> {
    validate_config(config)?;
    let packed = pack77(msg, None, ctx, hash_table)?;
    let tones = match config.mode {
        Mode::Ft8 => EncodedTones::Ft8(encode_ft8_tones_from_77bits(&packed.bits)),
        Mode::Ft4 => EncodedTones::Ft4(encode_ft4_tones_from_77bits(&packed.bits)),
    };
    let samples = modulate_tones(&tones, config)?;
    Ok(ModulatedWaveform {
        samples,
        sample_rate: config.sample_rate,
        mode: config.mode,
        msgsent: packed.msgsent,
        tones,
        i3: packed.i3,
        n3: packed.n3,
    })
}

/// Synthesizes GFSK PCM samples for pre-encoded channel tones with `start_delay_ms`,
/// `offset_ms`, and `amplitude` applied.
pub fn modulate_tones(tones: &EncodedTones, config: &ModulateConfig) -> Result<Vec<f32>, FtxrError> {
    validate_config(config)?;
    match (config.mode, tones) {
        (Mode::Ft8, EncodedTones::Ft8(t)) => {
            for &tone in t {
                if tone > 7 {
                    return Err(FtxrError::InvalidTone {
                        mode: Mode::Ft8,
                        tone,
                    });
                }
            }
            let active = gen_ft8_wave_real(t, config.af_hz as f64, config.sample_rate);
            let nsps = ft8_nsps(config.sample_rate);
            let late_fade = (nsps / 8).max(1);
            Ok(apply_delay_offset_and_scale(active, config, late_fade))
        }
        (Mode::Ft4, EncodedTones::Ft4(t)) => {
            for &tone in t {
                if tone > 3 {
                    return Err(FtxrError::InvalidTone {
                        mode: Mode::Ft4,
                        tone,
                    });
                }
            }
            let active = gen_ft4_wave_real(t, config.af_hz as f64, config.sample_rate);
            let nsps = ft4_nsps(config.sample_rate);
            let late_fade = (nsps / 4).max(1);
            Ok(apply_delay_offset_and_scale(active, config, late_fade))
        }
        _ => Err(FtxrError::InvalidConfig(format!(
            "tones mode mismatch for {:?}",
            config.mode
        ))),
    }
}

/// Modulates a message and places it into an exact `slot_samples` zero-padded buffer
/// (convenient for offline WAV generation and `jt9` loopback testing).
pub fn modulate_slot_pcm(
    msg: &str,
    config: &ModulateConfig,
    slot_samples: usize,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<(Vec<f32>, ModulatedWaveform), FtxrError> {
    let wave = modulate_message(msg, config, ctx, hash_table)?;
    let mut slot = vec![0.0_f32; slot_samples];
    let fs = config.sample_rate as f64;
    let start_idx = ((config.offset_ms as f64) * fs / 1000.0).round() as usize;
    if start_idx < slot_samples && !wave.samples.is_empty() {
        let copy_len = wave.samples.len().min(slot_samples - start_idx);
        slot[start_idx..start_idx + copy_len].copy_from_slice(&wave.samples[..copy_len]);
    }
    Ok((slot, wave))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulate::ft4::gen_ft4_wave_complex;
    use crate::modulate::ft8::gen_ft8_wave_complex;
    use std::f64::consts::PI;

    #[test]
    fn sample_counts_delays_and_offsets_match_spec() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();

        // FT8 raw active @ 12 kHz and 48 kHz
        let w12 = modulate_message(
            "CQ N1MPM FN33",
            &ModulateConfig::raw_active(Mode::Ft8, 1500.0, 12_000),
            &ctx,
            &ht,
        )
        .unwrap();
        assert_eq!(w12.samples.len(), 151_680);

        let w48 = modulate_message(
            "CQ N1MPM FN33",
            &ModulateConfig::raw_active(Mode::Ft8, 1500.0, 48_000),
            &ctx,
            &ht,
        )
        .unwrap();
        assert_eq!(w48.samples.len(), 606_720);

        // FT8 nominal 500 ms start delay
        let w12_nom = modulate_message(
            "CQ N1MPM FN33",
            &ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000),
            &ctx,
            &ht,
        )
        .unwrap();
        assert_eq!(w12_nom.samples.len(), 6_000 + 151_680);
        assert!(w12_nom.samples[..6_000].iter().all(|&x| x == 0.0));

        // digi-dx parity: 5000 ms offset @ 48 kHz with start_delay_ms = 0
        let mut cfg_late = ModulateConfig::raw_active(Mode::Ft8, 1500.0, 48_000);
        cfg_late.offset_ms = 5_000;
        let w48_late = modulate_message("CQ N1MPM FN33", &cfg_late, &ctx, &ht).unwrap();
        assert_eq!(w48_late.samples.len(), 606_720 - 240_000);
        assert!(w48_late.samples[0].abs() < 1e-6);

        // Past-end offset returns empty Vec
        let mut cfg_past = ModulateConfig::raw_active(Mode::Ft8, 1500.0, 48_000);
        cfg_past.offset_ms = 15_000;
        let w_past = modulate_message("CQ N1MPM FN33", &cfg_past, &ctx, &ht).unwrap();
        assert!(w_past.samples.is_empty());

        // FT4 raw active & nominal 300 ms start delay @ 12 kHz and 48 kHz
        let f4_12 = modulate_message(
            "CQ N1MPM FN33",
            &ModulateConfig::raw_active(Mode::Ft4, 1500.0, 12_000),
            &ctx,
            &ht,
        )
        .unwrap();
        assert_eq!(f4_12.samples.len(), 60_480);

        let f4_48 = modulate_message(
            "CQ N1MPM FN33",
            &ModulateConfig::raw_active(Mode::Ft4, 1500.0, 48_000),
            &ctx,
            &ht,
        )
        .unwrap();
        assert_eq!(f4_48.samples.len(), 241_920);

        let f4_12_nom = modulate_message(
            "CQ N1MPM FN33",
            &ModulateConfig::default_for(Mode::Ft4, 1500.0, 12_000),
            &ctx,
            &ht,
        )
        .unwrap();
        assert_eq!(f4_12_nom.samples.len(), 3_600 + 60_480);
        assert!(f4_12_nom.samples[..3_600].iter().all(|&x| x == 0.0));
    }

    #[test]
    fn complex_envelope_and_phase_continuity() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        let packed = pack77("CQ K1ABC FN42", None, &ctx, &ht).unwrap();

        let tones8 = encode_ft8_tones_from_77bits(&packed.bits);
        let cw8 = gen_ft8_wave_complex(&tones8, 1500.0, 12_000);
        assert_eq!(cw8.len(), 151_680);
        let nramp8 = 1920 / 8;
        for (i, c) in cw8.iter().enumerate() {
            let expected_mag = if i < nramp8 {
                0.5 * (1.0 - (PI * (i as f64) / (nramp8 as f64)).cos())
            } else if i >= cw8.len() - nramp8 {
                let k = i - (cw8.len() - nramp8);
                0.5 * (1.0 + (PI * (k as f64) / (nramp8 as f64)).cos())
            } else {
                1.0
            };
            assert!((c.norm() - expected_mag as f32).abs() < 1e-5);
        }

        let tones4 = encode_ft4_tones_from_77bits(&packed.bits);
        let cw4 = gen_ft4_wave_complex(&tones4, 1500.0, 12_000);
        assert_eq!(cw4.len(), 60_480);
        let nramp4 = 576;
        for (i, c) in cw4.iter().enumerate() {
            let expected_mag = if i < nramp4 {
                0.5 * (1.0 - (PI * (i as f64) / (nramp4 as f64)).cos())
            } else if i >= cw4.len() - nramp4 {
                let k = i - (cw4.len() - nramp4);
                0.5 * (1.0 + (PI * (k as f64) / (nramp4 as f64)).cos())
            } else {
                1.0
            };
            assert!((c.norm() - expected_mag as f32).abs() < 1e-5);
        }
    }

    #[test]
    fn validation_rejects_bad_configs() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();

        let bad_sr = ModulateConfig::default_for(Mode::Ft8, 1500.0, 4_000);
        assert!(matches!(
            modulate_message("CQ K1ABC FN42", &bad_sr, &ctx, &ht),
            Err(FtxrError::InvalidSampleRate(4_000))
        ));

        let bad_af = ModulateConfig::default_for(Mode::Ft8, 4900.0, 12_000);
        assert!(matches!(
            modulate_message("CQ K1ABC FN42", &bad_af, &ctx, &ht),
            Err(FtxrError::InvalidFrequency { .. })
        ));

        let mut bad_amp = ModulateConfig::default_for(Mode::Ft8, 1500.0, 12_000);
        bad_amp.amplitude = 0.0;
        assert!(matches!(
            modulate_message("CQ K1ABC FN42", &bad_amp, &ctx, &ht),
            Err(FtxrError::InvalidConfig(_))
        ));
    }
}
