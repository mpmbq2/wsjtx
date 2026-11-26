// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Pure-Rust RIFF/WAV and raw PCM audio reader/writer (12 kHz mono/stereo downmix, i16 & f32).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use thiserror::Error;

pub const EXPECTED_SAMPLE_RATE_HZ: u32 = 12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    I16Le,
    F32Le,
}

impl SampleFormat {
    pub fn parse_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "i16" | "i16le" | "s16" | "s16le" | "pcm16" => Some(Self::I16Le),
            "f32" | "f32le" | "float" | "float32" => Some(Self::F32Le),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::I16Le => "i16le",
            Self::F32Le => "f32le",
        }
    }

    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::I16Le => 2,
            Self::F32Le => 4,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PcmSamples {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

impl PcmSamples {
    pub fn len(&self) -> usize {
        match self {
            Self::I16(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Error)]
pub enum WavError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid RIFF/WAVE header: {0}")]
    InvalidHeader(String),
    #[error("unsupported WAV sample rate {0} Hz (expected 12000 Hz)")]
    UnsupportedSampleRate(u32),
    #[error("unsupported WAV format: {0}")]
    UnsupportedFormat(String),
    #[error("raw PCM byte length {0} is not a multiple of sample size {1}")]
    UnalignedPcmBytes(usize, usize),
}

/// Reads a WAV file from disk, supporting PCM 16-bit and IEEE float 32-bit, arbitrary BWF/metadata
/// chunks (with RIFF even-byte padding), and multi-channel downmix to mono 12 kHz.
pub fn read_wav_12k(path: &Path) -> Result<PcmSamples, WavError> {
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    decode_wav_bytes_12k(&bytes)
}

/// Decodes an in-memory RIFF/WAV byte slice to mono 12 kHz `PcmSamples`.
pub fn decode_wav_bytes_12k(bytes: &[u8]) -> Result<PcmSamples, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::InvalidHeader(
            "missing RIFF or WAVE signature".to_string(),
        ));
    }

    let mut pos = 12usize;
    let mut fmt_parsed = false;
    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;

        let end = pos.saturating_add(chunk_size).min(bytes.len());
        let chunk = &bytes[pos..end];

        if chunk_id == b"fmt " {
            if chunk.len() < 16 {
                return Err(WavError::InvalidHeader(
                    "fmt chunk shorter than 16 bytes".to_string(),
                ));
            }
            let raw_fmt = u16::from_le_bytes([chunk[0], chunk[1]]);
            channels = u16::from_le_bytes([chunk[2], chunk[3]]);
            sample_rate = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            bits_per_sample = u16::from_le_bytes([chunk[14], chunk[15]]);
            audio_format = if raw_fmt == 0xFFFE {
                if chunk.len() < 26 {
                    return Err(WavError::UnsupportedFormat(
                        "truncated WAVE_FORMAT_EXTENSIBLE chunk".to_string(),
                    ));
                }
                u16::from_le_bytes([chunk[24], chunk[25]])
            } else {
                raw_fmt
            };
            fmt_parsed = true;
        } else if chunk_id == b"data" {
            data_slice = Some(chunk);
            if fmt_parsed {
                break;
            }
        }

        // RIFF chunks are padded to an even byte boundary.
        let padded = chunk_size + (chunk_size & 1);
        pos = pos.saturating_add(padded);
    }

    if !fmt_parsed {
        return Err(WavError::InvalidHeader("missing fmt chunk".to_string()));
    }
    if sample_rate != EXPECTED_SAMPLE_RATE_HZ {
        return Err(WavError::UnsupportedSampleRate(sample_rate));
    }
    if channels == 0 {
        return Err(WavError::UnsupportedFormat(
            "zero audio channels".to_string(),
        ));
    }

    let data = data_slice.ok_or_else(|| WavError::InvalidHeader("missing data chunk".to_string()))?;
    let ch = channels as usize;

    match (audio_format, bits_per_sample) {
        (1, 16) => {
            let frame_bytes = 2 * ch;
            let n_frames = data.len() / frame_bytes;
            let mut out = Vec::with_capacity(n_frames);
            if ch == 1 {
                for frame in data[..n_frames * 2].chunks_exact(2) {
                    out.push(i16::from_le_bytes([frame[0], frame[1]]));
                }
            } else {
                for frame in data[..n_frames * frame_bytes].chunks_exact(frame_bytes) {
                    let mut acc = 0i32;
                    for s in frame.chunks_exact(2) {
                        acc += i32::from(i16::from_le_bytes([s[0], s[1]]));
                    }
                    out.push((acc / (ch as i32)).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
                }
            }
            Ok(PcmSamples::I16(out))
        }
        (3, 32) => {
            let frame_bytes = 4 * ch;
            let n_frames = data.len() / frame_bytes;
            let mut out = Vec::with_capacity(n_frames);
            if ch == 1 {
                for frame in data[..n_frames * 4].chunks_exact(4) {
                    let v = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
                    out.push(if v.is_finite() { v } else { 0.0 });
                }
            } else {
                let inv_ch = 1.0_f32 / (ch as f32);
                for frame in data[..n_frames * frame_bytes].chunks_exact(frame_bytes) {
                    let mut acc = 0.0_f32;
                    for s in frame.chunks_exact(4) {
                        let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                        if v.is_finite() {
                            acc += v;
                        }
                    }
                    out.push(acc * inv_ch);
                }
            }
            Ok(PcmSamples::F32(out))
        }
        _ => Err(WavError::UnsupportedFormat(format!(
            "format tag {audio_format}, {bits_per_sample} bits/sample (expected PCM i16 or IEEE float f32)"
        ))),
    }
}

/// Decodes raw little-endian PCM bytes (`i16le` or `f32le`) into `PcmSamples`.
pub fn decode_raw_pcm_bytes(bytes: &[u8], fmt: SampleFormat) -> Result<PcmSamples, WavError> {
    let bps = fmt.bytes_per_sample();
    if bytes.len() % bps != 0 {
        return Err(WavError::UnalignedPcmBytes(bytes.len(), bps));
    }
    match fmt {
        SampleFormat::I16Le => {
            let mut out = Vec::with_capacity(bytes.len() / 2);
            for c in bytes.chunks_exact(2) {
                out.push(i16::from_le_bytes([c[0], c[1]]));
            }
            Ok(PcmSamples::I16(out))
        }
        SampleFormat::F32Le => {
            let mut out = Vec::with_capacity(bytes.len() / 4);
            for c in bytes.chunks_exact(4) {
                out.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
            Ok(PcmSamples::F32(out))
        }
    }
}

/// Reads headerless little-endian raw PCM from a file on disk.
pub fn read_raw_pcm_file(path: &Path, fmt: SampleFormat) -> Result<PcmSamples, WavError> {
    let bytes = std::fs::read(path)?;
    decode_raw_pcm_bytes(&bytes, fmt)
}

/// Encodes normalized `f32` PCM (`[-1.0, 1.0]`) into raw little-endian bytes (`i16le` or `f32le`).
pub fn encode_raw_pcm_bytes(samples: &[f32], fmt: SampleFormat) -> Vec<u8> {
    match fmt {
        SampleFormat::I16Le => {
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for &s in samples {
                let scaled = (s.clamp(-1.0, 1.0) * 32767.0).round() as i32;
                let v = scaled.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            bytes
        }
        SampleFormat::F32Le => {
            let mut bytes = Vec::with_capacity(samples.len() * 4);
            for &s in samples {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            bytes
        }
    }
}

/// Writes a 44-byte RIFF/WAVE mono 16-bit PCM file (`f32` normalized in `[-1.0, 1.0]`).
pub fn write_wav_f32_as_i16(path: &Path, sample_rate_hz: u32, samples: &[f32]) -> Result<(), WavError> {
    let raw = encode_raw_pcm_bytes(samples, SampleFormat::I16Le);
    write_wav_bytes(path, 1, sample_rate_hz, 16, &raw)
}

/// Writes a 44-byte RIFF/WAVE mono 32-bit IEEE float file.
pub fn write_wav_f32(path: &Path, sample_rate_hz: u32, samples: &[f32]) -> Result<(), WavError> {
    let raw = encode_raw_pcm_bytes(samples, SampleFormat::F32Le);
    write_wav_bytes(path, 3, sample_rate_hz, 32, &raw)
}

/// Writes headerless raw PCM bytes to disk.
pub fn write_raw_pcm_file(path: &Path, samples: &[f32], fmt: SampleFormat) -> Result<(), WavError> {
    let f = File::create(path)?;
    let mut w = BufWriter::new(f);
    let bytes = encode_raw_pcm_bytes(samples, fmt);
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

fn write_wav_bytes(
    path: &Path,
    format_tag: u16,
    sample_rate_hz: u32,
    bits_per_sample: u16,
    data: &[u8],
) -> Result<(), WavError> {
    let f = File::create(path)?;
    let mut w = BufWriter::new(f);
    let channels: u16 = 1;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate_hz * u32::from(block_align);
    let data_len = data.len() as u32;
    let riff_len = 36u32.saturating_add(data_len);

    w.write_all(b"RIFF")?;
    w.write_all(&riff_len.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&format_tag.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate_hz.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits_per_sample.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    w.write_all(data)?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_wav_i16_and_f32_and_skips_bwf_chunks_and_downmixes_stereo() {
        let dir = tempfile::tempdir().unwrap();

        // 1. i16 roundtrip
        let samples = vec![0.0_f32, 0.5, -0.5, 1.0, -1.0];
        let p_i16 = dir.path().join("test_i16.wav");
        write_wav_f32_as_i16(&p_i16, 12_000, &samples).unwrap();
        match read_wav_12k(&p_i16).unwrap() {
            PcmSamples::I16(v) => assert_eq!(v, vec![0, 16384, -16384, 32767, -32767]),
            _ => panic!("expected i16"),
        }

        // 2. f32 roundtrip
        let p_f32 = dir.path().join("test_f32.wav");
        write_wav_f32(&p_f32, 12_000, &samples).unwrap();
        match read_wav_12k(&p_f32).unwrap() {
            PcmSamples::F32(v) => assert_eq!(v, samples),
            _ => panic!("expected f32"),
        }

        // 3. Odd-length custom chunk skipping + stereo downmix
        let mut stereo_wav = Vec::new();
        let data_bytes = [
            100i16.to_le_bytes(),
            300i16.to_le_bytes(),
            (-200i16).to_le_bytes(),
            400i16.to_le_bytes(),
        ]
        .concat();
        let junk_len = 3u32; // odd length -> padded to 4 bytes
        let riff_size = (4 + 8 + 16 + 8 + 4 + 8 + data_bytes.len()) as u32;
        stereo_wav.extend_from_slice(b"RIFF");
        stereo_wav.extend_from_slice(&riff_size.to_le_bytes());
        stereo_wav.extend_from_slice(b"WAVE");
        stereo_wav.extend_from_slice(b"JUNK");
        stereo_wav.extend_from_slice(&junk_len.to_le_bytes());
        stereo_wav.extend_from_slice(b"ABC\0");
        stereo_wav.extend_from_slice(b"fmt ");
        stereo_wav.extend_from_slice(&16u32.to_le_bytes());
        stereo_wav.extend_from_slice(&1u16.to_le_bytes());
        stereo_wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        stereo_wav.extend_from_slice(&12_000u32.to_le_bytes());
        stereo_wav.extend_from_slice(&(12_000u32 * 4).to_le_bytes());
        stereo_wav.extend_from_slice(&4u16.to_le_bytes());
        stereo_wav.extend_from_slice(&16u16.to_le_bytes());
        stereo_wav.extend_from_slice(b"data");
        stereo_wav.extend_from_slice(&(data_bytes.len() as u32).to_le_bytes());
        stereo_wav.extend_from_slice(&data_bytes);

        match decode_wav_bytes_12k(&stereo_wav).unwrap() {
            PcmSamples::I16(v) => assert_eq!(v, vec![200, 100]),
            _ => panic!("expected stereo downmixed i16"),
        }
    }

    #[test]
    fn rejects_non_12k_sample_rate() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad_sr.wav");
        write_wav_f32_as_i16(&p, 48_000, &[0.0; 100]).unwrap();
        assert!(matches!(
            read_wav_12k(&p),
            Err(WavError::UnsupportedSampleRate(48_000))
        ));
    }
}
