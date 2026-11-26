// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! 16-byte little-endian `FTXR` v1 binary packet framing.

use std::io::{self, Read, Write};

use thiserror::Error;

pub const FTXR_MAGIC: [u8; 4] = *b"FTXR";
pub const FTXR_IPC_VERSION: u16 = 1;
pub const FLAG_HAS_BINARY: u16 = 0x0001;
pub const FLAG_BINARY_F32LE: u16 = 0x0002;

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TOTAL_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub version: u16,
    pub flags: u16,
    pub json: Vec<u8>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("I/O failure: {0}")]
    Io(#[from] io::Error),
    #[error("fatal frame desync: {0}")]
    Desync(String),
}

/// Reads the next `FTXR` frame from `reader`.
///
/// Returns `Ok(None)` solely on a clean EOF right at a frame boundary; returns
/// `Err(FrameError::Desync)` on bad magic bytes, oversized lengths, or mid-frame EOF.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<RawFrame>, FrameError> {
    let mut hdr = [0u8; 16];
    let mut read = 0usize;
    while read < 16 {
        match reader.read(&mut hdr[read..]) {
            Ok(0) => {
                if read == 0 {
                    return Ok(None);
                }
                return Err(FrameError::Desync(format!(
                    "unexpected EOF after {read}/16 header bytes"
                )));
            }
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FrameError::Io(e)),
        }
    }

    if hdr[0..4] != FTXR_MAGIC {
        return Err(FrameError::Desync(format!(
            "invalid magic bytes {:?} (expected FTXR)",
            &hdr[0..4]
        )));
    }

    let version = u16::from_le_bytes([hdr[4], hdr[5]]);
    let flags = u16::from_le_bytes([hdr[6], hdr[7]]);
    let json_len = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    let payload_len = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) as usize;

    if json_len == 0 || json_len > MAX_JSON_BYTES {
        return Err(FrameError::Desync(format!(
            "invalid json_len {json_len} (allowed 1..={MAX_JSON_BYTES})"
        )));
    }
    if payload_len > MAX_PAYLOAD_BYTES || json_len.saturating_add(payload_len) > MAX_TOTAL_FRAME_BYTES
    {
        return Err(FrameError::Desync(format!(
            "frame too large: json={json_len}, payload={payload_len}"
        )));
    }

    let mut json = vec![0u8; json_len];
    if let Err(e) = reader.read_exact(&mut json) {
        return match e.kind() {
            io::ErrorKind::UnexpectedEof => {
                Err(FrameError::Desync("truncated JSON body before EOF".to_string()))
            }
            _ => Err(FrameError::Io(e)),
        };
    }

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        if let Err(e) = reader.read_exact(&mut payload) {
            return match e.kind() {
                io::ErrorKind::UnexpectedEof => Err(FrameError::Desync(
                    "truncated binary payload before EOF".to_string(),
                )),
                _ => Err(FrameError::Io(e)),
            };
        }
    }

    Ok(Some(RawFrame {
        version,
        flags,
        json,
        payload,
    }))
}

/// Serializes and flushes one `FTXR` v1 frame onto `writer`.
pub fn write_frame<W: Write>(
    writer: &mut W,
    flags: u16,
    json: &[u8],
    payload: &[u8],
) -> io::Result<()> {
    let eff_flags = flags | if payload.is_empty() { 0 } else { FLAG_HAS_BINARY };
    let mut hdr = [0u8; 16];
    hdr[0..4].copy_from_slice(&FTXR_MAGIC);
    hdr[4..6].copy_from_slice(&FTXR_IPC_VERSION.to_le_bytes());
    hdr[6..8].copy_from_slice(&eff_flags.to_le_bytes());
    hdr[8..12].copy_from_slice(&(json.len() as u32).to_le_bytes());
    hdr[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());

    writer.write_all(&hdr)?;
    writer.write_all(json)?;
    if !payload.is_empty() {
        writer.write_all(payload)?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrips_frame_and_detects_eof_and_desync() {
        let mut wire = Vec::new();
        let json = br#"{"op":"ping","id":"1"}"#;
        let pcm = vec![1u8, 2, 3, 4, 5, 6];
        write_frame(&mut wire, FLAG_HAS_BINARY, json, &pcm).unwrap();

        let mut cur = Cursor::new(&wire);
        let parsed = read_frame(&mut cur).unwrap().expect("frame");
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.flags & FLAG_HAS_BINARY, FLAG_HAS_BINARY);
        assert_eq!(parsed.json, json);
        assert_eq!(parsed.payload, pcm);
        assert!(read_frame(&mut cur).unwrap().is_none());

        // Corrupt magic
        let mut bad = wire.clone();
        bad[0] = b'X';
        assert!(matches!(
            read_frame(&mut Cursor::new(&bad)),
            Err(FrameError::Desync(_))
        ));

        // Truncated header
        assert!(matches!(
            read_frame(&mut Cursor::new(&wire[..7])),
            Err(FrameError::Desync(_))
        ));
    }
}
