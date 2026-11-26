// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Persistent 4-role warm stdio IPC v1 `--worker` implementation:
//! 1. Reader loop (`stdin`)
//! 2. Isolated `TXTHR` OS thread (`encode_tx`, zero RX lock contention)
//! 3. `RXCOORD` OS thread + shared `DecoderEngine` Rayon worker pool (`decode_slot`)
//! 4. Shared `Condvar` `DEADLINE` watchdog + dedicated `WRITER` thread (`BufWriter<StdoutLock>`)

use std::collections::HashSet;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use crossbeam_channel as chan;
use ftxr_core::{
    DecodeConfig, DecoderEngine, FtxrError, ModulateConfig, Mode, QsoContext,
};

use super::framing::{
    read_frame, write_frame, FrameError, FLAG_BINARY_F32LE, FLAG_HAS_BINARY, FTXR_IPC_VERSION,
};
use super::protocol::{extract_loose_id, InboundOp, OutboundOp, WireDecode};
use crate::cli::SHORT_VERSION;
use crate::wav::{decode_raw_pcm_bytes, encode_raw_pcm_bytes, PcmSamples, SampleFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExit {
    Clean = 0,
    FrameDesync = 3,
    VersionMismatch = 4,
}

impl WorkerExit {
    pub const fn code(self) -> i32 {
        self as i32
    }
}

enum WriterMsg {
    Frame {
        flags: u16,
        json: Vec<u8>,
        payload: Vec<u8>,
    },
    Shutdown,
}

impl WriterMsg {
    fn json_only(op: &OutboundOp) -> Self {
        Self::Frame {
            flags: 0,
            json: serde_json::to_vec(op).expect("serialize OutboundOp"),
            payload: Vec::new(),
        }
    }

    fn with_payload(op: &OutboundOp, flags: u16, payload: Vec<u8>) -> Self {
        Self::Frame {
            flags,
            json: serde_json::to_vec(op).expect("serialize OutboundOp"),
            payload,
        }
    }
}

struct ActiveDecode {
    id: String,
    cancel: Arc<AtomicBool>,
}

struct DeadlineSlot {
    generation: u64,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
struct PersistentSession {
    qso: Option<QsoContext>,
    rx_freq_hz: Option<f32>,
    freq_tol_hz: Option<f32>,
}

struct SharedState {
    active_decode: Mutex<Option<ActiveDecode>>,
    cancelled_queued_ids: Mutex<HashSet<String>>,
    session: RwLock<PersistentSession>,
    deadline: Mutex<Option<DeadlineSlot>>,
    deadline_stop: AtomicBool,
    deadline_cv: Condvar,
    next_gen: AtomicU64,
}

impl SharedState {
    fn new() -> Self {
        Self {
            active_decode: Mutex::new(None),
            cancelled_queued_ids: Mutex::new(HashSet::new()),
            session: RwLock::new(PersistentSession::default()),
            deadline: Mutex::new(None),
            deadline_stop: AtomicBool::new(false),
            deadline_cv: Condvar::new(),
            next_gen: AtomicU64::new(1),
        }
    }

    fn set_deadline(
        &self,
        generation: u64,
        timeout_ms: u64,
        cancel: Arc<AtomicBool>,
        timed_out: Arc<AtomicBool>,
    ) {
        let mut guard = self.deadline.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(DeadlineSlot {
            generation,
            deadline: Instant::now() + Duration::from_millis(timeout_ms),
            cancel,
            timed_out,
        });
        self.deadline_cv.notify_all();
    }

    fn clear_deadline(&self, generation: u64) {
        let mut guard = self.deadline.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_some_and(|d| d.generation == generation) {
            *guard = None;
            self.deadline_cv.notify_all();
        }
    }

    fn cancel_target(&self, target_id: Option<&str>) -> bool {
        let mut cancelled = false;
        if let Some(ref active) = *self.active_decode.lock().unwrap_or_else(|e| e.into_inner()) {
            if target_id.is_none() || target_id == Some(active.id.as_str()) {
                active.cancel.store(true, Ordering::Relaxed);
                cancelled = true;
            }
        }
        if let Some(tid) = target_id {
            if !cancelled {
                self.cancelled_queued_ids
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(tid.to_string());
                cancelled = true;
            }
        }
        cancelled
    }

    fn active_decode_id(&self) -> Option<String> {
        self.active_decode
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|a| a.id.clone())
    }
}

struct RxJob {
    id: String,
    slot_id: Option<String>,
    pcm: PcmSamples,
    cfg: DecodeConfig,
    timeout_ms: Option<u64>,
}

struct TxJob {
    id: String,
    mode: Mode,
    message: String,
    af_hz: f32,
    sample_rate: u32,
    sample_format: SampleFormat,
    start_delay_ms: Option<u32>,
    offset_ms: u64,
    pad_to_slot: bool,
    amplitude: f32,
}

/// Runs the warm `--worker` loop over process `stdin` and `stdout`.
pub fn run_worker(cli_threads: Option<usize>) -> WorkerExit {
    run_worker_io(
        BufReader::new(io::stdin()),
        BufWriter::new(io::stdout()),
        cli_threads,
    )
}

/// Generic `Read`/`Write` `--worker` loop used by both `main` and in-process pipe integration tests.
pub fn run_worker_io<R: Read, W: Write + Send>(
    mut reader: R,
    mut writer: W,
    cli_threads: Option<usize>,
) -> WorkerExit {
    // 1. Strict synchronous handshake (`hello` MUST be frame #1).
    let hello_frame = match read_frame(&mut reader) {
        Ok(None) => return WorkerExit::Clean,
        Err(FrameError::Desync(msg)) => {
            tracing::error!("worker handshake frame desync: {msg}");
            return WorkerExit::FrameDesync;
        }
        Err(FrameError::Io(e)) => {
            tracing::error!("worker handshake I/O error: {e}");
            return WorkerExit::FrameDesync;
        }
        Ok(Some(raw)) => raw,
    };

    if hello_frame.version != FTXR_IPC_VERSION {
        let id = extract_loose_id(&hello_frame.json);
        let err = OutboundOp::Error {
            id,
            code: "version_mismatch",
            message: format!(
                "unsupported framing version {} (expected {})",
                hello_frame.version, FTXR_IPC_VERSION
            ),
            fatal: true,
        };
        let _ = send_direct_op(&mut writer, &err);
        return WorkerExit::VersionMismatch;
    }

    let (hello_id, req_threads, default_telemetry) = match serde_json::from_slice::<InboundOp>(
        &hello_frame.json,
    ) {
        Ok(InboundOp::Hello {
            id,
            protocol_version,
            client,
            client_name,
            client_version,
            threads,
            enable_telemetry_default,
        }) => {
            if protocol_version != FTXR_IPC_VERSION {
                let err = OutboundOp::Error {
                    id,
                    code: "version_mismatch",
                    message: format!(
                        "unsupported protocol_version {protocol_version} (expected {FTXR_IPC_VERSION})"
                    ),
                    fatal: true,
                };
                let _ = send_direct_op(&mut writer, &err);
                return WorkerExit::VersionMismatch;
            }
            tracing::info!(
                client = ?client.or(client_name),
                client_version = ?client_version,
                "IPC handshake complete"
            );
            (id, threads.or(cli_threads), enable_telemetry_default.unwrap_or(false))
        }
        Ok(other) => {
            let err = OutboundOp::Error {
                id: extract_loose_id(&hello_frame.json),
                code: "handshake_required",
                message: format!("first IPC frame must be 'hello', got {other:?}"),
                fatal: true,
            };
            let _ = send_direct_op(&mut writer, &err);
            return WorkerExit::VersionMismatch;
        }
        Err(e) => {
            let err = OutboundOp::Error {
                id: extract_loose_id(&hello_frame.json),
                code: "handshake_error",
                message: format!("invalid hello JSON: {e}"),
                fatal: true,
            };
            let _ = send_direct_op(&mut writer, &err);
            return WorkerExit::VersionMismatch;
        }
    };

    let engine = match DecoderEngine::with_threads(req_threads) {
        Ok(eng) => Arc::new(eng),
        Err(e) => {
            let err = OutboundOp::Error {
                id: hello_id,
                code: "invalid_config",
                message: format!("failed to initialize engine: {e}"),
                fatal: true,
            };
            let _ = send_direct_op(&mut writer, &err);
            return WorkerExit::VersionMismatch;
        }
    };
    let effective_threads = engine.num_threads();

    let ready = OutboundOp::Ready {
        id: hello_id,
        protocol_version: FTXR_IPC_VERSION,
        version: SHORT_VERSION,
        threads: effective_threads,
        modes: ["ft8", "ft4"],
        capabilities: ["streaming_decode", "cancel", "telemetry", "concurrent_encode"],
    };
    if send_direct_op(&mut writer, &ready).is_err() {
        return WorkerExit::Clean;
    }

    // 2. Spawn 4 persistent isolated roles cleanly (`WRITER`, `DEADLINE`, `TXTHR`, `RXCOORD`).
    let state = Arc::new(SharedState::new());
    let (out_tx, out_rx) = chan::unbounded::<WriterMsg>();
    let (tx_tx, tx_rx) = chan::bounded::<TxJob>(32);
    let (rx_tx, rx_rx) = chan::bounded::<RxJob>(8);

    std::thread::scope(|scope| {
        let writer_handle = std::thread::Builder::new()
            .name("ftxr-writer".to_string())
            .spawn_scoped(scope, move || {
            for msg in out_rx {
                match msg {
                    WriterMsg::Frame {
                        flags,
                        json,
                        payload,
                    } => {
                        if write_frame(&mut writer, flags, &json, &payload).is_err() {
                            break;
                        }
                    }
                    WriterMsg::Shutdown => break,
                }
            }
            let _ = writer.flush();
        })
        .expect("spawn writer thread");

    let deadline_state = Arc::clone(&state);
    let deadline_handle = std::thread::Builder::new()
        .name("ftxr-deadline".to_string())
        .spawn_scoped(scope, move || {
            let mut guard = deadline_state
                .deadline
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            loop {
                if deadline_state.deadline_stop.load(Ordering::Relaxed) {
                    break;
                }
                match guard.as_ref() {
                    None => {
                        guard = deadline_state
                            .deadline_cv
                            .wait(guard)
                            .unwrap_or_else(|e| e.into_inner());
                    }
                    Some(slot) => {
                        let now = Instant::now();
                        if now >= slot.deadline {
                            slot.timed_out.store(true, Ordering::Relaxed);
                            slot.cancel.store(true, Ordering::Relaxed);
                            *guard = None;
                        } else {
                            let wait_dur = slot.deadline.saturating_duration_since(now);
                            let (next_guard, _) = deadline_state
                                .deadline_cv
                                .wait_timeout(guard, wait_dur)
                                .unwrap_or_else(|e| e.into_inner());
                            guard = next_guard;
                        }
                    }
                }
            }
        })
        .expect("spawn deadline watchdog thread");

    let tx_engine = Arc::clone(&engine);
    let tx_out = out_tx.clone();
    let tx_handle = std::thread::Builder::new()
        .name("ftxr-tx".to_string())
        .spawn_scoped(scope, move || {
            for job in tx_rx {
                let started = Instant::now();
                if let Err(err_op) = handle_tx_job(&tx_engine, &job, started, &tx_out) {
                    let _ = tx_out.send(WriterMsg::json_only(&err_op));
                }
            }
        })
        .expect("spawn TXTHR thread");

    let rx_engine = Arc::clone(&engine);
    let rx_state = Arc::clone(&state);
    let rx_out = out_tx.clone();
    let rx_handle = std::thread::Builder::new()
        .name("ftxr-rxcoord".to_string())
        .spawn_scoped(scope, move || {
            for job in rx_rx {
                let started = Instant::now();
                let pre_cancelled = rx_state
                    .cancelled_queued_ids
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&job.id);
                if pre_cancelled {
                    let done = OutboundOp::DecodeDone {
                        id: job.id,
                        slot_id: job.slot_id,
                        status: "cancelled",
                        decoded_count: 0,
                        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    };
                    let _ = rx_out.send(WriterMsg::json_only(&done));
                    continue;
                }

                let generation = rx_state.next_gen.fetch_add(1, Ordering::Relaxed);
                let cancel = Arc::new(AtomicBool::new(false));
                let timed_out = Arc::new(AtomicBool::new(false));
                {
                    let mut active = rx_state
                        .active_decode
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *active = Some(ActiveDecode {
                        id: job.id.clone(),
                        cancel: Arc::clone(&cancel),
                    });
                }
                if let Some(ms) = job.timeout_ms {
                    rx_state.set_deadline(
                        generation,
                        ms,
                        Arc::clone(&cancel),
                        Arc::clone(&timed_out),
                    );
                }

                let mut decoded_count = 0usize;
                let stream_id = job.id.clone();
                let stream_slot_id = job.slot_id.clone();
                let stream_out = &rx_out;

                let res = match &job.pcm {
                    PcmSamples::I16(samples) => rx_engine.decode_i16_streaming(
                        samples,
                        &job.cfg,
                        &cancel,
                        |dec| {
                            decoded_count += 1;
                            let ev = OutboundOp::DecodeEvent {
                                id: stream_id.clone(),
                                slot_id: stream_slot_id.clone(),
                                decode: WireDecode::from(dec),
                            };
                            let _ = stream_out.send(WriterMsg::json_only(&ev));
                        },
                    ),
                    PcmSamples::F32(samples) => rx_engine.decode_streaming(
                        samples,
                        &job.cfg,
                        &cancel,
                        |dec| {
                            decoded_count += 1;
                            let ev = OutboundOp::DecodeEvent {
                                id: stream_id.clone(),
                                slot_id: stream_slot_id.clone(),
                                decode: WireDecode::from(dec),
                            };
                            let _ = stream_out.send(WriterMsg::json_only(&ev));
                        },
                    ),
                };

                rx_state.clear_deadline(generation);
                {
                    let mut active = rx_state
                        .active_decode
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *active = None;
                }

                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                match res {
                    Ok(_) => {
                        let done = OutboundOp::DecodeDone {
                            id: job.id,
                            slot_id: job.slot_id,
                            status: "completed",
                            decoded_count,
                            elapsed_ms,
                        };
                        let _ = rx_out.send(WriterMsg::json_only(&done));
                    }
                    Err(FtxrError::Cancelled) => {
                        let status = if timed_out.load(Ordering::Relaxed) {
                            "timeout"
                        } else {
                            "cancelled"
                        };
                        let done = OutboundOp::DecodeDone {
                            id: job.id,
                            slot_id: job.slot_id,
                            status,
                            decoded_count,
                            elapsed_ms,
                        };
                        let _ = rx_out.send(WriterMsg::json_only(&done));
                    }
                    Err(e) => {
                        let err = OutboundOp::Error {
                            id: Some(job.id),
                            code: "decode_failed",
                            message: e.to_string(),
                            fatal: false,
                        };
                        let _ = rx_out.send(WriterMsg::json_only(&err));
                    }
                }
            }
        })
        .expect("spawn RXCOORD thread");

    // 3. Reader loop on `stdin`.
    let mut exit_status = WorkerExit::Clean;
    loop {
        let frame = match read_frame(&mut reader) {
            Ok(None) => break,
            Err(FrameError::Desync(msg)) => {
                tracing::error!("worker stdin frame desync: {msg}");
                exit_status = WorkerExit::FrameDesync;
                break;
            }
            Err(FrameError::Io(e)) => {
                tracing::error!("worker stdin I/O error: {e}");
                exit_status = WorkerExit::FrameDesync;
                break;
            }
            Ok(Some(f)) => f,
        };

        if frame.version != FTXR_IPC_VERSION {
            let err = OutboundOp::Error {
                id: extract_loose_id(&frame.json),
                code: "version_mismatch",
                message: format!(
                    "frame version {} != expected {}",
                    frame.version, FTXR_IPC_VERSION
                ),
                fatal: false,
            };
            let _ = out_tx.send(WriterMsg::json_only(&err));
            continue;
        }

        let op: InboundOp = match serde_json::from_slice(&frame.json) {
            Ok(op) => op,
            Err(e) => {
                let err = OutboundOp::Error {
                    id: extract_loose_id(&frame.json),
                    code: "invalid_json",
                    message: format!("malformed JSON op: {e}"),
                    fatal: false,
                };
                let _ = out_tx.send(WriterMsg::json_only(&err));
                continue;
            }
        };

        match op {
            InboundOp::Hello { id, .. } => {
                let err = OutboundOp::Error {
                    id,
                    code: "duplicate_hello",
                    message: "hello already completed".to_string(),
                    fatal: false,
                };
                let _ = out_tx.send(WriterMsg::json_only(&err));
            }
            InboundOp::SetContext { id, ctx } => {
                let mut sess = state.session.write().unwrap_or_else(|e| e.into_inner());
                let default_rx = sess.rx_freq_hz.unwrap_or(1500.0);
                match ctx.apply_to(sess.qso.as_ref(), Mode::Ft8, default_rx) {
                    Ok((new_qso, opt_rx, opt_tol)) => {
                        sess.qso = new_qso;
                        if opt_rx.is_some() {
                            sess.rx_freq_hz = opt_rx;
                        }
                        if opt_tol.is_some() {
                            sess.freq_tol_hz = opt_tol;
                        }
                        let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Ok {
                            id,
                            entries_cleared: None,
                        }));
                    }
                    Err(msg) => {
                        let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                            id,
                            code: "invalid_context",
                            message: msg,
                            fatal: false,
                        }));
                    }
                }
            }
            InboundOp::SessionReset { id } => {
                let entries_cleared = engine.reset_session();
                let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Ok {
                    id,
                    entries_cleared: Some(entries_cleared),
                }));
            }
            InboundOp::Cancel { id, target_id } => {
                let cancelled = state.cancel_target(target_id.as_deref());
                let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::CancelAck {
                    id,
                    target_id,
                    cancelled,
                }));
            }
            InboundOp::Ping { id, seq } => {
                let active_decode_id = state.active_decode_id();
                let hash_entries = engine.hash_table().len();
                let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Pong {
                    id,
                    seq,
                    active_decode_id,
                    hash_entries,
                }));
            }
            InboundOp::Shutdown { id } => {
                state.cancel_target(None);
                let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Ok {
                    id,
                    entries_cleared: None,
                }));
                break;
            }
            InboundOp::EncodeTx {
                id,
                mode,
                message,
                af_hz,
                sample_rate,
                sample_format,
                start_delay_ms,
                offset_ms,
                pad_to_slot,
                amplitude,
            } => {
                let fmt = match sample_format.as_deref() {
                    Some(s) => match SampleFormat::parse_str(s) {
                        Some(f) => f,
                        None => {
                            let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                                id: Some(id),
                                code: "invalid_sample_format",
                                message: format!("unsupported sample_format '{s}'"),
                                fatal: false,
                            }));
                            continue;
                        }
                    },
                    None => SampleFormat::F32Le,
                };
                let job = TxJob {
                    id,
                    mode,
                    message,
                    af_hz,
                    sample_rate: sample_rate.unwrap_or(12_000),
                    sample_format: fmt,
                    start_delay_ms,
                    offset_ms: offset_ms.unwrap_or(0),
                    pad_to_slot: pad_to_slot.unwrap_or(false),
                    amplitude: amplitude.unwrap_or(1.0),
                };
                if tx_tx.send(job).is_err() {
                    break;
                }
            }
            InboundOp::DecodeSlot {
                id,
                slot_id,
                mode,
                depth,
                freq_min_hz,
                freq_max_hz,
                rx_freq_hz,
                freq_tol_hz,
                sample_rate,
                sample_format,
                early_pass,
                qso_first,
                enable_telemetry,
                timeout_ms,
                utc_hhmmss: _,
                qso_override,
            } => {
                let sr = sample_rate.unwrap_or(12_000);
                if sr != 12_000 {
                    let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                        id: Some(id),
                        code: "invalid_sample_rate",
                        message: format!("unsupported sample_rate {sr} (expected 12000)"),
                        fatal: false,
                    }));
                    continue;
                }
                let fmt = match sample_format.as_deref() {
                    Some(s) => match SampleFormat::parse_str(s) {
                        Some(f) => f,
                        None => {
                            let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                                id: Some(id),
                                code: "invalid_sample_format",
                                message: format!("unsupported sample_format '{s}'"),
                                fatal: false,
                            }));
                            continue;
                        }
                    },
                    None => {
                        if (frame.flags & FLAG_BINARY_F32LE) != 0 {
                            SampleFormat::F32Le
                        } else {
                            SampleFormat::I16Le
                        }
                    }
                };

                let pcm = match decode_raw_pcm_bytes(&frame.payload, fmt) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                            id: Some(id),
                            code: "invalid_payload",
                            message: e.to_string(),
                            fatal: false,
                        }));
                        continue;
                    }
                };

                let sess_snap = state.session.read().unwrap_or_else(|e| e.into_inner()).clone();
                let mut cfg = DecodeConfig::for_mode(mode);
                if let Some(d) = depth {
                    cfg.depth = d;
                }
                if let Some(fmin) = freq_min_hz {
                    cfg.freq_min_hz = fmin;
                }
                if let Some(fmax) = freq_max_hz {
                    cfg.freq_max_hz = fmax;
                }
                cfg.rx_freq_hz = rx_freq_hz.or(sess_snap.rx_freq_hz).unwrap_or(cfg.rx_freq_hz);
                cfg.freq_tol_hz = freq_tol_hz
                    .or(sess_snap.freq_tol_hz)
                    .unwrap_or(cfg.freq_tol_hz);
                cfg.early_pass = early_pass.unwrap_or(false);
                cfg.qso_first = qso_first.unwrap_or(true);
                cfg.enable_telemetry = enable_telemetry.unwrap_or(default_telemetry);

                let (merged_qso, ov_rx, ov_tol) = if let Some(ref ov) = qso_override {
                    match ov.apply_to(sess_snap.qso.as_ref(), mode, cfg.rx_freq_hz) {
                        Ok(res) => res,
                        Err(msg) => {
                            let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                                id: Some(id),
                                code: "invalid_context",
                                message: msg,
                                fatal: false,
                            }));
                            continue;
                        }
                    }
                } else {
                    (sess_snap.qso, None, None)
                };
                if let Some(rx) = ov_rx {
                    cfg.rx_freq_hz = rx;
                }
                if let Some(tol) = ov_tol {
                    cfg.freq_tol_hz = tol;
                }
                cfg.qso = merged_qso.map(|mut q| {
                    q.ap_width_hz = mode.default_ap_width_hz();
                    q
                });

                let job = RxJob {
                    id,
                    slot_id,
                    pcm,
                    cfg,
                    timeout_ms,
                };
                if let Err(chan::TrySendError::Full(rejected)) = rx_tx.try_send(job) {
                    let _ = out_tx.send(WriterMsg::json_only(&OutboundOp::Error {
                        id: Some(rejected.id),
                        code: "busy",
                        message: "RX worker queue full".to_string(),
                        fatal: false,
                    }));
                }
            }
        }
    }

    // 4. Deterministic cleanup: abort active/watchdog, drain queues, join all 4 helper threads.
    if exit_status == WorkerExit::FrameDesync {
        state.cancel_target(None);
    }
    drop(tx_tx);
    drop(rx_tx);
    let _ = tx_handle.join();
    let _ = rx_handle.join();

    state.deadline_stop.store(true, Ordering::Relaxed);
    state.deadline_cv.notify_all();
    let _ = deadline_handle.join();

    let _ = out_tx.send(WriterMsg::Shutdown);
    drop(out_tx);
    let _ = writer_handle.join();

    exit_status
    })
}

fn send_direct_op<W: Write>(writer: &mut W, op: &OutboundOp) -> io::Result<()> {
    let json = serde_json::to_vec(op).expect("serialize OutboundOp");
    write_frame(writer, 0, &json, &[])
}

fn handle_tx_job(
    engine: &DecoderEngine,
    job: &TxJob,
    started: Instant,
    out_tx: &chan::Sender<WriterMsg>,
) -> Result<(), Box<OutboundOp>> {
    if !job.amplitude.is_finite() || job.amplitude <= 0.0 || job.amplitude > 1.0 {
        return Err(Box::new(OutboundOp::Error {
            id: Some(job.id.clone()),
            code: "invalid_amplitude",
            message: format!("amplitude {} must be in (0.0, 1.0]", job.amplitude),
            fatal: false,
        }));
    }
    let mut mcfg = ModulateConfig::default_for(job.mode, job.af_hz, job.sample_rate);
    if let Some(delay) = job.start_delay_ms {
        mcfg.start_delay_ms = delay;
    }
    mcfg.offset_ms = job.offset_ms;

    let mut samples = engine.modulate(&job.message, &mcfg).map_err(|e| {
        Box::new(OutboundOp::Error {
            id: Some(job.id.clone()),
            code: "encode_failed",
            message: e.to_string(),
            fatal: false,
        })
    })?;

    if job.pad_to_slot {
        let target_len = (job.mode.slot_duration_ms() as usize * job.sample_rate as usize) / 1000;
        if samples.len() < target_len {
            samples.resize(target_len, 0.0);
        }
    }
    if (job.amplitude - 1.0).abs() > 1e-6 {
        for s in &mut samples {
            *s *= job.amplitude;
        }
    }

    let sample_count = samples.len();
    let duration_ms = (sample_count as f64) * 1000.0 / f64::from(job.sample_rate);
    let payload = encode_raw_pcm_bytes(&samples, job.sample_format);
    let flags = FLAG_HAS_BINARY
        | if job.sample_format == SampleFormat::F32Le {
            FLAG_BINARY_F32LE
        } else {
            0
        };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let res = OutboundOp::EncodeResult {
        id: job.id.clone(),
        mode: job.mode,
        message: job.message.clone(),
        af_hz: job.af_hz,
        sample_rate: job.sample_rate,
        sample_format: job.sample_format.as_str(),
        sample_count,
        duration_ms,
        elapsed_ms,
    };
    let _ = out_tx.send(WriterMsg::with_payload(&res, flags, payload));
    Ok(())
}
