// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

#![forbid(unsafe_code)]

//! Standalone FT8/FT4 modem CLI and warm stdio IPC v1 worker daemon (`ftxr-modem`).

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use clap::Parser;
use serde::Serialize;

use ftxr_core::{
    ContestMode, DecodeConfig, DecoderEngine, ModulateConfig, Mode, QsoContext,
};

pub mod cli;
pub mod format;
pub mod ipc;
pub mod wav;

use cli::{Cli, Commands, DecodeArgs, EncodeArgs, LONG_VERSION, SHORT_VERSION};
use format::{
    format_decode_finished, format_jt9_line, resolve_utc_tag, JsonDecodeItem, OutputFormat,
};
use wav::{
    decode_raw_pcm_bytes, read_raw_pcm_file, read_wav_12k, write_raw_pcm_file, write_wav_f32,
    write_wav_f32_as_i16, PcmSamples, SampleFormat, EXPECTED_SAMPLE_RATE_HZ,
};

#[derive(Serialize)]
struct VersionJson {
    name: &'static str,
    version: &'static str,
    ipc_protocol_version: u16,
    license: &'static str,
}

#[derive(Serialize)]
struct DecodeSummaryJson<'a> {
    mode: Mode,
    file: String,
    utc: String,
    decoded_count: usize,
    elapsed_ms: f64,
    decodes: Vec<JsonDecodeItem<'a>>,
}

#[derive(Serialize)]
struct EncodeMetaJson {
    mode: Mode,
    message: String,
    af_hz: f32,
    sample_rate: u32,
    sample_format: &'static str,
    sample_count: usize,
    duration_ms: f64,
    out: String,
}

fn init_stderr_tracing(level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.version {
        if cli.json {
            let v = VersionJson {
                name: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
                ipc_protocol_version: ipc::FTXR_IPC_VERSION,
                license: "GPL-3.0-only",
            };
            println!("{}", serde_json::to_string(&v).expect("version json"));
        } else {
            println!("ftxr-modem {SHORT_VERSION}");
        }
        return ExitCode::SUCCESS;
    }

    if cli.license {
        println!("{LONG_VERSION}");
        return ExitCode::SUCCESS;
    }

    init_stderr_tracing(&cli.log_level);

    if cli.worker {
        if cli.command.is_some() {
            eprintln!("error: --worker cannot be combined with decode/encode subcommands");
            return ExitCode::from(2);
        }
        let exit = ipc::run_worker(cli.threads);
        return ExitCode::from(exit.code() as u8);
    }

    match cli.command {
        Some(Commands::Decode(args)) => match run_decode_cli(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err((code, msg)) => {
                eprintln!("error: {msg}");
                ExitCode::from(code)
            }
        },
        Some(Commands::Encode(args)) => match run_encode_cli(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err((code, msg)) => {
                eprintln!("error: {msg}");
                ExitCode::from(code)
            }
        },
        None => {
            eprintln!(
                "Usage: ftxr-modem [--worker | decode ... | encode ... | --version | --license] (try --help)"
            );
            ExitCode::from(2)
        }
    }
}

fn build_qso_context(args: &DecodeArgs, mode: Mode) -> Option<QsoContext> {
    let contest: ContestMode = args.contest.into();
    let has_ctx = args.my_call.is_some()
        || args.his_call.is_some()
        || args.his_grid.is_some()
        || args.qso_progress.is_some()
        || contest != ContestMode::None
        || args.no_ap
        || args.ap_cq_only;
    if !has_ctx {
        return None;
    }
    Some(QsoContext {
        my_call: args.my_call.clone(),
        his_call: args.his_call.clone(),
        his_grid: args.his_grid.clone(),
        qso_progress: args.qso_progress.unwrap_or(0),
        contest,
        tx_freq_hz: args.rx_freq_hz,
        rx_freq_hz: args.rx_freq_hz,
        ap_enabled: !args.no_ap,
        ap_cq_only: args.ap_cq_only,
        ap_width_hz: mode.default_ap_width_hz(),
    })
}

fn read_decode_input(path: &std::path::Path, raw_fmt: Option<cli::CliRawFormat>) -> Result<PcmSamples, (u8, String)> {
    match raw_fmt {
        Some(fmt) => {
            let sample_fmt: SampleFormat = fmt.into();
            if path.as_os_str() == "-" {
                let mut bytes = Vec::new();
                io::stdin()
                    .read_to_end(&mut bytes)
                    .map_err(|e| (1, format!("reading stdin raw PCM: {e}")))?;
                decode_raw_pcm_bytes(&bytes, sample_fmt).map_err(|e| (2, e.to_string()))
            } else {
                read_raw_pcm_file(path, sample_fmt).map_err(|e| (1, e.to_string()))
            }
        }
        None => {
            if path.as_os_str() == "-" {
                return Err((2, "reading WAV from '-' requires --raw-pcm <i16le|f32le>".to_string()));
            }
            read_wav_12k(path).map_err(|e| (1, format!("{}: {e}", path.display())))
        }
    }
}

fn run_decode_cli(args: &DecodeArgs) -> Result<(), (u8, String)> {
    let mode: Mode = args.mode.into();
    let engine = DecoderEngine::with_threads(args.threads)
        .map_err(|e| (2, e.to_string()))?;

    let mut cfg = DecodeConfig::for_mode(mode);
    cfg.depth = args.depth;
    cfg.freq_min_hz = args.freq_min_hz;
    cfg.freq_max_hz = args.freq_max_hz;
    cfg.rx_freq_hz = args.rx_freq_hz;
    cfg.freq_tol_hz = args.freq_tol_hz;
    cfg.enable_telemetry = args.telemetry;
    cfg.early_pass = args.early_pass;
    cfg.qso = build_qso_context(args, mode);

    let cancel = AtomicBool::new(false);
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    for (idx, path) in args.files.iter().enumerate() {
        if idx > 0 && args.reset_between_files {
            engine.reset_session();
        }
        let pcm = read_decode_input(path, args.raw_pcm)?;
        let utc = resolve_utc_tag(
            args.utc.as_deref(),
            if path.as_os_str() == "-" { None } else { Some(path.as_path()) },
        );
        let started = Instant::now();
        let mut streamed_count = 0usize;

        let decodes = match args.format {
            OutputFormat::Jt9 => {
                let res = match &pcm {
                    PcmSamples::I16(samples) => {
                        engine.decode_i16_streaming(samples, &cfg, &cancel, |dec| {
                            streamed_count += 1;
                            let _ = writeln!(out, "{}", format_jt9_line(&utc, &dec));
                            let _ = out.flush();
                        })
                    }
                    PcmSamples::F32(samples) => {
                        engine.decode_streaming(samples, &cfg, &cancel, |dec| {
                            streamed_count += 1;
                            let _ = writeln!(out, "{}", format_jt9_line(&utc, &dec));
                            let _ = out.flush();
                        })
                    }
                }
                .map_err(|e| (1, e.to_string()))?;
                let _ = writeln!(out, "{}", format_decode_finished(0, res.len()));
                let _ = out.flush();
                res
            }
            OutputFormat::Jsonl => {
                let res = match &pcm {
                    PcmSamples::I16(samples) => {
                        engine.decode_i16_streaming(samples, &cfg, &cancel, |dec| {
                            streamed_count += 1;
                            let item = JsonDecodeItem::from_decoded(&utc, &dec);
                            if let Ok(line) = serde_json::to_string(&item) {
                                let _ = writeln!(out, "{line}");
                                let _ = out.flush();
                            }
                        })
                    }
                    PcmSamples::F32(samples) => {
                        engine.decode_streaming(samples, &cfg, &cancel, |dec| {
                            streamed_count += 1;
                            let item = JsonDecodeItem::from_decoded(&utc, &dec);
                            if let Ok(line) = serde_json::to_string(&item) {
                                let _ = writeln!(out, "{line}");
                                let _ = out.flush();
                            }
                        })
                    }
                }
                .map_err(|e| (1, e.to_string()))?;
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let footer = serde_json::json!({
                    "event": "decode_finished",
                    "file": path.display().to_string(),
                    "decoded_count": res.len(),
                    "elapsed_ms": elapsed_ms,
                });
                let _ = writeln!(out, "{footer}");
                let _ = out.flush();
                res
            }
            OutputFormat::Json => {
                let res = match &pcm {
                    PcmSamples::I16(samples) => engine.decode_i16(samples, &cfg),
                    PcmSamples::F32(samples) => engine.decode(samples, &cfg),
                }
                .map_err(|e| (1, e.to_string()))?;
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let items: Vec<JsonDecodeItem<'_>> = res
                    .iter()
                    .map(|d| JsonDecodeItem::from_decoded(&utc, d))
                    .collect();
                let summary = DecodeSummaryJson {
                    mode,
                    file: path.display().to_string(),
                    utc: utc.clone(),
                    decoded_count: res.len(),
                    elapsed_ms,
                    decodes: items,
                };
                let json = serde_json::to_string_pretty(&summary)
                    .map_err(|e| (1, e.to_string()))?;
                let _ = writeln!(out, "{json}");
                let _ = out.flush();
                res
            }
        };

        if !args.quiet {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            tracing::info!(
                file = %path.display(),
                decodes = decodes.len(),
                elapsed_ms,
                "decoded file"
            );
        }
    }

    Ok(())
}

fn run_encode_cli(args: &EncodeArgs) -> Result<(), (u8, String)> {
    if !args.amplitude.is_finite() || args.amplitude <= 0.0 || args.amplitude > 1.0 {
        return Err((2, format!("amplitude {} must be in (0.0, 1.0]", args.amplitude)));
    }
    let mode: Mode = args.mode.into();
    let engine = DecoderEngine::with_threads(Some(1)).map_err(|e| (2, e.to_string()))?;
    let mut mcfg = ModulateConfig::default_for(mode, args.af_hz, args.sample_rate);
    if let Some(delay) = args.start_delay_ms {
        mcfg.start_delay_ms = delay;
    }
    mcfg.offset_ms = args.offset_ms;

    let mut samples = engine
        .modulate(&args.message, &mcfg)
        .map_err(|e| (2, e.to_string()))?;

    if args.full_slot {
        let target_len = (mode.slot_duration_ms() as usize * args.sample_rate as usize) / 1000;
        if samples.len() < target_len {
            samples.resize(target_len, 0.0);
        }
    }
    if (args.amplitude - 1.0).abs() > 1e-6 {
        for s in &mut samples {
            *s *= args.amplitude;
        }
    }

    let fmt = match args.sample_format {
        Some(f) => f.into(),
        None if args.raw_pcm => SampleFormat::F32Le,
        None => SampleFormat::I16Le,
    };

    if args.raw_pcm {
        write_raw_pcm_file(&args.out, &samples, fmt).map_err(|e| (1, e.to_string()))?;
    } else {
        match fmt {
            SampleFormat::I16Le => write_wav_f32_as_i16(&args.out, args.sample_rate, &samples),
            SampleFormat::F32Le => write_wav_f32(&args.out, args.sample_rate, &samples),
        }
        .map_err(|e| (1, e.to_string()))?;
    }

    if args.json_meta {
        let meta = EncodeMetaJson {
            mode,
            message: args.message.clone(),
            af_hz: args.af_hz,
            sample_rate: args.sample_rate,
            sample_format: fmt.as_str(),
            sample_count: samples.len(),
            duration_ms: (samples.len() as f64) * 1000.0 / f64::from(args.sample_rate),
            out: args.out.display().to_string(),
        };
        println!("{}", serde_json::to_string(&meta).expect("encode meta json"));
    }

    let _ = EXPECTED_SAMPLE_RATE_HZ;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn long_version_contains_gplv3_and_wsjtx_attribution() {
        assert!(LONG_VERSION.contains("GPLv3"));
        assert!(LONG_VERSION.contains("WSJT-X"));
        assert!(LONG_VERSION.contains("NO WARRANTY"));
    }

    #[test]
    fn cli_parses_worker_decode_and_encode() {
        let w = Cli::try_parse_from(["ftxr-modem", "--worker"]).unwrap();
        assert!(w.worker);

        let d = Cli::try_parse_from([
            "ftxr-modem",
            "decode",
            "--mode",
            "ft8",
            "--depth",
            "3",
            "--telemetry",
            "--format",
            "json",
            "sample.wav",
        ])
        .unwrap();
        match d.command {
            Some(Commands::Decode(da)) => {
                assert!(matches!(da.mode, cli::CliMode::Ft8));
                assert_eq!(da.depth, 3);
                assert!(da.telemetry);
                assert_eq!(da.format, OutputFormat::Json);
                assert_eq!(da.files, vec![std::path::PathBuf::from("sample.wav")]);
            }
            _ => panic!("expected Decode"),
        }

        let e = Cli::try_parse_from([
            "ftxr-modem",
            "encode",
            "--mode",
            "ft4",
            "--af",
            "1500",
            "--sr",
            "12000",
            "--out",
            "out.wav",
            "CQ N0AAA EM10",
        ])
        .unwrap();
        match e.command {
            Some(Commands::Encode(ea)) => {
                assert!(matches!(ea.mode, cli::CliMode::Ft4));
                assert_eq!(ea.af_hz, 1500.0);
                assert_eq!(ea.sample_rate, 12_000);
                assert_eq!(ea.message, "CQ N0AAA EM10");
            }
            _ => panic!("expected Encode"),
        }
    }

    #[test]
    fn encode_and_decode_roundtrip_ft8_and_ft4_via_wav_and_raw_pcm() {
        let dir = tempfile::tempdir().unwrap();
        for (cli_mode, mode, msg) in [
            (cli::CliMode::Ft8, Mode::Ft8, "CQ N0AAA EM10"),
            (cli::CliMode::Ft4, Mode::Ft4, "CQ K1ABC FN42"),
        ] {
            let wav_path = dir.path().join(format!("{:?}_000001.wav", mode));
            let enc = EncodeArgs {
                mode: cli_mode,
                af_hz: 1500.0,
                sample_rate: 12_000,
                start_delay_ms: None,
                offset_ms: 0,
                full_slot: true,
                amplitude: 0.8,
                sample_format: None,
                raw_pcm: false,
                json_meta: false,
                out: wav_path.clone(),
                message: msg.to_string(),
            };
            run_encode_cli(&enc).unwrap();
            let engine = DecoderEngine::with_threads(Some(2)).unwrap();
            let PcmSamples::I16(i16s) = read_wav_12k(&wav_path).unwrap() else {
                panic!("expected i16");
            };
            let mut cfg = DecodeConfig::for_mode(mode);
            cfg.enable_telemetry = true;
            let decs = engine.decode_i16(&i16s, &cfg).unwrap();
            assert!(
                decs.iter().any(|d| d.message == msg),
                "mode={mode:?} decs={decs:?}"
            );
            assert!(decs[0].telemetry.is_some());
        }
    }

    #[test]
    fn worker_in_process_ipc_handshake_encode_decode_cancel_reset_and_exits() {
        use ipc::framing::{read_frame, write_frame, FLAG_BINARY_F32LE};

        // 1. Version mismatch exit code 4
        let mut bad_ver = Vec::new();
        write_frame(
            &mut bad_ver,
            0,
            br#"{"op":"hello","id":"h1","protocol_version":99}"#,
            &[],
        )
        .unwrap();
        let mut out_buf = Vec::new();
        let exit_bad = ipc::worker::run_worker_io(Cursor::new(bad_ver), &mut out_buf, Some(1));
        assert_eq!(exit_bad, ipc::WorkerExit::VersionMismatch);
        let err_frame = read_frame(&mut Cursor::new(&out_buf)).unwrap().unwrap();
        let err_val: serde_json::Value = serde_json::from_slice(&err_frame.json).unwrap();
        assert_eq!(err_val["op"], "error");
        assert_eq!(err_val["code"], "version_mismatch");
        assert_eq!(err_val["fatal"], true);

        // 2. Bad magic desync exit code 3 after handshake
        let mut desync = Vec::new();
        write_frame(
            &mut desync,
            0,
            br#"{"op":"hello","id":"h2","protocol_version":1,"threads":1}"#,
            &[],
        )
        .unwrap();
        desync.extend_from_slice(b"NOPE000000000000");
        let mut out_desync = Vec::new();
        let exit_desync = ipc::worker::run_worker_io(Cursor::new(desync), &mut out_desync, None);
        assert_eq!(exit_desync, ipc::WorkerExit::FrameDesync);

        // 3. Full session via paired reader/writer stream: hello -> set_context -> encode_tx -> decode_slot -> ping -> session_reset -> bad_json recovery -> shutdown
        let (c2w_rx, mut c2w_tx) = std::os::unix::net::UnixStream::pair().unwrap();
        let (mut w2c_rx, w2c_tx) = std::os::unix::net::UnixStream::pair().unwrap();

        let worker_thread =
            std::thread::spawn(move || ipc::worker::run_worker_io(c2w_rx, w2c_tx, Some(2)));

        // Hello
        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"hello","id":"1","protocol_version":1,"client":"test"}"#,
            &[],
        )
        .unwrap();
        let f_ready = read_frame(&mut w2c_rx).unwrap().unwrap();
        let v_ready: serde_json::Value = serde_json::from_slice(&f_ready.json).unwrap();
        assert_eq!(v_ready["op"], "ready");
        assert_eq!(v_ready["threads"], 2);

        // Malformed JSON recovers non-fatally
        write_frame(&mut c2w_tx, 0, br#"{not valid json"#, &[]).unwrap();
        let f_badj = read_frame(&mut w2c_rx).unwrap().unwrap();
        let v_badj: serde_json::Value = serde_json::from_slice(&f_badj.json).unwrap();
        assert_eq!(v_badj["op"], "error");
        assert_eq!(v_badj["fatal"], false);

        // Set context
        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"set_context","id":"2","my_call":"N0AAA","dx_call":"K1ABC","qso_freq_hz":1500.0}"#,
            &[],
        )
        .unwrap();
        let f_ctx = read_frame(&mut w2c_rx).unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&f_ctx.json).unwrap()["op"],
            "ok"
        );

        // Encode TX (FT8 padded to slot)
        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"encode_tx","id":"tx1","mode":"ft8","message":"CQ K1ABC FN42","af_hz":1500.0,"pad_to_slot":true}"#,
            &[],
        )
        .unwrap();
        let f_tx = read_frame(&mut w2c_rx).unwrap().unwrap();
        let v_tx: serde_json::Value = serde_json::from_slice(&f_tx.json).unwrap();
        assert_eq!(v_tx["op"], "encode_result");
        assert_eq!(v_tx["sample_count"], 180_000);
        assert_eq!(f_tx.payload.len(), 180_000 * 4);

        // Feed generated TX audio right back into decode_slot (f32le)!
        write_frame(
            &mut c2w_tx,
            FLAG_BINARY_F32LE,
            br#"{"op":"decode_slot","id":"rx1","slot_id":"s1","mode":"ft8","depth":2,"sample_format":"f32le","enable_telemetry":true}"#,
            &f_tx.payload,
        )
        .unwrap();

        let mut saw_decode = false;
        loop {
            let f = read_frame(&mut w2c_rx).unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&f.json).unwrap();
            match v["op"].as_str().unwrap() {
                "decode_event" => {
                    assert_eq!(v["decode"]["message"], "CQ K1ABC FN42");
                    assert!(v["decode"]["telemetry"].is_object());
                    saw_decode = true;
                }
                "decode_done" => {
                    assert_eq!(v["status"], "completed");
                    assert!(saw_decode);
                    break;
                }
                other => panic!("unexpected op {other}"),
            }
        }

        // Session reset & ping
        write_frame(&mut c2w_tx, 0, br#"{"op":"session_reset","id":"r1"}"#, &[]).unwrap();
        let f_rst = read_frame(&mut w2c_rx).unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&f_rst.json).unwrap()["op"],
            "ok"
        );
        write_frame(&mut c2w_tx, 0, br#"{"op":"ping","id":"p1","seq":7}"#, &[]).unwrap();
        let f_pong = read_frame(&mut w2c_rx).unwrap().unwrap();
        let v_pong: serde_json::Value = serde_json::from_slice(&f_pong.json).unwrap();
        assert_eq!(v_pong["op"], "pong");
        assert_eq!(v_pong["seq"], 7);
        assert_eq!(v_pong["hash_entries"], 0);

        // Clean shutdown
        write_frame(&mut c2w_tx, 0, br#"{"op":"shutdown","id":"end"}"#, &[]).unwrap();
        let f_bye = read_frame(&mut w2c_rx).unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&f_bye.json).unwrap()["op"],
            "ok"
        );
        let _ = c2w_tx.shutdown(std::net::Shutdown::Both);
        drop(c2w_tx);
        assert_eq!(worker_thread.join().unwrap(), ipc::WorkerExit::Clean);
    }

    #[test]
    fn worker_cancels_active_decode_fast_and_runs_concurrent_tx() {
        use ipc::framing::{read_frame, write_frame};
        use std::net::Shutdown;

        let (c2w_rx, mut c2w_tx) = std::os::unix::net::UnixStream::pair().unwrap();
        let (mut w2c_rx, w2c_tx) = std::os::unix::net::UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || ipc::worker::run_worker_io(c2w_rx, w2c_tx, Some(2)));

        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"hello","id":"h","protocol_version":1}"#,
            &[],
        )
        .unwrap();
        let _ = read_frame(&mut w2c_rx).unwrap().unwrap();

        // Synthesize noisy white/deterministic i16 audio to keep candidates busy during cancel.
        let mut dense = vec![0u8; 180_000 * 2];
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        for chunk in dense.chunks_exact_mut(2) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let s = ((state >> 48) as i16).clamp(-8000, 8000);
            chunk.copy_from_slice(&s.to_le_bytes());
        }

        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"decode_slot","id":"cdec","mode":"ft8","depth":3,"sample_format":"i16le"}"#,
            &dense,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));

        let t_tx = Instant::now();
        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"encode_tx","id":"txfast","mode":"ft4","message":"CQ N0AAA EM10","af_hz":1500.0}"#,
            &[],
        )
        .unwrap();
        let t_cancel = Instant::now();
        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"cancel","id":"cid","target_id":"cdec"}"#,
            &[],
        )
        .unwrap();

        let mut got_tx = false;
        let mut got_cancel_ack = false;
        let mut got_done = false;
        while !(got_tx && got_cancel_ack && got_done) {
            let f = read_frame(&mut w2c_rx).unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&f.json).unwrap();
            match v["op"].as_str().unwrap() {
                "encode_result" => {
                    assert!(t_tx.elapsed().as_millis() < 1000);
                    got_tx = true;
                }
                "cancel_ack" => {
                    assert_eq!(v["cancelled"], true);
                    got_cancel_ack = true;
                }
                "decode_done" => {
                    let status = v["status"].as_str().unwrap();
                    assert!(status == "cancelled" || status == "completed");
                    assert!(t_cancel.elapsed().as_millis() < 2000);
                    got_done = true;
                }
                "decode_event" => {}
                other => panic!("unexpected op {other}"),
            }
        }

        // Timeout watchdog test
        write_frame(
            &mut c2w_tx,
            0,
            br#"{"op":"decode_slot","id":"tdec","mode":"ft8","depth":3,"sample_format":"i16le","timeout_ms":1}"#,
            &dense,
        )
        .unwrap();
        loop {
            let f = read_frame(&mut w2c_rx).unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&f.json).unwrap();
            if v["op"] == "decode_done" {
                let st = v["status"].as_str().unwrap();
                assert!(st == "timeout" || st == "completed", "status={st}");
                break;
            }
        }

        write_frame(&mut c2w_tx, 0, br#"{"op":"shutdown","id":"bye"}"#, &[]).unwrap();
        let _ = read_frame(&mut w2c_rx).unwrap().unwrap();
        let _ = c2w_tx.shutdown(Shutdown::Both);
        drop(c2w_tx);
        assert_eq!(handle.join().unwrap(), ipc::WorkerExit::Clean);
    }
}
