// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Standalone oracle test client for spawning and driving `ftxr-modem --worker` over stdio IPC v1.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use ftxr_core::{DecodeTelemetry, Mode};

pub const FTXR_MAGIC: [u8; 4] = *b"FTXR";
pub const FTXR_VERSION: u16 = 1;
pub const FLAG_HAS_BINARY: u16 = 0x0001;
#[allow(dead_code)]
pub const FLAG_BINARY_F32LE: u16 = 0x0002;

#[derive(Debug, Error)]
pub enum IpcClientError {
    #[error("failed to locate ftxr-modem binary under {0}")]
    BinaryNotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol error ({code}): {message}")]
    RemoteError { code: String, message: String },
    #[error("unexpected worker frame: {0}")]
    Protocol(String),
}

#[derive(Debug, Clone)]
pub struct ClientFrame {
    #[allow(dead_code)]
    pub version: u16,
    #[allow(dead_code)]
    pub flags: u16,
    pub json: Vec<u8>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WireDecode {
    pub mode: Mode,
    pub snr_db: i8,
    pub snr_db_exact: f32,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub af_hz_rounded: i32,
    pub message: String,
    pub ap_tag: Option<String>,
    pub uncertain: bool,
    pub raw_metadata: Option<String>,
    #[serde(default)]
    pub telemetry: Option<DecodeTelemetry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodeSlotOutcome {
    pub id: String,
    pub status: String,
    pub decoded_count: usize,
    pub worker_elapsed_ms: f64,
    pub wall_ms: f64,
    pub first_event_ms: Option<f64>,
    pub decodes: Vec<WireDecode>,
}

#[derive(Debug, Clone)]
pub struct ProcMemoryStats {
    pub vm_rss_kb: u64,
    pub threads: usize,
}

/// Resolves `ftxr-modem` binary path (preferring `target/release/ftxr-modem`, falling back to `target/debug/ftxr-modem`).
pub fn resolve_modem_binary(rust_ftx_root: &Path, explicit: Option<&Path>) -> Result<PathBuf, IpcClientError> {
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(IpcClientError::BinaryNotFound(p.display().to_string()));
    }
    let rel = rust_ftx_root.join("target/release/ftxr-modem");
    if rel.exists() {
        return Ok(rel);
    }
    let dbg = rust_ftx_root.join("target/debug/ftxr-modem");
    if dbg.exists() {
        return Ok(dbg);
    }
    Err(IpcClientError::BinaryNotFound(
        rust_ftx_root.join("target/{release,debug}/ftxr-modem").display().to_string(),
    ))
}

/// Reads Linux `/proc/<pid>/status` (`VmRSS` in KiB, `Threads` count).
pub fn read_proc_status(pid: u32) -> Option<ProcMemoryStats> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let mut vm_rss_kb = None;
    let mut threads = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let num = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            vm_rss_kb = Some(num);
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            let num = rest.trim().parse::<usize>().ok()?;
            threads = Some(num);
        }
    }
    Some(ProcMemoryStats {
        vm_rss_kb: vm_rss_kb?,
        threads: threads?,
    })
}

pub struct FtxrIpcClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    pub worker_threads: usize,
    pub worker_version: String,
}

impl FtxrIpcClient {
    pub fn spawn(
        bin_path: &Path,
        threads: Option<usize>,
        enable_telemetry_default: bool,
    ) -> Result<Self, IpcClientError> {
        let mut cmd = Command::new(bin_path);
        cmd.arg("--worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(n) = threads {
            cmd.arg("--threads").arg(n.to_string());
        }
        let mut child = cmd.spawn()?;
        let stdin = BufWriter::new(child.stdin.take().expect("stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let stderr = child.stderr.take().expect("stderr");
        // Drain stderr cleanly so pipe never stalls; report unexpected panic lines.
        std::thread::spawn(move || {
            let mut r = BufReader::new(stderr);
            let mut buf = String::new();
            let _ = r.read_to_string(&mut buf);
        });

        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            worker_threads: 0,
            worker_version: String::new(),
        };

        let hello_id = client.alloc_id("hello");
        let hello = serde_json::json!({
            "op": "hello",
            "id": hello_id,
            "protocol_version": FTXR_VERSION,
            "client": "ftxr-oracle",
            "threads": threads,
            "enable_telemetry_default": enable_telemetry_default,
        });
        client.send_json(&hello, 0, &[])?;
        let ready = client.recv_frame()?;
        let v: serde_json::Value = serde_json::from_slice(&ready.json)?;
        if v["op"] == "error" {
            return Err(IpcClientError::RemoteError {
                code: v["code"].as_str().unwrap_or("error").to_string(),
                message: v["message"].as_str().unwrap_or("").to_string(),
            });
        }
        if v["op"] != "ready" {
            return Err(IpcClientError::Protocol(format!("expected ready, got {v}")));
        }
        client.worker_threads = v["threads"].as_u64().unwrap_or(1) as usize;
        client.worker_version = v["version"].as_str().unwrap_or("").to_string();
        Ok(client)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn alloc_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_id);
        self.next_id += 1;
        id
    }

    pub fn send_json(
        &mut self,
        val: &serde_json::Value,
        flags: u16,
        payload: &[u8],
    ) -> Result<(), IpcClientError> {
        let json = serde_json::to_vec(val)?;
        self.send_raw_frame(flags, &json, payload)
    }

    pub fn send_raw_frame(
        &mut self,
        flags: u16,
        json: &[u8],
        payload: &[u8],
    ) -> Result<(), IpcClientError> {
        let eff_flags = flags | if payload.is_empty() { 0 } else { FLAG_HAS_BINARY };
        let mut hdr = [0u8; 16];
        hdr[0..4].copy_from_slice(&FTXR_MAGIC);
        hdr[4..6].copy_from_slice(&FTXR_VERSION.to_le_bytes());
        hdr[6..8].copy_from_slice(&eff_flags.to_le_bytes());
        hdr[8..12].copy_from_slice(&(json.len() as u32).to_le_bytes());
        hdr[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.stdin.write_all(&hdr)?;
        self.stdin.write_all(json)?;
        if !payload.is_empty() {
            self.stdin.write_all(payload)?;
        }
        self.stdin.flush()?;
        Ok(())
    }

    pub fn recv_frame(&mut self) -> Result<ClientFrame, IpcClientError> {
        let mut hdr = [0u8; 16];
        self.stdout.read_exact(&mut hdr)?;
        if hdr[0..4] != FTXR_MAGIC {
            return Err(IpcClientError::Protocol(format!(
                "worker stdout magic mismatch: {:?}",
                &hdr[0..4]
            )));
        }
        let version = u16::from_le_bytes([hdr[4], hdr[5]]);
        let flags = u16::from_le_bytes([hdr[6], hdr[7]]);
        let json_len = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
        let payload_len = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) as usize;
        let mut json = vec![0u8; json_len];
        self.stdout.read_exact(&mut json)?;
        let mut payload = vec![0u8; payload_len];
        if payload_len > 0 {
            self.stdout.read_exact(&mut payload)?;
        }
        Ok(ClientFrame {
            version,
            flags,
            json,
            payload,
        })
    }

    pub fn recv_value(&mut self) -> Result<(serde_json::Value, Vec<u8>), IpcClientError> {
        let f = self.recv_frame()?;
        let v: serde_json::Value = serde_json::from_slice(&f.json)?;
        Ok((v, f.payload))
    }

    pub fn ping(&mut self, seq: u64) -> Result<serde_json::Value, IpcClientError> {
        let id = self.alloc_id("ping");
        let req = serde_json::json!({ "op": "ping", "id": id, "seq": seq });
        self.send_json(&req, 0, &[])?;
        let (v, _) = self.recv_value()?;
        if v["op"] == "error" {
            return Err(IpcClientError::RemoteError {
                code: v["code"].as_str().unwrap_or("err").to_string(),
                message: v["message"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok(v)
    }

    pub fn session_reset(&mut self) -> Result<usize, IpcClientError> {
        let id = self.alloc_id("rst");
        self.send_json(&serde_json::json!({ "op": "session_reset", "id": id }), 0, &[])?;
        let (v, _) = self.recv_value()?;
        if v["op"] == "error" {
            return Err(IpcClientError::RemoteError {
                code: v["code"].as_str().unwrap_or("err").to_string(),
                message: v["message"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok(v["entries_cleared"].as_u64().unwrap_or(0) as usize)
    }

    pub fn set_context(&mut self, ctx: &serde_json::Value) -> Result<(), IpcClientError> {
        let mut req = ctx.clone();
        req["op"] = serde_json::Value::String("set_context".to_string());
        if req.get("id").is_none() {
            req["id"] = serde_json::Value::String(self.alloc_id("ctx"));
        }
        self.send_json(&req, 0, &[])?;
        let (v, _) = self.recv_value()?;
        if v["op"] == "error" {
            return Err(IpcClientError::RemoteError {
                code: v["code"].as_str().unwrap_or("err").to_string(),
                message: v["message"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode_slot_i16(
        &mut self,
        mode: Mode,
        depth: u8,
        samples_i16: &[i16],
        rx_freq_hz: f32,
        early_pass: bool,
        enable_telemetry: bool,
        timeout_ms: Option<u64>,
        slot_id: Option<&str>,
    ) -> Result<DecodeSlotOutcome, IpcClientError> {
        let id = self.alloc_id("dec");
        let mut payload = Vec::with_capacity(samples_i16.len() * 2);
        for &s in samples_i16 {
            payload.extend_from_slice(&s.to_le_bytes());
        }
        let mut req = serde_json::json!({
            "op": "decode_slot",
            "id": id,
            "mode": mode,
            "depth": depth,
            "rx_freq_hz": rx_freq_hz,
            "early_pass": early_pass,
            "sample_rate": 12000,
            "sample_format": "i16le",
            "enable_telemetry": enable_telemetry,
        });
        if let Some(sid) = slot_id {
            req["slot_id"] = serde_json::Value::String(sid.to_string());
        }
        if let Some(tmo) = timeout_ms {
            req["timeout_ms"] = serde_json::Value::Number(tmo.into());
        }

        let t0 = Instant::now();
        let mut first_event_ms: Option<f64> = None;
        self.send_json(&req, 0, &payload)?;

        let mut decodes = Vec::new();
        loop {
            let (v, _) = self.recv_value()?;
            match v["op"].as_str().unwrap_or("") {
                "decode_event" => {
                    if first_event_ms.is_none() {
                        first_event_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
                    }
                    let wd: WireDecode = serde_json::from_value(v["decode"].clone())?;
                    decodes.push(wd);
                }
                "decode_done" => {
                    let wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    return Ok(DecodeSlotOutcome {
                        id,
                        status: v["status"].as_str().unwrap_or("unknown").to_string(),
                        decoded_count: v["decoded_count"].as_u64().unwrap_or(0) as usize,
                        worker_elapsed_ms: v["elapsed_ms"].as_f64().unwrap_or(0.0),
                        wall_ms,
                        first_event_ms,
                        decodes,
                    });
                }
                "error" => {
                    return Err(IpcClientError::RemoteError {
                        code: v["code"].as_str().unwrap_or("err").to_string(),
                        message: v["message"].as_str().unwrap_or("").to_string(),
                    });
                }
                other => {
                    return Err(IpcClientError::Protocol(format!(
                        "unexpected op during decode_slot: {other}"
                    )));
                }
            }
        }
    }

    pub fn start_decode_slot_i16(
        &mut self,
        mode: Mode,
        depth: u8,
        samples_i16: &[i16],
        enable_telemetry: bool,
        timeout_ms: Option<u64>,
    ) -> Result<String, IpcClientError> {
        let id = self.alloc_id("dec");
        let mut payload = Vec::with_capacity(samples_i16.len() * 2);
        for &s in samples_i16 {
            payload.extend_from_slice(&s.to_le_bytes());
        }
        let mut req = serde_json::json!({
            "op": "decode_slot",
            "id": id,
            "mode": mode,
            "depth": depth,
            "sample_rate": 12000,
            "sample_format": "i16le",
            "enable_telemetry": enable_telemetry,
        });
        if let Some(tmo) = timeout_ms {
            req["timeout_ms"] = serde_json::Value::Number(tmo.into());
        }
        self.send_json(&req, 0, &payload)?;
        Ok(id)
    }

    pub fn send_encode_tx(
        &mut self,
        mode: Mode,
        message: &str,
        af_hz: f32,
        pad_to_slot: bool,
    ) -> Result<String, IpcClientError> {
        let id = self.alloc_id("tx");
        let req = serde_json::json!({
            "op": "encode_tx",
            "id": id,
            "mode": mode,
            "message": message,
            "af_hz": af_hz,
            "sample_rate": 12000,
            "sample_format": "f32le",
            "pad_to_slot": pad_to_slot,
        });
        self.send_json(&req, 0, &[])?;
        Ok(id)
    }

    #[allow(dead_code)]
    pub fn encode_tx_blocking(
        &mut self,
        mode: Mode,
        message: &str,
        af_hz: f32,
        pad_to_slot: bool,
    ) -> Result<(Vec<u8>, f64), IpcClientError> {
        let t0 = Instant::now();
        let _id = self.send_encode_tx(mode, message, af_hz, pad_to_slot)?;
        let (v, payload) = self.recv_value()?;
        let wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if v["op"] == "error" {
            return Err(IpcClientError::RemoteError {
                code: v["code"].as_str().unwrap_or("err").to_string(),
                message: v["message"].as_str().unwrap_or("").to_string(),
            });
        }
        if v["op"] != "encode_result" {
            return Err(IpcClientError::Protocol(format!("expected encode_result, got {v}")));
        }
        Ok((payload, wall_ms))
    }

    pub fn send_cancel(&mut self, target_id: Option<&str>) -> Result<String, IpcClientError> {
        let id = self.alloc_id("cancel");
        let mut req = serde_json::json!({ "op": "cancel", "id": id });
        if let Some(tid) = target_id {
            req["target_id"] = serde_json::Value::String(tid.to_string());
        }
        self.send_json(&req, 0, &[])?;
        Ok(id)
    }

    pub fn wait_until_idle(
        &mut self,
        deadline: Duration,
    ) -> Result<(usize, Option<String>, Vec<f64>), IpcClientError> {
        let start = Instant::now();
        let mut event_count = 0usize;
        let mut done_status = None;
        let mut tx_latencies = Vec::new();
        while start.elapsed() < deadline {
            let (v, _) = self.recv_value()?;
            match v["op"].as_str().unwrap_or("") {
                "decode_event" => event_count += 1,
                "encode_result" => {
                    tx_latencies.push(v["elapsed_ms"].as_f64().unwrap_or(0.0));
                }
                "cancel_ack" => {}
                "decode_done" => {
                    done_status = Some(v["status"].as_str().unwrap_or("").to_string());
                    break;
                }
                "error" => {
                    return Err(IpcClientError::RemoteError {
                        code: v["code"].as_str().unwrap_or("err").to_string(),
                        message: v["message"].as_str().unwrap_or("").to_string(),
                    });
                }
                _ => {}
            }
        }
        Ok((event_count, done_status, tx_latencies))
    }

    pub fn shutdown(mut self) -> Result<ExitStatus, IpcClientError> {
        let id = self.alloc_id("bye");
        self.send_json(&serde_json::json!({ "op": "shutdown", "id": id }), 0, &[])?;
        let (v, _) = self.recv_value()?;
        if v["op"] != "ok" {
            return Err(IpcClientError::Protocol(format!("expected shutdown ok, got {v}")));
        }
        drop(self.stdin);
        Ok(self.child.wait()?)
    }
}
