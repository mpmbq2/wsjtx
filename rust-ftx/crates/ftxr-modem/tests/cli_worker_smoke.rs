// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 4] = b"FTXR";
const FLAG_HAS_BINARY: u16 = 0x0001;
const FLAG_BINARY_F32LE: u16 = 0x0002;

fn write_frame<W: Write>(w: &mut W, flags: u16, json: &[u8], payload: &[u8]) -> io::Result<()> {
    let eff_flags = flags | if payload.is_empty() { 0 } else { FLAG_HAS_BINARY };
    let mut hdr = [0u8; 16];
    hdr[0..4].copy_from_slice(MAGIC);
    hdr[4..6].copy_from_slice(&1u16.to_le_bytes());
    hdr[6..8].copy_from_slice(&eff_flags.to_le_bytes());
    hdr[8..12].copy_from_slice(&(json.len() as u32).to_le_bytes());
    hdr[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&hdr)?;
    w.write_all(json)?;
    w.write_all(payload)?;
    Ok(())
}

fn read_frame<R: Read>(r: &mut R) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut hdr = [0u8; 16];
    r.read_exact(&mut hdr)?;
    assert_eq!(&hdr[0..4], MAGIC, "stdout had non-FTXR prefix: {:?}", &hdr[0..4]);
    let version = u16::from_le_bytes([hdr[4], hdr[5]]);
    assert_eq!(version, 1);
    let json_len = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    let payload_len = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) as usize;
    let mut json = vec![0u8; json_len];
    r.read_exact(&mut json)?;
    let mut payload = vec![0u8; payload_len];
    r.read_exact(&mut payload)?;
    Ok((json, payload))
}

#[test]
fn subprocess_cli_and_worker_binary_smoke() {
    let bin = env!("CARGO_BIN_EXE_ftxr-modem");

    // 1. `--version` single line & startup budget (<20ms in release, <250ms in debug)
    let t_ver = Instant::now();
    let ver_out = Command::new(bin).arg("--version").output().unwrap();
    let ver_elapsed = t_ver.elapsed();
    assert!(ver_out.status.success());
    let ver_str = String::from_utf8(ver_out.stdout).unwrap();
    assert_eq!(ver_str.lines().count(), 1, "ver={ver_str:?}");
    assert!(ver_str.contains("ftxr-modem"), "ver={ver_str:?}");
    let ver_limit = if cfg!(debug_assertions) {
        Duration::from_millis(250)
    } else {
        Duration::from_millis(20)
    };
    assert!(
        ver_elapsed < ver_limit,
        "--version took {ver_elapsed:?} (limit {ver_limit:?})"
    );

    // 2. `--version --json` & `--license`
    let ver_json_out = Command::new(bin).args(["--version", "--json"]).output().unwrap();
    assert!(ver_json_out.status.success());
    let vj: serde_json::Value = serde_json::from_slice(&ver_json_out.stdout).unwrap();
    assert_eq!(vj["ipc_protocol_version"], 1);
    assert_eq!(vj["license"], "GPL-3.0-only");

    let lic_out = Command::new(bin).arg("--license").output().unwrap();
    assert!(lic_out.status.success());
    let lic_str = String::from_utf8(lic_out.stdout).unwrap();
    assert!(
        lic_str.contains("GNU General Public License") && lic_str.contains("GPLv3"),
        "lic_str={lic_str:?}"
    );

    // 3. `encode` + `decode` subprocess roundtrip (`--format jt9` and `--format json --telemetry`)
    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("000001_sub.wav");
    let enc_out = Command::new(bin)
        .args([
            "encode",
            "--mode",
            "ft8",
            "--af",
            "1500",
            "--sr",
            "12000",
            "--out",
            wav_path.to_str().unwrap(),
            "CQ N0AAA EM10",
        ])
        .output()
        .unwrap();
    assert!(
        enc_out.status.success(),
        "encode stderr={}",
        String::from_utf8_lossy(&enc_out.stderr)
    );

    let dec_jt9 = Command::new(bin)
        .args([
            "decode",
            "--mode",
            "ft8",
            "--depth",
            "2",
            "--format",
            "jt9",
            wav_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(dec_jt9.status.success());
    let jt9_str = String::from_utf8(dec_jt9.stdout).unwrap();
    assert!(jt9_str.contains("CQ N0AAA EM10"), "jt9_str={jt9_str:?}");

    let dec_json = Command::new(bin)
        .args([
            "decode",
            "--mode",
            "ft8",
            "--depth",
            "2",
            "--format",
            "json",
            "--telemetry",
            wav_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(dec_json.status.success());
    let dj: serde_json::Value = serde_json::from_slice(&dec_json.stdout).unwrap();
    assert_eq!(dj["mode"], "ft8");
    let decs = dj["decodes"].as_array().unwrap();
    assert!(decs.iter().any(|d| d["message"] == "CQ N0AAA EM10"));
    assert!(decs[0]["telemetry"].is_object());

    // 4. `--worker` real stdio subprocess (verify zero stray stdout bytes + clean exit 0)
    let mut child = Command::new(bin)
        .args(["--worker", "--threads", "2"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut c_in = child.stdin.take().unwrap();
    let mut c_out = child.stdout.take().unwrap();

    write_frame(
        &mut c_in,
        0,
        br#"{"op":"hello","id":"sub_h","protocol_version":1}"#,
        &[],
    )
    .unwrap();
    c_in.flush().unwrap();

    let (hello_json, _) = read_frame(&mut c_out).unwrap();
    let hello_v: serde_json::Value = serde_json::from_slice(&hello_json).unwrap();
    assert_eq!(hello_v["op"], "ready");
    assert_eq!(hello_v["protocol_version"], 1);

    // Encode via warm worker
    write_frame(
        &mut c_in,
        0,
        br#"{"op":"encode_tx","id":"sub_tx","mode":"ft4","message":"CQ K1ABC FN42","af_hz":1500.0,"pad_to_slot":true,"sample_rate":12000,"sample_format":"f32le"}"#,
        &[],
    )
    .unwrap();
    c_in.flush().unwrap();
    let (tx_json, tx_pcm) = read_frame(&mut c_out).unwrap();
    let tx_v: serde_json::Value = serde_json::from_slice(&tx_json).unwrap();
    assert_eq!(tx_v["op"], "encode_result");
    assert!(!tx_pcm.is_empty());

    // Decode that PCM via warm worker
    write_frame(
        &mut c_in,
        FLAG_HAS_BINARY | FLAG_BINARY_F32LE,
        br#"{"op":"decode_slot","id":"sub_rx","mode":"ft4","depth":2,"sample_rate":12000,"sample_format":"f32le","rx_freq_hz":1500.0}"#,
        &tx_pcm,
    )
    .unwrap();
    c_in.flush().unwrap();

    let mut saw_msg = false;
    loop {
        let (f_json, _) = read_frame(&mut c_out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&f_json).unwrap();
        match v["op"].as_str().unwrap() {
            "decode_event" => {
                if v["decode"]["message"] == "CQ K1ABC FN42" {
                    saw_msg = true;
                }
            }
            "decode_done" => {
                assert_eq!(v["status"], "completed");
                break;
            }
            other => panic!("unexpected op {other}"),
        }
    }
    assert!(saw_msg);

    write_frame(&mut c_in, 0, br#"{"op":"shutdown","id":"sub_bye"}"#, &[]).unwrap();
    c_in.flush().unwrap();
    let (bye_json, _) = read_frame(&mut c_out).unwrap();
    let bye_v: serde_json::Value = serde_json::from_slice(&bye_json).unwrap();
    assert_eq!(bye_v["op"], "ok");
    drop(c_in);
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));

    // 5. `--worker` protocol_version mismatch exit code 4
    let mut bad = Command::new(bin)
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut b_in = bad.stdin.take().unwrap();
        write_frame(
            &mut b_in,
            0,
            br#"{"op":"hello","id":"bad","protocol_version":999}"#,
            &[],
        )
        .unwrap();
        b_in.flush().unwrap();
    }
    let bad_status = bad.wait().unwrap();
    assert_eq!(bad_status.code(), Some(4));
}
