// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Stage 3 Suite A — Standalone Fortran vs. Rust `osd174_91` & `decode174_91` unit oracle.
//!
//! Feeds 2,200+ LLR vectors (AWGN, real BP `zsave`, AP `a1`–`a6`, `Keff ∈ {77,84,91}` ×
//! `ndeep ∈ {0..=3}`, adversarial edges, and hybrid `decode174_91`) to both WSJT-X Fortran
//! and pure-Rust `ftxr_core::fec` and asserts 100% codeword identity on distinct-LLR inputs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ftxr_core::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
use ftxr_core::dsp::fft::Ft8FftPlans;
use ftxr_core::fec::bp::{bp_decode_174_91, LDPC_N};
use ftxr_core::fec::decode174_91::decode174_91;
use ftxr_core::fec::osd::{osd174_91, OsdScratch};
use ftxr_core::ft8::ft8b::{compute_all_bit_metrics, ft8b_extract_stage2_intermediates, Ft8bScratch};
use ftxr_core::ft8::params::NMAX;
use ftxr_core::ft8::sync8::{sync8, BitMetricMode, Sync8Scratch};
use ftxr_core::ft8::sync8d::Sync8dTables;
use ftxr_core::protocol::hashtable::CallsignHashTable;
use ftxr_core::protocol::ldpc::encode174_91;
use ftxr_core::protocol::pack77::pack77;
use ftxr_core::protocol::types::UnpackContext;
use ftxr_core::telemetry::FecMethod;
use ftxr_core::Mode;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use thiserror::Error;

use crate::sim::{SimError, SimRunner};
use crate::synth::{derive_seed, synthesize_slot, GaussianRng, SignalSpec, SlotSpec};

const MAX_DMIN_REL_ERR: f64 = 1e-4;

#[derive(Debug, Error)]
pub enum OsdOracleError {
    #[error("simulator error: {0}")]
    Sim(#[from] SimError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Fortran osd_oracle build/run failed: {0}")]
    Fortran(String),
    #[error("Suite A OSD oracle gate failed: {0}")]
    GateFailed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupReport {
    pub group: String,
    pub total: usize,
    pub cw_exact: usize,
    pub nhard_exact: usize,
    pub max_dmin_rel_err: f64,
    pub extended_pivots: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SuiteAReport {
    pub total_vectors: usize,
    pub groups: Vec<GroupReport>,
    pub hybrid_total: usize,
    pub hybrid_exact: usize,
    pub hybrid_max_dmin_rel_err: f64,
}

struct OsdVector {
    group: &'static str,
    mode_op: i32, // 0 = osd174_91, 1 = decode174_91
    k: usize,
    ndeep_or_maxosd: i32,
    norder: u8,
    apmask: [u8; LDPC_N],
    llr: [f32; LDPC_N],
    distinct_llr: bool,
}

struct FortranOut {
    ntype: i32,
    nhard: i32,
    dmin: f32,
    cw: [u8; LDPC_N],
    msg91: [u8; 91],
}

pub fn ensure_osd_oracle_binary(
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
) -> Result<PathBuf, OsdOracleError> {
    let src = rust_ftx_root.join("tools/ftxr-oracle/fortran_dump/osd_oracle.f90");
    let lib_fort = bin_dir.join("libwsjt_fort.a");
    let lib_cxx = bin_dir.join("libwsjt_cxx.a");
    if !lib_fort.exists() || !lib_cxx.exists() {
        return Err(OsdOracleError::Fortran(format!(
            "missing libwsjt_fort.a or libwsjt_cxx.a in {}",
            bin_dir.display()
        )));
    }

    let out_dir = cache_dir.join("bin");
    fs::create_dir_all(&out_dir)?;
    let exe = out_dir.join("osd_oracle");

    let needs_rebuild = if exe.exists() {
        let exe_m = fs::metadata(&exe)?.modified()?;
        let src_m = fs::metadata(&src)?.modified()?;
        let lib_m = fs::metadata(&lib_fort)?.modified()?;
        src_m > exe_m || lib_m > exe_m
    } else {
        true
    };

    if needs_rebuild {
        let output = Command::new("gfortran")
            .args([
                "-O2",
                "-fno-f2c",
                "-fno-second-underscore",
                "-I",
                &bin_dir.display().to_string(),
                &src.display().to_string(),
                &lib_fort.display().to_string(),
                &lib_cxx.display().to_string(),
                "-lfftw3f_threads",
                "-lfftw3f",
                "-lm",
                "-lboost_log_setup",
                "-lboost_log",
                "-lboost_chrono",
                "-lboost_filesystem",
                "-lboost_regex",
                "-lboost_thread",
                "-lboost_atomic",
                "-lstdc++",
                "-lgfortran",
                "-lquadmath",
                "-o",
                &exe.display().to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        if !output.status.success() {
            return Err(OsdOracleError::Fortran(format!(
                "gfortran compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }

    Ok(exe)
}

fn run_fortran_batch(exe: &Path, vecs: &[&OsdVector]) -> Result<Vec<FortranOut>, OsdOracleError> {
    let tmp = tempfile::tempdir()?;
    let in_path = tmp.path().join("req.bin");
    let out_path = tmp.path().join("resp.bin");

    let rec_size = 4 + 4 + 4 + 4 + LDPC_N + 4 * LDPC_N;
    let mut bytes = Vec::with_capacity(4 + vecs.len() * rec_size);
    bytes.extend_from_slice(&(vecs.len() as i32).to_le_bytes());
    for v in vecs {
        bytes.extend_from_slice(&v.mode_op.to_le_bytes());
        bytes.extend_from_slice(&(v.k as i32).to_le_bytes());
        bytes.extend_from_slice(&v.ndeep_or_maxosd.to_le_bytes());
        bytes.extend_from_slice(&i32::from(v.norder).to_le_bytes());
        bytes.extend_from_slice(&v.apmask);
        for &l in &v.llr {
            bytes.extend_from_slice(&l.to_le_bytes());
        }
    }
    fs::write(&in_path, &bytes)?;

    let out = Command::new(exe)
        .args([in_path.display().to_string(), out_path.display().to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        return Err(OsdOracleError::Fortran(format!(
            "osd_oracle failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }

    let rbytes = fs::read(&out_path)?;
    let mut pos = 0usize;
    let read_i32 = |buf: &[u8], p: &mut usize| -> i32 {
        let val = i32::from_le_bytes(buf[*p..*p + 4].try_into().unwrap());
        *p += 4;
        val
    };
    let read_f32 = |buf: &[u8], p: &mut usize| -> f32 {
        let val = f32::from_le_bytes(buf[*p..*p + 4].try_into().unwrap());
        *p += 4;
        val
    };

    let n = read_i32(&rbytes, &mut pos) as usize;
    assert_eq!(n, vecs.len());
    let mut results = Vec::with_capacity(n);
    for _ in 0..n {
        let ntype = read_i32(&rbytes, &mut pos);
        let nhard = read_i32(&rbytes, &mut pos);
        let dmin = read_f32(&rbytes, &mut pos);
        let mut cw = [0u8; LDPC_N];
        cw.copy_from_slice(&rbytes[pos..pos + LDPC_N]);
        pos += LDPC_N;
        let mut msg91 = [0u8; 91];
        msg91.copy_from_slice(&rbytes[pos..pos + 91]);
        pos += 91;
        results.push(FortranOut {
            ntype,
            nhard,
            dmin,
            cw,
            msg91,
        });
    }
    Ok(results)
}

fn random_valid_cw(rng: &mut ChaCha8Rng, ht: &CallsignHashTable, ctx: &UnpackContext) -> [u8; LDPC_N] {
    let calls = ["N0AAA", "K9BBB", "AA1AA", "BB2BB", "VE3CCC", "DL4DDD", "JA5EEE"];
    let grids = ["EM10", "DM79", "FN20", "IO91", "JN48", "PM95"];
    let c1 = calls[rng.gen_range(0..calls.len())];
    let c2 = calls[rng.gen_range(0..calls.len())];
    let g = grids[rng.gen_range(0..grids.len())];
    let msg = if rng.gen_bool(0.3) {
        format!("CQ {c1} {g}")
    } else if rng.gen_bool(0.5) {
        format!("{c1} {c2} RR73")
    } else {
        format!("{c1} {c2} -18")
    };
    let p = pack77(&msg, None, ctx, ht).unwrap();
    encode174_91(&p.bits)
}

pub fn run_suite_a_osd_oracle(
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
    out_json: Option<&Path>,
) -> Result<SuiteAReport, OsdOracleError> {
    let exe = ensure_osd_oracle_binary(rust_ftx_root, bin_dir, cache_dir)?;
    let sim = SimRunner::new(bin_dir.to_path_buf(), cache_dir.to_path_buf());
    let ht = CallsignHashTable::new();
    let ctx = UnpackContext::default();

    let mut rng = ChaCha8Rng::from_seed(derive_seed("v1", "stage3_suite_a", Mode::Ft8, "osd", 0));
    let mut grng = GaussianRng::from_seed(derive_seed("v1", "stage3_suite_a", Mode::Ft8, "awgn", 1));

    let mut all_vecs: Vec<OsdVector> = Vec::with_capacity(2250);

    // 1. 500 uncoded/coded AWGN vectors (Eb/N0 from -1 to +5 dB, Keff=91, ndeep=2, apmask=0)
    for i in 0..500 {
        let cw = random_valid_cw(&mut rng, &ht, &ctx);
        let eb_n0_db = -1.0_f64 + 6.0 * ((i % 7) as f64) / 6.0;
        let sigma = (0.5 * 10.0_f64.powf(-eb_n0_db / 10.0) * (174.0 / 91.0)).sqrt();
        let mut llr = [0.0_f32; LDPC_N];
        for b in 0..LDPC_N {
            let x = if cw[b] == 1 { 1.0_f64 } else { -1.0_f64 };
            let y = x + sigma * grng.next_gaussian();
            llr[b] = (2.0 * y / (sigma * sigma)) as f32;
        }
        all_vecs.push(OsdVector {
            group: "coded_awgn",
            mode_op: 0,
            k: 91,
            ndeep_or_maxosd: 2,
            norder: 2,
            apmask: [0u8; LDPC_N],
            llr,
            distinct_llr: true,
        });
    }

    // 2. 500 real BP-accumulated zsave vectors from -20..-23 dB FT8 frames
    {
        let plans = Ft8FftPlans::new();
        let downsampler = Ft8Downsampler::new();
        let sync8d_tables = Sync8dTables::new();
        let mut sync_scratch = Sync8Scratch::new(&plans);
        let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
        let mut snapshot = Ft8SpectrumSnapshot::new();
        let mut ft8b_scratch = Ft8bScratch::new(&plans);

        let mut collected = 0usize;
        let mut trial = 0usize;
        while collected < 500 {
            let snr_db = -20.0 - ((trial % 4) as f64);
            let slot = SlotSpec {
                mode: Mode::Ft8,
                signals: vec![SignalSpec {
                    message: "CQ N0AAA EM10".to_string(),
                    af_hz: 1500.0,
                    dt_sec: 0.0,
                    snr_db,
                    late_start_ms: None,
                }],
                truncate_ms: None,
            };
            let seed = derive_seed("v1", "suite_a_zsave", Mode::Ft8, "frame", trial);
            trial += 1;
            let synth = synthesize_slot(&sim, seed, &slot)?;
            let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
            for (d, &s) in dd.iter_mut().zip(synth.samples.iter()) {
                *d = f32::from(s);
            }
            let cands = sync8(&dd, 1400.0, 1600.0, 1.3, 1500.0, 50, &plans, &mut sync_scratch);
            downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);
            for cand in cands.iter().take(5) {
                let Some(inter) = ft8b_extract_stage2_intermediates(
                    &snapshot,
                    cand,
                    3,
                    BitMetricMode::Power,
                    &plans,
                    &downsampler,
                    &sync8d_tables,
                    &mut ft8b_scratch,
                ) else {
                    continue;
                };
                let metrics =
                    compute_all_bit_metrics(&ft8b_scratch.cs, BitMetricMode::Power, &mut ft8b_scratch.s2);
                for llr_set in &metrics.llrs {
                    let bp = bp_decode_174_91(llr_set, &[0u8; LDPC_N], 2);
                    if bp.decoded.is_none() {
                        for zrow in bp.zsave.iter().take(bp.num_zsave) {
                            all_vecs.push(OsdVector {
                                group: "bp_zsave",
                                mode_op: 0,
                                k: 91,
                                ndeep_or_maxosd: 2,
                                norder: 2,
                                apmask: [0u8; LDPC_N],
                                llr: *zrow,
                                distinct_llr: true,
                            });
                            collected += 1;
                            if collected >= 500 {
                                break;
                            }
                        }
                    }
                    if collected >= 500 {
                        break;
                    }
                }
                let _ = inter;
                if collected >= 500 {
                    break;
                }
            }
        }
    }

    // 3. 500 AP-masked vectors (a1..a6)
    let ap_widths = [32usize, 32, 61, 77, 77, 77];
    for i in 0..500 {
        let cw = random_valid_cw(&mut rng, &ht, &ctx);
        let sigma = 1.15_f64;
        let mut llr = [0.0_f32; LDPC_N];
        for b in 0..LDPC_N {
            let x = if cw[b] == 1 { 1.0_f64 } else { -1.0_f64 };
            let y = x + sigma * grng.next_gaussian();
            llr[b] = (2.0 * y / (sigma * sigma)) as f32;
        }
        let apmag = llr.iter().map(|v| v.abs()).fold(0.0_f32, f32::max) * 1.1;
        let kind = i % 6;
        let n_lock = ap_widths[kind];
        let mut apmask = [0u8; LDPC_N];
        let match_ap = i % 5 != 0; // 80% matching AP, 20% mismatching AP
        for b in 0..n_lock.min(77) {
            if b < 58 || n_lock == 77 || b >= 74 {
                apmask[b] = 1;
                let bit = if match_ap { cw[b] } else { cw[b] ^ 1 };
                llr[b] = if bit == 1 { apmag } else { -apmag };
            }
        }
        all_vecs.push(OsdVector {
            group: "ap_masked",
            mode_op: 0,
            k: 91,
            ndeep_or_maxosd: 2,
            norder: 2,
            apmask,
            llr,
            distinct_llr: false, // AP bits share magnitude `apmag`
        });
    }

    // 4. 250 Keff in {77, 84, 91} x ndeep in {0, 1, 2, 3}
    let keffs = [77usize, 84, 91];
    for i in 0..250 {
        let cw = random_valid_cw(&mut rng, &ht, &ctx);
        let sigma = 0.9_f64 + 0.3 * ((i % 3) as f64);
        let mut llr = [0.0_f32; LDPC_N];
        for b in 0..LDPC_N {
            let x = if cw[b] == 1 { 1.0_f64 } else { -1.0_f64 };
            let y = x + sigma * grng.next_gaussian();
            llr[b] = (2.0 * y / (sigma * sigma)) as f32;
        }
        all_vecs.push(OsdVector {
            group: "keff_ndeep_sweep",
            mode_op: 0,
            k: keffs[i % keffs.len()],
            ndeep_or_maxosd: (i % 4) as i32,
            norder: 2,
            apmask: [0u8; LDPC_N],
            llr,
            distinct_llr: true,
        });
    }

    // 5. 250 adversarial edge vectors (zeroes, exact 0.0, ±1e4, rank edges)
    for i in 0..250 {
        let cw = random_valid_cw(&mut rng, &ht, &ctx);
        let mut llr = [0.0_f32; LDPC_N];
        let mut distinct = true;
        match i % 5 {
            0 => {
                // All-zero LLR
                distinct = false;
            }
            1 => {
                // Half exact 0.0, half distinct
                distinct = false;
                for b in 0..LDPC_N {
                    if b % 2 == 0 {
                        llr[b] = 0.0;
                    } else {
                        let mag = 1.0 + 0.01 * (b as f32);
                        llr[b] = if cw[b] == 1 { mag } else { -mag };
                    }
                }
            }
            2 => {
                // Extreme ±1e4 with distinct perturbations
                for b in 0..LDPC_N {
                    let mag = 10_000.0 - (b as f32) * 0.5;
                    let flip = b == 12 || b == 44;
                    let bit = if flip { cw[b] ^ 1 } else { cw[b] };
                    llr[b] = if bit == 1 { mag } else { -mag };
                }
            }
            3 => {
                // Tiny 1e-6 distinct LLRs
                for b in 0..LDPC_N {
                    let mag = 1e-6 * (1.0 + (b as f32) * 0.02);
                    llr[b] = if cw[b] == 1 { mag } else { -mag };
                }
            }
            _ => {
                // Weakest MRB bits zeroed
                for b in 0..LDPC_N {
                    let mag = if b < 85 { 2.0 + (b as f32) * 0.01 } else { 0.05 + (b as f32) * 0.001 };
                    llr[b] = if cw[b] == 1 { mag } else { -mag };
                }
            }
        }
        all_vecs.push(OsdVector {
            group: "adversarial",
            mode_op: 0,
            k: 91,
            ndeep_or_maxosd: i % 4,
            norder: 2,
            apmask: [0u8; LDPC_N],
            llr,
            distinct_llr: distinct,
        });
    }

    // 6. 200 hybrid decode174_91 (maxosd=2, norder=2) differential checks
    for i in 0..200 {
        let cw = random_valid_cw(&mut rng, &ht, &ctx);
        let sigma = 0.8 + 0.6 * ((i % 4) as f64) / 3.0;
        let mut llr = [0.0_f32; LDPC_N];
        for b in 0..LDPC_N {
            let x = if cw[b] == 1 { 1.0_f64 } else { -1.0_f64 };
            let y = x + sigma * grng.next_gaussian();
            llr[b] = (2.0 * y / (sigma * sigma)) as f32;
        }
        all_vecs.push(OsdVector {
            group: "hybrid_decode174_91",
            mode_op: 1,
            k: 91,
            ndeep_or_maxosd: 2,
            norder: 2,
            apmask: [0u8; LDPC_N],
            llr,
            distinct_llr: true,
        });
    }

    // Run Fortran per distinct (mode_op, k) batch so `osd174_91`'s `save first` `gen(k, N)`
    // matches each vector's exact `k`.
    let mut f90_outputs: Vec<Option<FortranOut>> = (0..all_vecs.len()).map(|_| None).collect();
    for k in [77usize, 84, 91] {
        for mode_op in [0i32, 1] {
            let idxs: Vec<usize> = all_vecs
                .iter()
                .enumerate()
                .filter(|(_, v)| v.k == k && v.mode_op == mode_op)
                .map(|(i, _)| i)
                .collect();
            if idxs.is_empty() {
                continue;
            }
            let refs: Vec<&OsdVector> = idxs.iter().map(|&i| &all_vecs[i]).collect();
            let outs = run_fortran_batch(&exe, &refs)?;
            for (pos, out) in idxs.into_iter().zip(outs) {
                f90_outputs[pos] = Some(out);
            }
        }
    }

    let mut scratch = OsdScratch::new();
    let group_names = [
        "coded_awgn",
        "bp_zsave",
        "ap_masked",
        "keff_ndeep_sweep",
        "adversarial",
    ];
    let mut groups = Vec::new();

    for gname in group_names {
        let mut total = 0usize;
        let mut cw_exact = 0usize;
        let mut nhard_exact = 0usize;
        let mut max_dmin_rel_err = 0.0_f64;
        let mut extended_pivots = 0usize;

        for (v, f_opt) in all_vecs.iter().zip(f90_outputs.iter()) {
            if v.group != gname {
                continue;
            }
            total += 1;
            let f = f_opt.as_ref().unwrap();
            let r = osd174_91(
                &v.llr,
                v.k,
                &v.apmask,
                v.ndeep_or_maxosd as u8,
                &mut scratch,
            )
            .ok_or_else(|| OsdOracleError::GateFailed("osd174_91 returned None".into()))?;

            if !r.dmin.is_finite() {
                return Err(OsdOracleError::GateFailed(format!(
                    "[{gname}] non-finite dmin {}",
                    r.dmin
                )));
            }

            let mut r_true_dmin = 0.0_f64;
            let mut f_true_dmin = 0.0_f64;
            let mut r_true_nhard = 0_i32;
            for b in 0..LDPC_N {
                let hbit = u8::from(v.llr[b] >= 0.0);
                let abs_l = f64::from(v.llr[b].abs());
                if r.cw[b] != hbit {
                    r_true_nhard += 1;
                    r_true_dmin += abs_l;
                }
                if f.cw[b] != hbit {
                    f_true_dmin += abs_l;
                }
            }
            let r_self_rel = (f64::from(r.dmin) - r_true_dmin).abs() / r_true_dmin.max(1.0);
            if r.nhardmin.abs() != r_true_nhard || r_self_rel > MAX_DMIN_REL_ERR {
                return Err(OsdOracleError::GateFailed(format!(
                    "[{gname}] Rust OSD internal dmin/nhard mismatch: reported (nhard={}, dmin={}), true=({}, {})",
                    r.nhardmin, r.dmin, r_true_nhard, r_true_dmin
                )));
            }

            let same_cw = r.cw == f.cw;
            let same_nhard = r.nhardmin == f.nhard;
            if same_cw {
                cw_exact += 1;
            }
            if same_nhard {
                nhard_exact += 1;
            }

            let rel_err = (f64::from(r.dmin) - f64::from(f.dmin)).abs()
                / f64::from(f.dmin).abs().max(1.0);

            if r.used_extended_pivot {
                extended_pivots += 1;
                let f_bug_rel = (f64::from(f.dmin) - f_true_dmin).abs() / f_true_dmin.max(1.0);
                println!(
                    "[INFO] [{gname}] k+20 Fortran pivot exhausted (k={}, ndeep={}): same_cw={same_cw}, r_dmin={:.4} (true={r_true_dmin:.4}), f_dmin={:.4} (f_true={f_true_dmin:.4}, f_err={f_bug_rel:.2e})",
                    v.k, v.ndeep_or_maxosd, r.dmin, f.dmin
                );
                if f.nhard >= 0 && (r.nhardmin < 0 || f64::from(r.dmin) > f_true_dmin + 1e-3) {
                    return Err(OsdOracleError::GateFailed(format!(
                        "[{gname}] extended-pivot Rust OSD underperformed valid Fortran decode: r_nhard={}, f_nhard={}, r_dmin={}, f_true_dmin={}",
                        r.nhardmin, f.nhard, r.dmin, f_true_dmin
                    )));
                }
            } else {
                max_dmin_rel_err = max_dmin_rel_err.max(rel_err);
                if v.distinct_llr && (!same_cw || !same_nhard || rel_err > MAX_DMIN_REL_ERR) {
                    return Err(OsdOracleError::GateFailed(format!(
                        "[{gname}] mismatch on distinct-LLR vector (k={}, ndeep={}): same_cw={same_cw}, nhard=({} vs {}), dmin=({} vs {}, rel_err={rel_err:.2e})",
                        v.k, v.ndeep_or_maxosd, r.nhardmin, f.nhard, r.dmin, f.dmin
                    )));
                }
            }
        }

        if gname == "ap_masked" && cw_exact < total {
            println!(
                "[INFO] ap_masked exact cw={cw_exact}/{total} (tie-breaking on equal AP magnitudes)"
            );
        }

        println!(
            "[PASS] Suite A [{gname}]: {cw_exact}/{total} exact cw, {nhard_exact}/{total} exact nhard, max_dmin_rel_err={max_dmin_rel_err:.2e}, ext_pivots={extended_pivots}"
        );
        groups.push(GroupReport {
            group: gname.to_string(),
            total,
            cw_exact,
            nhard_exact,
            max_dmin_rel_err,
            extended_pivots,
        });
    }

    // Hybrid decode174_91 verification
    let mut hybrid_total = 0usize;
    let mut hybrid_exact = 0usize;
    let mut hybrid_max_dmin_rel_err = 0.0_f64;

    for (v, f_opt) in all_vecs.iter().zip(f90_outputs.iter()) {
        if v.group != "hybrid_decode174_91" {
            continue;
        }
        hybrid_total += 1;
        let f = f_opt.as_ref().unwrap();
        let r_opt = decode174_91(
            &v.llr,
            v.k,
            v.ndeep_or_maxosd,
            v.norder,
            &v.apmask,
            &mut scratch,
        );
        match (f.ntype, r_opt) {
            (0, None) => {
                hybrid_exact += 1;
            }
            (ntype @ (1 | 2), Some(dec)) => {
                let r_ntype = match dec.fec_method {
                    FecMethod::Bp { .. } => 1,
                    FecMethod::Osd { .. } => 2,
                };
                let rel_err = (f64::from(dec.dmin) - f64::from(f.dmin)).abs()
                    / f64::from(f.dmin).abs().max(1.0);
                hybrid_max_dmin_rel_err = hybrid_max_dmin_rel_err.max(rel_err);
                if r_ntype == ntype
                    && dec.codeword == f.cw
                    && dec.message91 == f.msg91
                    && dec.nhard_errors == f.nhard
                    && rel_err <= MAX_DMIN_REL_ERR
                {
                    hybrid_exact += 1;
                } else {
                    return Err(OsdOracleError::GateFailed(format!(
                        "[hybrid_decode174_91] mismatch: ntype=({r_ntype} vs {ntype}), nhard=({} vs {}), dmin=({} vs {})",
                        dec.nhard_errors, f.nhard, dec.dmin, f.dmin
                    )));
                }
            }
            (ntype, other) => {
                return Err(OsdOracleError::GateFailed(format!(
                    "[hybrid_decode174_91] outcome mismatch: f90 ntype={ntype}, rust={}",
                    other.is_some()
                )));
            }
        }
    }
    println!(
        "[PASS] Suite A [hybrid_decode174_91]: {hybrid_exact}/{hybrid_total} exact, max_dmin_rel_err={hybrid_max_dmin_rel_err:.2e}"
    );

    let report = SuiteAReport {
        total_vectors: all_vecs.len(),
        groups,
        hybrid_total,
        hybrid_exact,
        hybrid_max_dmin_rel_err,
    };

    if let Some(out_path) = out_json {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(out_path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }

    Ok(report)
}
