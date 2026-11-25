// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! 12 kHz mono 16-bit PCM WAV reader (RIFF/BWF chunk-aware) and 44-byte writer,
//! plus SHA-256 hashing and WSJT-X `-fbounds-check` filename validation.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Standard sample rate for WSJT-X FT8/FT4 baseband WAVs.
pub const SAMPLE_RATE_12K: u32 = 12_000;

#[derive(Debug, Error)]
pub enum WavError {
    #[error("I/O error reading/writing WAV {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid WAV format in {path}: {reason}")]
    InvalidFormat { path: String, reason: String },
}

#[derive(Debug, Clone)]
pub struct WavData {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub samples: Vec<i16>,
}

/// Reads a PCM 16-bit WAV file, cleanly skipping arbitrary RIFF/BWF chunks (`bext`, `junk`,
/// `LIST`, etc.) with even-byte alignment matching `wsjtx/lib/readwav.f90:60-70`.
pub fn read_wav(path: &Path) -> Result<WavData, WavError> {
    let path_str = path.display().to_string();
    let file = File::open(path).map_err(|source| WavError::Io {
        path: path_str.clone(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|source| WavError::Io {
        path: path_str.clone(),
        source,
    })?;

    parse_wav_bytes(&bytes, &path_str)
}

/// Reads a WAV file and validates that it is 12,000 Hz mono 16-bit linear PCM.
pub fn read_wav_12k_mono(path: &Path) -> Result<Vec<i16>, WavError> {
    let wav = read_wav(path)?;
    if wav.sample_rate != SAMPLE_RATE_12K || wav.channels != 1 || wav.bits_per_sample != 16 {
        return Err(WavError::InvalidFormat {
            path: path.display().to_string(),
            reason: format!(
                "expected 12000 Hz mono 16-bit PCM, got {} Hz {} ch {}-bit",
                wav.sample_rate, wav.channels, wav.bits_per_sample
            ),
        });
    }
    Ok(wav.samples)
}

fn parse_wav_bytes(data: &[u8], path_str: &str) -> Result<WavData, WavError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavError::InvalidFormat {
            path: path_str.to_string(),
            reason: "missing RIFF/WAVE header".to_string(),
        });
    }

    let mut pos = 12usize;
    let mut audio_format: Option<u16> = None;
    let mut channels: Option<u16> = None;
    let mut sample_rate: Option<u32> = None;
    let mut bits_per_sample: Option<u16> = None;
    let mut samples: Option<Vec<i16>> = None;

    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        pos += 8;

        let available = data.len().saturating_sub(pos);
        let payload_len = chunk_size.min(available);
        let payload = &data[pos..pos + payload_len];

        if chunk_id == b"fmt " {
            if payload.len() < 16 {
                return Err(WavError::InvalidFormat {
                    path: path_str.to_string(),
                    reason: format!("fmt chunk too short ({} bytes)", payload.len()),
                });
            }
            audio_format = Some(u16::from_le_bytes([payload[0], payload[1]]));
            channels = Some(u16::from_le_bytes([payload[2], payload[3]]));
            sample_rate = Some(u32::from_le_bytes([
                payload[4], payload[5], payload[6], payload[7],
            ]));
            bits_per_sample = Some(u16::from_le_bytes([payload[14], payload[15]]));
        } else if chunk_id == b"data" {
            let n_samples = payload.len() / 2;
            let mut out = Vec::with_capacity(n_samples);
            for frame in payload.chunks_exact(2) {
                out.push(i16::from_le_bytes([frame[0], frame[1]]));
            }
            samples = Some(out);
        }

        // RIFF chunks are padded to an even byte boundary.
        let padded = chunk_size.saturating_add(chunk_size & 1);
        pos = pos.saturating_add(padded);
    }

    let fmt = audio_format.ok_or_else(|| WavError::InvalidFormat {
        path: path_str.to_string(),
        reason: "missing fmt chunk".to_string(),
    })?;
    if fmt != 1 {
        return Err(WavError::InvalidFormat {
            path: path_str.to_string(),
            reason: format!("unsupported WAV audio format {fmt} (only PCM=1 supported)"),
        });
    }

    let channels = channels.unwrap_or(0);
    let sample_rate = sample_rate.unwrap_or(0);
    let bits_per_sample = bits_per_sample.unwrap_or(0);
    if bits_per_sample != 16 {
        return Err(WavError::InvalidFormat {
            path: path_str.to_string(),
            reason: format!("unsupported bits_per_sample {bits_per_sample} (expected 16)"),
        });
    }

    let samples = samples.ok_or_else(|| WavError::InvalidFormat {
        path: path_str.to_string(),
        reason: "missing data chunk".to_string(),
    })?;

    Ok(WavData {
        sample_rate,
        channels,
        bits_per_sample,
        samples,
    })
}

/// Writes a standard 44-byte RIFF/WAVE 12 kHz mono 16-bit PCM file matching `wsjtx/lib/wavhdr.f90`.
pub fn write_wav_12k_mono(path: &Path, samples: &[i16]) -> Result<(), WavError> {
    let path_str = path.display().to_string();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| WavError::Io {
                path: path_str.clone(),
                source,
            })?;
        }
    }

    let data_bytes: u32 = (samples.len() * 2)
        .try_into()
        .map_err(|_| WavError::InvalidFormat {
            path: path_str.clone(),
            reason: "WAV sample count exceeds 32-bit RIFF size".to_string(),
        })?;
    let riff_size: u32 = 36u32.saturating_add(data_bytes);
    let byte_rate: u32 = SAMPLE_RATE_12K * 2;
    let block_align: u16 = 2;

    let file = File::create(path).map_err(|source| WavError::Io {
        path: path_str.clone(),
        source,
    })?;
    let mut writer = BufWriter::new(file);

    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&1u16.to_le_bytes()); // Mono
    header[24..28].copy_from_slice(&SAMPLE_RATE_12K.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes()); // 16-bit
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());

    writer.write_all(&header).map_err(|source| WavError::Io {
        path: path_str.clone(),
        source,
    })?;

    for &s in samples {
        writer
            .write_all(&s.to_le_bytes())
            .map_err(|source| WavError::Io {
                path: path_str.clone(),
                source,
            })?;
    }

    writer.flush().map_err(|source| WavError::Io {
        path: path_str,
        source,
    })
}

/// Checks whether a WAV path has a filename safe for Release `jt9` (`-fbounds-check` requires
/// at least 6 characters before `.wav`, e.g. `YYMMDD_HHMMSS.wav`).
pub fn is_jt9_safe_filename(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".wav") else {
        return false;
    };
    if stem.len() < 6 {
        return false;
    }
    let tail = &stem[stem.len() - 6..];
    tail.bytes().all(|b| b.is_ascii_digit())
}

/// Computes lowercase hex SHA-256 of a byte slice.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

/// Computes lowercase hex SHA-256 of a file on disk.
pub fn sha256_file(path: &Path) -> Result<String, WavError> {
    let path_str = path.display().to_string();
    let file = File::open(path).map_err(|source| WavError::Io {
        path: path_str.clone(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|source| WavError::Io {
            path: path_str.clone(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn wsjtx_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .expect("wsjtx root")
    }

    #[test]
    fn reads_wsjtx_bwf_and_standard_samples() {
        let root = wsjtx_root();

        let ft8_sample = root.join("samples/FT8/210703_133430.wav");
        let ft8 = read_wav_12k_mono(&ft8_sample).expect("read FT8 BWF sample");
        assert_eq!(ft8.len(), 180_000);

        let ft4_sample = root.join("samples/FT4/000000_000002.wav");
        let ft4 = read_wav_12k_mono(&ft4_sample).expect("read FT4 sample");
        assert_eq!(ft4.len(), 72_576);

        let committed_191111 =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/committed/ft8/191111_110200.wav");
        if committed_191111.exists() {
            let s = read_wav_12k_mono(&committed_191111).expect("read 191111_110200.wav");
            assert_eq!(s.len(), 180_000);
        }
    }

    #[test]
    fn roundtrips_wav_write_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("000000_000001.wav");
        let input: Vec<i16> = (-1000..1000).collect();
        write_wav_12k_mono(&path, &input).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len() as usize, 44 + input.len() * 2);
        let output = read_wav_12k_mono(&path).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn validates_jt9_safe_filenames() {
        assert!(is_jt9_safe_filename(Path::new("000000_000001.wav")));
        assert!(is_jt9_safe_filename(Path::new("/tmp/210703_133430.wav")));
        assert!(is_jt9_safe_filename(Path::new("191111_110200.WAV")));
        assert!(!is_jt9_safe_filename(Path::new("a.wav")));
        assert!(!is_jt9_safe_filename(Path::new("test.wav")));
        assert!(!is_jt9_safe_filename(Path::new("abcdef.wav")));
    }
}
