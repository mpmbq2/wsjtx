// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Stage 3 FT8 weak-signal parity verification suites (Suites B, C, D, E):
//! - Suite B: Intermediate pipeline numerical diff (`bmeta..bmete`, `llra..llre`, `ft8apset`, `lrefinedt`)
//! - Suite C: Deterministic AWGN sensitivity cliff (`-15` to `-24 dB`, blind & AP `a1`–`a6` + Hound)
//! - Suite D: Crowded-band (15–25 overlapping signals, 25 dB DR) & real-world corpus diff vs `jt9 -8 -d 3`
//! - Suite E: Pure-noise false-positive sweep (500 AWGN slots) & `digi-dx` AP tag compatibility audit

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::FRAC_1_SQRT_2;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use ftxr_core::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
use ftxr_core::dsp::fft::Ft8FftPlans;
use ftxr_core::fec::bp::LDPC_N;
use ftxr_core::ft8::ap::compute_ap_seed;
use ftxr_core::ft8::decoder::{Ft8DecodeOptions, Ft8Decoder};
use ftxr_core::ft8::ft8b::{
    compute_all_bit_metrics, ft8b_extract_stage2_intermediates, Ft8bScratch,
};
use ftxr_core::ft8::params::{NMAX, NN, NZ};
use ftxr_core::ft8::subtract::{Ft8Subtractor, SubtractScratch};
use ftxr_core::ft8::sync8::BitMetricMode;
use ftxr_core::ft8::sync8d::Sync8dTables;
use ftxr_core::modulate::ft8::{encode_ft8_tones_from_77bits, gen_ft8_wave_real};
use ftxr_core::protocol::hashtable::CallsignHashTable;
use ftxr_core::protocol::pack77::pack77;
use ftxr_core::protocol::types::UnpackContext;
use ftxr_core::{ApTag, ContestMode, DecodeConfig, DecodedMessage, Mode, QsoContext};
use num_complex::Complex32;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::jt9::{Jt9Decode, Jt9Error, Jt9Profile, Jt9RunConfig, Jt9Runner};
use crate::osd_oracle::{run_suite_a_osd_oracle, OsdOracleError, SuiteAReport};
use crate::synth::{
    derive_seed, synthesize_slot_from_templates, GaussianRng, SignalSpec, SlotSpec,
};
use crate::wav::{self, WavError};

const MAX_REL_ERR: f64 = 1e-4;

fn flush_stdout() {
    let _ = std::io::stdout().flush();
}

#[derive(Debug, Error)]
pub enum Stage3Error {
    #[error("Suite A error: {0}")]
    SuiteA(#[from] OsdOracleError),
    #[error("jt9 error: {0}")]
    Jt9(#[from] Jt9Error),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("core error: {0}")]
    Core(#[from] ftxr_core::FtxrError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Fortran ft8_stage3_dump failed: {0}")]
    FortranDriver(String),
    #[error("Stage 3 exit gate failed: {0}")]
    GateFailed(String),
    #[error("Stage 3 checkpoint paused before {next_step} after {completed_in_run} steps ({elapsed_secs}s); re-run with --resume")]
    CheckpointPaused {
        next_step: String,
        completed_in_run: usize,
        elapsed_secs: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteBMetricErr {
    pub candidate_idx: usize,
    pub imetric: u8,
    pub bmeta_err: f64,
    pub bmetb_err: f64,
    pub bmetc_err: f64,
    pub bmetd_err: f64,
    pub bmete_err: f64,
    pub llra_err: f64,
    pub llrb_err: f64,
    pub llrc_err: f64,
    pub llrd_err: f64,
    pub llre_err: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteBFixtureReport {
    pub label: String,
    pub max_samples: usize,
    pub candidates_checked: usize,
    pub max_llr_rel_err: f64,
    pub sub_refinedt_rel_err: Option<f64>,
    pub per_candidate: Vec<SuiteBMetricErr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteBReport {
    pub apset_cases_checked: usize,
    pub apset_all_exact: bool,
    pub fixtures: Vec<SuiteBFixtureReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliffPoint {
    pub snr_db: f64,
    pub rust_trials: usize,
    pub rust_decoded: usize,
    pub rust_rate: f64,
    pub jt9_trials: usize,
    pub jt9_decoded: usize,
    pub jt9_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliffConfigSummary {
    pub name: String,
    pub rust_threshold_50pct_db: Option<f64>,
    pub jt9_threshold_50pct_db: Option<f64>,
    pub threshold_delta_db: Option<f64>,
    pub rust_ap_counts: BTreeMap<String, usize>,
    pub jt9_ap_counts: BTreeMap<String, usize>,
    pub points: Vec<CliffPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteCReport {
    pub trials_per_step_rust: usize,
    pub trials_per_step_jt9: usize,
    pub configs: Vec<CliffConfigSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteDFixtureSummary {
    pub fixture_id: String,
    pub jt9_count: usize,
    pub jt9_a7_a8_excluded: usize,
    pub rust_3stage_count: usize,
    pub rust_singleshot_count: usize,
    pub matched_3stage: usize,
    pub matched_singleshot: usize,
    pub jt9_elapsed_ms: f64,
    pub rust_3stage_elapsed_ms: f64,
    pub rust_singleshot_elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteDReport {
    pub crowded_slots: usize,
    pub crowded_truth_signals: usize,
    pub crowded_jt9_tp: usize,
    pub crowded_rust_3stage_tp: usize,
    pub crowded_rust_singleshot_tp: usize,
    pub crowded_ratio_3stage: f64,
    pub crowded_ratio_singleshot: f64,
    pub real_corpus: Vec<SuiteDFixtureSummary>,
    pub real_total_jt9: usize,
    pub real_total_rust_3stage: usize,
    pub real_total_rust_singleshot: usize,
    pub real_ratio_3stage: f64,
    pub real_ratio_singleshot: f64,
    pub matched_total: usize,
    pub matched_within_tol: usize,
    pub matched_within_tol_ratio: f64,
    pub max_af_diff_hz: f32,
    pub max_dt_diff_sec: f32,
    pub max_snr_diff_db: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteEConfigSummary {
    pub name: String,
    pub slots: usize,
    pub rust_fps_total: usize,
    pub rust_fps_verified: usize,
    pub rust_fps_ap_certain: usize,
    pub rust_fps_ap_uncertain: usize,
    pub jt9_fps_total: usize,
    pub jt9_fps_verified: usize,
    pub jt9_fps_ap_certain: usize,
    pub jt9_fps_ap_uncertain: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteEReport {
    pub configs: Vec<SuiteEConfigSummary>,
    pub digidx_metadata_audit_passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stage3Scorecard {
    pub suite_a: Option<SuiteAReport>,
    pub suite_b: Option<SuiteBReport>,
    pub suite_c: Option<SuiteCReport>,
    pub suite_d: Option<SuiteDReport>,
    pub suite_e: Option<SuiteEReport>,
}

pub struct Stage3Options {
    pub suites: BTreeSet<String>,
    pub trials: usize,
    pub jt9_trials: usize,
    pub noise_slots: usize,
    pub jobs: usize,
    pub cliff_configs: Option<BTreeSet<String>>,
    pub max_wall_secs: Option<u64>,
    pub resume_cache: bool,
}

struct ResumeCtx {
    cache_dir: Option<PathBuf>,
    exe_mtime: Option<SystemTime>,
    start: Instant,
    deadline: Option<Instant>,
    completed_in_run: usize,
}

impl ResumeCtx {
    fn new(cache_dir: Option<PathBuf>, max_wall_secs: Option<u64>) -> Self {
        let exe_mtime = std::env::current_exe()
            .ok()
            .and_then(|p| fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        let start = Instant::now();
        let deadline = max_wall_secs.map(|s| start + Duration::from_secs(s));
        Self {
            cache_dir,
            exe_mtime,
            start,
            deadline,
            completed_in_run: 0,
        }
    }

    fn load_json<T: DeserializeOwned>(&self, rel: &str) -> Option<T> {
        let root = self.cache_dir.as_ref()?;
        let path = root.join(rel);
        let meta = fs::metadata(&path).ok()?;
        let mtime = meta.modified().ok()?;
        if let Some(exe_m) = self.exe_mtime {
            if mtime < exe_m {
                return None;
            }
        }
        let data = fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    fn check_deadline(&self, next_step: &str) -> Result<(), Stage3Error> {
        if let Some(dl) = self.deadline {
            if self.completed_in_run > 0 && Instant::now() >= dl {
                return Err(Stage3Error::CheckpointPaused {
                    next_step: next_step.to_string(),
                    completed_in_run: self.completed_in_run,
                    elapsed_secs: self.start.elapsed().as_secs(),
                });
            }
        }
        Ok(())
    }

    fn record_step<T: Serialize>(&mut self, rel: &str, val: &T) -> Result<(), Stage3Error> {
        self.completed_in_run += 1;
        let Some(root) = self.cache_dir.as_ref() else {
            return Ok(());
        };
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(val)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }
}

struct BinReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BinReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn read_i32(&mut self) -> i32 {
        let v = i32::from_le_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn read_f32(&mut self) -> f32 {
        let v = f32::from_le_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn read_f32_vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.read_f32()).collect()
    }
    fn read_c32_vec(&mut self, n: usize) -> Vec<Complex32> {
        (0..n)
            .map(|_| {
                let re = self.read_f32();
                let im = self.read_f32();
                Complex32::new(re, im)
            })
            .collect()
    }
    fn read_i32_vec(&mut self, n: usize) -> Vec<i32> {
        (0..n).map(|_| self.read_i32()).collect()
    }
}

fn l2_rel_err_f32(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let d = f64::from(x) - f64::from(y);
        num += d * d;
        den += f64::from(y) * f64::from(y);
    }
    if den == 0.0 {
        num.sqrt()
    } else {
        (num / den).sqrt()
    }
}

fn build_ft8_unit_template(msg: &str, af_hz: f64) -> Vec<f64> {
    let ht = CallsignHashTable::new();
    let ctx = UnpackContext::default();
    let packed = pack77(msg, None, &ctx, &ht).expect("valid synthetic msg");
    let tones = encode_ft8_tones_from_77bits(&packed.bits);
    let wave = gen_ft8_wave_real(&tones, af_hz, 12_000);
    let mut tpl = vec![0.0_f64; NMAX];
    for (i, &w) in wave.iter().enumerate() {
        tpl[6000 + i] = f64::from(w);
    }
    let active = &tpl[6240..6000 + NZ - 240];
    let rms = (active.iter().map(|&x| x * x).sum::<f64>() / (active.len() as f64)).sqrt();
    let scale = FRAC_1_SQRT_2 / rms;
    for s in &mut tpl {
        *s *= scale;
    }
    tpl
}

fn interpolate_50pct(points: &[CliffPoint], use_rust: bool) -> Option<f64> {
    for win in points.windows(2) {
        let hi = &win[0];
        let lo = &win[1];
        let r_hi = if use_rust { hi.rust_rate } else { hi.jt9_rate };
        let r_lo = if use_rust { lo.rust_rate } else { lo.jt9_rate };
        if (r_hi - 0.5).abs() < 1e-9 {
            return Some(hi.snr_db);
        }
        if r_hi > 0.5 && r_lo <= 0.5 {
            let denom = r_hi - r_lo;
            if denom.abs() < 1e-9 {
                return Some(lo.snr_db);
            }
            let frac = (r_hi - 0.5) / denom;
            return Some(hi.snr_db + frac * (lo.snr_db - hi.snr_db));
        }
    }
    None
}

pub fn ensure_ft8_stage3_dump_binary(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
) -> Result<PathBuf, Stage3Error> {
    let src = rust_ftx_root.join("tools/ftxr-oracle/fortran_dump/ft8_stage3_dump.f90");
    let lib_fort = bin_dir.join("libwsjt_fort.a");
    let lib_cxx = bin_dir.join("libwsjt_cxx.a");
    if !lib_fort.exists() || !lib_cxx.exists() {
        return Err(Stage3Error::FortranDriver(format!(
            "missing libwsjt_fort.a or libwsjt_cxx.a in {}",
            bin_dir.display()
        )));
    }

    let out_dir = cache_dir.join("bin");
    fs::create_dir_all(&out_dir)?;
    let exe = out_dir.join("ft8_stage3_dump");

    let needs_rebuild = if exe.exists() {
        let exe_m = fs::metadata(&exe)?.modified()?;
        let src_m = fs::metadata(&src)?.modified()?;
        let lib_m = fs::metadata(&lib_fort)?.modified()?;
        src_m > exe_m || lib_m > exe_m
    } else {
        true
    };

    if needs_rebuild {
        let ft8_inc = wsjtx_root.join("lib/ft8");
        let output = Command::new("gfortran")
            .args([
                "-O2",
                "-fno-f2c",
                "-fno-second-underscore",
                "-I",
                &ft8_inc.display().to_string(),
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
            return Err(Stage3Error::FortranDriver(format!(
                "gfortran compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }

    Ok(exe)
}

fn verify_suite_b_fixture(
    dump_exe: &Path,
    label: &str,
    samples_i16: &[i16],
    max_samples: usize,
) -> Result<SuiteBFixtureReport, Stage3Error> {
    let tmp = tempfile::tempdir()?;
    let raw_bin = tmp.path().join("raw_i16.bin");
    let dump_dir = tmp.path().join("dumps");
    fs::create_dir_all(&dump_dir)?;

    let mut i16_padded = vec![0i16; NMAX];
    let copy_n = samples_i16.len().min(max_samples).min(NMAX);
    i16_padded[..copy_n].copy_from_slice(&samples_i16[..copy_n]);
    let mut raw_bytes = Vec::with_capacity(NMAX * 2);
    for &s in &i16_padded {
        raw_bytes.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(&raw_bin, &raw_bytes)?;

    let out = Command::new(dump_exe)
        .args([
            raw_bin.display().to_string(),
            dump_dir.display().to_string(),
            "200".to_string(),
            "4000".to_string(),
            "1500".to_string(),
            max_samples.to_string(),
            "N0AAA".to_string(),
            "K9BBB".to_string(),
            "0".to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        return Err(Stage3Error::FortranDriver(format!(
            "ft8_stage3_dump failed on {label}: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }

    let mut dd: Box<[f32; NMAX]> = vec![0.0_f32; NMAX].into_boxed_slice().try_into().unwrap();
    for (dst, &src) in dd.iter_mut().zip(i16_padded.iter()) {
        *dst = f32::from(src);
    }

    let plans = Ft8FftPlans::new();
    let downsampler = Ft8Downsampler::new();
    let sync8d_tables = Sync8dTables::new();
    let subtractor = Ft8Subtractor::new(&plans);

    let sync_bytes = fs::read(dump_dir.join("sync8.bin"))?;
    let mut sr = BinReader::new(&sync_bytes);
    let ncand_f90 = sr.read_i32() as usize;
    let mut cands_f90 = Vec::with_capacity(ncand_f90);
    for _ in 0..ncand_f90 {
        cands_f90.push(ftxr_core::ft8::sync8::Ft8Candidate {
            freq_hz: sr.read_f32(),
            dt_sec: sr.read_f32(),
            sync: sr.read_f32(),
        });
    }

    let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
    let mut snapshot = Ft8SpectrumSnapshot::new();
    downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);
    let mut ft8b_scratch = Ft8bScratch::new(&plans);

    let mut per_cand = Vec::new();
    let mut max_llr_rel_err = 0.0_f64;
    let n_check = ncand_f90.min(10);

    for (idx, cand) in cands_f90.iter().take(n_check).enumerate() {
        let cbytes = fs::read(dump_dir.join(format!("cand3_{:02}.bin", idx + 1)))?;
        let mut cr = BinReader::new(&cbytes);
        let _f1 = cr.read_f32();
        let _xdt = cr.read_f32();
        let _nsync = cr.read_i32();
        let cs_f90 = cr.read_c32_vec(8 * NN);
        let _s8_f90 = cr.read_f32_vec(8 * NN);

        // Run intermediate extraction with depth=3 (may bail on nsync <= 6/7, in which case
        // feed Fortran's `cs` directly or verify `ft8b_scratch.cs`).
        let mut cs_arr = [[Complex32::ZERO; 8]; NN];
        let inter_opt = ft8b_extract_stage2_intermediates(
            &snapshot,
            cand,
            3,
            BitMetricMode::Amplitude,
            &plans,
            &downsampler,
            &sync8d_tables,
            &mut ft8b_scratch,
        );
        if inter_opt.is_some() {
            cs_arr = ft8b_scratch.cs;
        } else {
            for k in 0..NN {
                cs_arr[k].copy_from_slice(&cs_f90[k * 8..(k + 1) * 8]);
            }
        }

        for imetric in [1u8, 2] {
            let bmeta_f90 = cr.read_f32_vec(LDPC_N);
            let bmetb_f90 = cr.read_f32_vec(LDPC_N);
            let bmetc_f90 = cr.read_f32_vec(LDPC_N);
            let bmetd_f90 = cr.read_f32_vec(LDPC_N);
            let bmete_f90 = cr.read_f32_vec(LDPC_N);
            let llra_f90 = cr.read_f32_vec(LDPC_N);
            let llrb_f90 = cr.read_f32_vec(LDPC_N);
            let llrc_f90 = cr.read_f32_vec(LDPC_N);
            let llrd_f90 = cr.read_f32_vec(LDPC_N);
            let llre_f90 = cr.read_f32_vec(LDPC_N);

            let mode = if imetric == 1 {
                BitMetricMode::Amplitude
            } else {
                BitMetricMode::Power
            };
            let m = compute_all_bit_metrics(&cs_arr, mode, &mut ft8b_scratch.s2);

            let bmeta_err = l2_rel_err_f32(&m.bmeta, &bmeta_f90);
            let bmetb_err = l2_rel_err_f32(&m.bmetb, &bmetb_f90);
            let bmetc_err = l2_rel_err_f32(&m.bmetc, &bmetc_f90);
            let bmetd_err = l2_rel_err_f32(&m.bmetd, &bmetd_f90);
            let bmete_err = l2_rel_err_f32(&m.bmete, &bmete_f90);
            let llra_err = l2_rel_err_f32(&m.llrs[0], &llra_f90);
            let llrb_err = l2_rel_err_f32(&m.llrs[1], &llrb_f90);
            let llrc_err = l2_rel_err_f32(&m.llrs[2], &llrc_f90);
            let llrd_err = l2_rel_err_f32(&m.llrs[3], &llrd_f90);
            let llre_err = l2_rel_err_f32(&m.llrs[4], &llre_f90);

            let worst = [
                bmeta_err, bmetb_err, bmetc_err, bmetd_err, bmete_err, llra_err, llrb_err,
                llrc_err, llrd_err, llre_err,
            ]
            .into_iter()
            .fold(0.0_f64, f64::max);
            max_llr_rel_err = max_llr_rel_err.max(worst);

            if worst > MAX_REL_ERR {
                return Err(Stage3Error::GateFailed(format!(
                    "[{label}] cand {idx} imetric={imetric} multi-symbol LLR err={worst:.3e} > {MAX_REL_ERR} (b={bmetb_err:.2e}, c={bmetc_err:.2e}, e={bmete_err:.2e})"
                )));
            }

            per_cand.push(SuiteBMetricErr {
                candidate_idx: idx,
                imetric,
                bmeta_err,
                bmetb_err,
                bmetc_err,
                bmetd_err,
                bmete_err,
                llra_err,
                llrb_err,
                llrc_err,
                llrd_err,
                llre_err,
            });
        }
    }

    let sub_t_path = dump_dir.join("sub_t.bin");
    let sub_refinedt_rel_err = if sub_t_path.exists() {
        let sbytes = fs::read(&sub_t_path)?;
        let mut subr = BinReader::new(&sbytes);
        let _ic = subr.read_i32();
        let f1_sub = subr.read_f32();
        let xdt_sub = subr.read_f32();
        let itone_i32 = subr.read_i32_vec(NN);
        let itone: [u8; NN] = itone_i32
            .iter()
            .map(|&v| v as u8)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let dd_sub_f90 = subr.read_f32_vec(NMAX);

        let mut dd_rs = dd.clone();
        let mut sub_scratch = SubtractScratch::new(&plans);
        subtractor.subtract(
            &mut dd_rs,
            &itone,
            f1_sub,
            xdt_sub,
            true,
            &plans,
            &mut sub_scratch,
        );
        let err = l2_rel_err_f32(&*dd_rs, &dd_sub_f90);
        if err > MAX_REL_ERR {
            return Err(Stage3Error::GateFailed(format!(
                "[{label}] subtractft8(lrefinedt=true) err={err:.3e} > {MAX_REL_ERR}"
            )));
        }
        Some(err)
    } else {
        None
    };

    Ok(SuiteBFixtureReport {
        label: label.to_string(),
        max_samples,
        candidates_checked: n_check,
        max_llr_rel_err,
        sub_refinedt_rel_err,
        per_candidate: per_cand,
    })
}

fn verify_suite_b_apset(
    dump_exe: &Path,
    blank_raw_bin: &Path,
) -> Result<usize, Stage3Error> {
    let cases = [
        ("N0AAA", "K9BBB", ContestMode::None),
        ("PJ4/N0AAA", "K9BBB", ContestMode::None),
        ("N0AAA", "-", ContestMode::None),
        ("N0AAA", "KH1/KH7Z", ContestMode::None),
        ("N0AAA", "KH1/KH7Z", ContestMode::Hound),
        ("-", "K9BBB", ContestMode::None),
    ];

    let tmp = tempfile::tempdir()?;
    for (idx, &(my_c, his_c, contest)) in cases.iter().enumerate() {
        let ddir = tmp.path().join(format!("ap_{idx}"));
        fs::create_dir_all(&ddir)?;
        let out = Command::new(dump_exe)
            .args([
                blank_raw_bin.display().to_string(),
                ddir.display().to_string(),
                "200".to_string(),
                "4000".to_string(),
                "1500".to_string(),
                "0".to_string(),
                my_c.to_string(),
                his_c.to_string(),
                (contest as u8).to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        if !out.status.success() {
            return Err(Stage3Error::FortranDriver(format!(
                "ft8_stage3_dump apset failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }

        let abytes = fs::read(ddir.join("apset.bin"))?;
        let mut ar = BinReader::new(&abytes);
        let apsym_f90: Vec<i8> = ar.read_i32_vec(58).into_iter().map(|v| v as i8).collect();
        let aph10_f90: Vec<i8> = ar.read_i32_vec(10).into_iter().map(|v| v as i8).collect();

        let ht = CallsignHashTable::new();
        let ctx = QsoContext {
            my_call: if my_c == "-" { None } else { Some(my_c.to_string()) },
            his_call: if his_c == "-" { None } else { Some(his_c.to_string()) },
            contest,
            ..Default::default()
        };
        let seed = compute_ap_seed(&ctx, &ht);
        if seed.apsym.as_slice() != apsym_f90.as_slice()
            || seed.aph10.as_slice() != aph10_f90.as_slice()
        {
            return Err(Stage3Error::GateFailed(format!(
                "ft8apset mismatch for ({my_c}, {his_c}, {contest:?}): rust apsym[0]={}, f90={}, rust aph10[0]={}, f90={}",
                seed.apsym[0], apsym_f90[0], seed.aph10[0], aph10_f90[0]
            )));
        }
    }
    Ok(cases.len())
}

pub fn run_suite_b_intermediate_diff(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
) -> Result<SuiteBReport, Stage3Error> {
    let dump_exe = ensure_ft8_stage3_dump_binary(wsjtx_root, rust_ftx_root, bin_dir, cache_dir)?;

    let tmp = tempfile::tempdir()?;
    let zero_bin = tmp.path().join("zeros.bin");
    fs::write(&zero_bin, vec![0u8; NMAX * 2])?;
    let apset_cases_checked = verify_suite_b_apset(&dump_exe, &zero_bin)?;
    println!("[PASS] Suite B [ft8apset]: {apset_cases_checked}/{apset_cases_checked} exact");

    let real_210703 = wav::read_wav_12k_mono(&wsjtx_root.join("samples/FT8/210703_133430.wav"))?;
    let real_191111 = wav::read_wav_12k_mono(&rust_ftx_root.join("fixtures/committed/ft8/191111_110200.wav"))?;

    let fixtures = vec![
        verify_suite_b_fixture(&dump_exe, "210703_133430_180k", &real_210703, NMAX)?,
        verify_suite_b_fixture(&dump_exe, "210703_133430_172k", &real_210703, 172_800)?,
        verify_suite_b_fixture(&dump_exe, "191111_110200_180k", &real_191111, NMAX)?,
    ];
    for f in &fixtures {
        println!(
            "[PASS] Suite B [{}]: {} cands, max_llr_rel_err={:.2e}, sub_refinedt_err={:?}",
            f.label,
            f.candidates_checked,
            f.max_llr_rel_err,
            f.sub_refinedt_rel_err.map(|v| format!("{v:.2e}"))
        );
    }

    Ok(SuiteBReport {
        apset_cases_checked,
        apset_all_exact: true,
        fixtures,
    })
}

fn tally_rust_ap(decs: &[DecodedMessage], counts: &mut BTreeMap<String, usize>) {
    for d in decs {
        let key = match d.ap_tag {
            ApTag::None => "none".to_string(),
            ApTag::Ap { kind, uncertain } => {
                if uncertain {
                    format!("? a{kind}")
                } else {
                    format!("a{kind}")
                }
            }
        };
        *counts.entry(key).or_insert(0) += 1;
    }
}

fn tally_jt9_ap(decs: &[Jt9Decode], counts: &mut BTreeMap<String, usize>) -> Result<(), Stage3Error> {
    for d in decs {
        if d.ap_tag.is_deferred_a7_a8() {
            return Err(Stage3Error::GateFailed(format!(
                "Unexpected deferred {} decode leaked from isolated jt9 run: {:?}",
                d.ap_tag.as_str(),
                d
            )));
        }
        let key = if !d.ap_tag.is_ap() {
            "none".to_string()
        } else if d.ap_uncertain {
            format!("? {}", d.ap_tag.as_str())
        } else {
            d.ap_tag.as_str().to_string()
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    Ok(())
}

fn run_jt9_parallel<T: Sync>(
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    jobs: usize,
    items: &[T],
    make_cfg_and_samples: impl Fn(usize, &T) -> (Jt9RunConfig, Vec<i16>) + Sync,
) -> Result<Vec<Vec<Jt9Decode>>, Stage3Error> {
    let n = items.len();
    let results: Vec<std::sync::Mutex<Option<Vec<Jt9Decode>>>> =
        (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    let next_idx = AtomicUsize::new(0);
    let err_slot: std::sync::Mutex<Option<Stage3Error>> = std::sync::Mutex::new(None);
    let workers = jobs.max(1).min(n.max(1));

    std::thread::scope(|s| {
        for wid in 0..workers {
            let results_ref = &results;
            let next_ref = &next_idx;
            let err_ref = &err_slot;
            let maker = &make_cfg_and_samples;
            s.spawn(move || {
                let Ok(tmp) = tempfile::tempdir() else {
                    return;
                };
                let wav_path = tmp.path().join(format!("000000_{:06}.wav", wid + 1));
                loop {
                    if err_ref.lock().unwrap().is_some() {
                        break;
                    }
                    let idx = next_ref.fetch_add(1, Ordering::Relaxed);
                    if idx >= n {
                        break;
                    }
                    let (cfg, samples) = maker(idx, &items[idx]);
                    if let Err(e) = wav::write_wav_12k_mono(&wav_path, &samples) {
                        *err_ref.lock().unwrap() = Some(Stage3Error::Wav(e));
                        break;
                    }
                    match jt9.run_wav(&wav_path, &cfg, wisdom) {
                        Ok(out) => {
                            *results_ref[idx].lock().unwrap() = Some(out.decodes);
                        }
                        Err(e) => {
                            *err_ref.lock().unwrap() = Some(Stage3Error::Jt9(e));
                            break;
                        }
                    }
                }
            });
        }
    });

    if let Some(e) = err_slot.into_inner().unwrap() {
        return Err(e);
    }
    Ok(results
        .into_iter()
        .map(|m| m.into_inner().unwrap().unwrap_or_default())
        .collect())
}

#[derive(Clone)]
struct CliffSpec {
    name: &'static str,
    message: &'static str,
    fixed_af_hz: Option<f64>,
    qso: QsoContext,
    jt9_profile: Jt9Profile,
    jt9_my: Option<&'static str>,
    jt9_his: Option<&'static str>,
    jt9_prog: Option<u8>,
    jt9_exp: Option<u8>,
    max_delta_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCliffStep {
    point: CliffPoint,
    rust_ap_counts: BTreeMap<String, usize>,
    jt9_ap_counts: BTreeMap<String, usize>,
}

fn run_suite_c_awgn_cliff(
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    trials_rust: usize,
    trials_jt9: usize,
    jobs: usize,
    cliff_filter: Option<&BTreeSet<String>>,
    ctx: &mut ResumeCtx,
) -> Result<SuiteCReport, Stage3Error> {
    let specs = vec![
        CliffSpec {
            name: "blind_cq_ap_noncq",
            message: "N0AAA K9BBB DM79",
            fixed_af_hz: None,
            qso: QsoContext {
                qso_progress: 0,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::BlindCqAp,
            jt9_my: None,
            jt9_his: None,
            jt9_prog: None,
            jt9_exp: None,
            max_delta_db: 0.75,
        },
        CliffSpec {
            name: "blind_cq_ap_cq",
            message: "CQ N0AAA EM10",
            fixed_af_hz: None,
            qso: QsoContext {
                qso_progress: 0,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::BlindCqAp,
            jt9_my: None,
            jt9_his: None,
            jt9_prog: None,
            jt9_exp: None,
            max_delta_db: 0.75,
        },
        CliffSpec {
            name: "qso_ap_a2",
            message: "N0AAA K9BBB DM79",
            fixed_af_hz: None,
            qso: QsoContext {
                my_call: Some("N0AAA".into()),
                qso_progress: 0,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::QsoAp,
            jt9_my: Some("N0AAA"),
            jt9_his: Some("b"),
            jt9_prog: Some(0),
            jt9_exp: None,
            max_delta_db: 0.75,
        },
        CliffSpec {
            name: "qso_ap_a3",
            message: "N0AAA K9BBB -18",
            fixed_af_hz: None,
            qso: QsoContext {
                my_call: Some("N0AAA".into()),
                his_call: Some("K9BBB".into()),
                qso_progress: 2,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::QsoAp,
            jt9_my: Some("N0AAA"),
            jt9_his: Some("K9BBB"),
            jt9_prog: Some(2),
            jt9_exp: None,
            max_delta_db: 0.75,
        },
        CliffSpec {
            name: "qso_ap_a6_rr73",
            message: "N0AAA K9BBB RR73",
            fixed_af_hz: None,
            qso: QsoContext {
                my_call: Some("N0AAA".into()),
                his_call: Some("K9BBB".into()),
                qso_progress: 3,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::QsoAp,
            jt9_my: Some("N0AAA"),
            jt9_his: Some("K9BBB"),
            jt9_prog: Some(3),
            jt9_exp: None,
            max_delta_db: 0.75,
        },
        CliffSpec {
            name: "hound_ap_600hz",
            message: "K9BBB RR73; N0AAA <KH1/KH7Z> -12",
            fixed_af_hz: Some(600.0),
            qso: QsoContext {
                my_call: Some("N0AAA".into()),
                his_call: Some("KH1/KH7Z".into()),
                qso_progress: 1,
                contest: ContestMode::Hound,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::QsoAp,
            jt9_my: Some("N0AAA"),
            jt9_his: Some("KH1/KH7Z"),
            jt9_prog: Some(1),
            jt9_exp: Some(7),
            max_delta_db: 0.75,
        },
        CliffSpec {
            name: "hound_skip_1200hz",
            message: "K9BBB RR73; N0AAA <KH1/KH7Z> -12",
            fixed_af_hz: Some(1200.0),
            qso: QsoContext {
                my_call: Some("N0AAA".into()),
                his_call: Some("KH1/KH7Z".into()),
                qso_progress: 1,
                contest: ContestMode::Hound,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            jt9_profile: Jt9Profile::QsoAp,
            jt9_my: Some("N0AAA"),
            jt9_his: Some("KH1/KH7Z"),
            jt9_prog: Some(1),
            jt9_exp: Some(7),
            max_delta_db: 0.75,
        },
    ];

    let cancel = AtomicBool::new(false);
    let mut rng = ChaCha8Rng::from_seed(derive_seed("v1", "stage3_suite_c", Mode::Ft8, "rng", 0));

    let mut config_summaries = Vec::new();
    for spec in &specs {
        // Consume RNG identically for all configs before any cliff_filter skip so seeds stay invariant.
        let mut per_snr_params: Vec<(i32, Vec<(f64, f64)>)> = Vec::with_capacity(10);
        for snr in (-24..=-15).rev() {
            let mut trial_params = Vec::with_capacity(trials_rust);
            for _t in 0..trials_rust {
                let af_hz = spec
                    .fixed_af_hz
                    .unwrap_or_else(|| rng.gen_range(500.0_f64..2500.0_f64));
                let dt_sec = rng.gen_range(-0.5_f64..0.5_f64);
                trial_params.push((af_hz, dt_sec));
            }
            per_snr_params.push((snr, trial_params));
        }
        if let Some(filter) = cliff_filter {
            if !filter.contains(spec.name) {
                continue;
            }
        }

        let fixed_tpl = spec
            .fixed_af_hz
            .map(|af| build_ft8_unit_template(spec.message, af));
        let mut points = Vec::new();
        let mut rust_ap_counts = BTreeMap::new();
        let mut jt9_ap_counts = BTreeMap::new();

        for (snr, trial_params) in &per_snr_params {
            let snr = *snr;
            let cache_rel = format!(
                "suite_c/{}_snr{snr}_r{trials_rust}_j{trials_jt9}.json",
                spec.name
            );
            let step_res: CachedCliffStep = if let Some(cached) = ctx.load_json(&cache_rel) {
                cached
            } else {
                ctx.check_deadline(&format!("Suite C [{}] SNR {snr} dB", spec.name))?;

                let n_trials = trial_params.len();
                type SynthSlot = std::sync::Mutex<Option<(f64, Vec<i16>, Vec<DecodedMessage>)>>;
                let synth_slots: Vec<SynthSlot> =
                    (0..n_trials).map(|_| std::sync::Mutex::new(None)).collect();
                let next_idx = AtomicUsize::new(0);
                let err_slot: std::sync::Mutex<Option<Stage3Error>> = std::sync::Mutex::new(None);
                let workers = jobs.max(1).min(n_trials.max(1));

                std::thread::scope(|s| {
                    for _ in 0..workers {
                        let out_ref = &synth_slots;
                        let next_ref = &next_idx;
                        let err_ref = &err_slot;
                        let params_ref = trial_params;
                        let cancel_ref = &cancel;
                        let fixed_ref = &fixed_tpl;
                        s.spawn(move || {
                            let mut local_decoder = Ft8Decoder::new();
                            loop {
                                if err_ref.lock().unwrap().is_some() {
                                    break;
                                }
                                let t = next_ref.fetch_add(1, Ordering::Relaxed);
                                if t >= n_trials {
                                    break;
                                }
                                let (af_hz, dt_sec) = params_ref[t];
                                let tpl = fixed_ref
                                    .clone()
                                    .unwrap_or_else(|| build_ft8_unit_template(spec.message, af_hz));
                                let slot_spec = SlotSpec {
                                    mode: Mode::Ft8,
                                    signals: vec![SignalSpec {
                                        message: spec.message.to_string(),
                                        af_hz,
                                        dt_sec,
                                        snr_db: f64::from(snr),
                                        late_start_ms: None,
                                    }],
                                    truncate_ms: None,
                                };
                                let seed = derive_seed(
                                    "v1",
                                    "stage3_cliff",
                                    Mode::Ft8,
                                    &format!("{}_{snr}", spec.name),
                                    t,
                                );
                                let synth = synthesize_slot_from_templates(seed, &slot_spec, &[tpl]);
                                let ht = CallsignHashTable::new();
                                let mut qso = spec.qso.clone();
                                qso.rx_freq_hz = af_hz as f32;
                                let cfg = DecodeConfig {
                                    mode: Mode::Ft8,
                                    depth: 3,
                                    freq_min_hz: 200.0,
                                    freq_max_hz: 3000.0,
                                    rx_freq_hz: af_hz as f32,
                                    qso: Some(qso),
                                    ..DecodeConfig::for_mode(Mode::Ft8)
                                };
                                match local_decoder.decode_i16(
                                    &synth.samples,
                                    &cfg,
                                    &ht,
                                    cancel_ref,
                                    Ft8DecodeOptions::wsjtx_three_stage(),
                                    |_| {},
                                ) {
                                    Ok(decs) => {
                                        *out_ref[t].lock().unwrap() =
                                            Some((af_hz, synth.samples, decs));
                                    }
                                    Err(e) => {
                                        *err_ref.lock().unwrap() = Some(Stage3Error::Core(e));
                                        break;
                                    }
                                }
                            }
                        });
                    }
                });
                if let Some(e) = err_slot.into_inner().unwrap() {
                    return Err(e);
                }

                let mut step_rust_ap = BTreeMap::new();
                let mut rust_decoded = 0usize;
                let mut trial_items = Vec::with_capacity(n_trials);
                for m in synth_slots {
                    let (af_hz, samples, decs) = m.into_inner().unwrap().unwrap();
                    tally_rust_ap(&decs, &mut step_rust_ap);
                    if decs.iter().any(|d| d.message == spec.message) {
                        rust_decoded += 1;
                    }
                    trial_items.push((af_hz, samples));
                }

                let jt9_slice = &trial_items[..trials_jt9.min(trial_items.len())];
                let jt9_outs =
                    run_jt9_parallel(jt9, wisdom, jobs, jt9_slice, |_, (af_hz, samples)| {
                        let mut jcfg = Jt9RunConfig::new(Mode::Ft8, 3, spec.jt9_profile, 1);
                        jcfg.freq_low_hz = 200;
                        jcfg.freq_high_hz = 3000;
                        jcfg.rx_freq_hz = af_hz.round() as u16;
                        jcfg.my_call = spec.jt9_my.map(str::to_string);
                        jcfg.his_call = spec.jt9_his.map(str::to_string);
                        jcfg.qso_progress = spec.jt9_prog;
                        jcfg.nexp_decode = spec.jt9_exp;
                        (jcfg, samples.clone())
                    })?;

                let mut step_jt9_ap = BTreeMap::new();
                let mut jt9_decoded = 0usize;
                for decs in &jt9_outs {
                    tally_jt9_ap(decs, &mut step_jt9_ap)?;
                    if decs.iter().any(|d| d.message == spec.message) {
                        jt9_decoded += 1;
                    }
                }

                let step_val = CachedCliffStep {
                    point: CliffPoint {
                        snr_db: f64::from(snr),
                        rust_trials: trials_rust,
                        rust_decoded,
                        rust_rate: rust_decoded as f64 / trials_rust.max(1) as f64,
                        jt9_trials: jt9_slice.len(),
                        jt9_decoded,
                        jt9_rate: jt9_decoded as f64 / jt9_slice.len().max(1) as f64,
                    },
                    rust_ap_counts: step_rust_ap,
                    jt9_ap_counts: step_jt9_ap,
                };
                ctx.record_step(&cache_rel, &step_val)?;
                step_val
            };

            for (k, v) in step_res.rust_ap_counts {
                *rust_ap_counts.entry(k).or_insert(0) += v;
            }
            for (k, v) in step_res.jt9_ap_counts {
                *jt9_ap_counts.entry(k).or_insert(0) += v;
            }
            points.push(step_res.point);
        }

        let rust_t50 = interpolate_50pct(&points, true);
        let jt9_t50 = interpolate_50pct(&points, false);
        let delta = match (rust_t50, jt9_t50) {
            (Some(r), Some(j)) => Some(r - j),
            _ => None,
        };
        if let Some(d) = delta {
            if d.abs() > spec.max_delta_db {
                return Err(Stage3Error::GateFailed(format!(
                    "Suite C [{}] threshold delta {d:+.2} dB exceeded ±{:.2} dB (rust={rust_t50:?}, jt9={jt9_t50:?})",
                    spec.name, spec.max_delta_db
                )));
            }
        }
        if spec.name == "hound_skip_1200hz"
            && rust_ap_counts.keys().any(|k| k != "none")
        {
            return Err(Stage3Error::GateFailed(format!(
                "Suite C [hound_skip_1200hz] unexpectedly emitted AP decodes above 950 Hz: {rust_ap_counts:?}"
            )));
        }

        println!(
            "[PASS] Suite C [{}]: rust_T50={:?} dB vs jt9_T50={:?} dB (delta={:?} dB), rust_ap={rust_ap_counts:?}, jt9_ap={jt9_ap_counts:?}",
            spec.name,
            rust_t50.map(|v| (v * 100.0).round() / 100.0),
            jt9_t50.map(|v| (v * 100.0).round() / 100.0),
            delta.map(|v| (v * 100.0).round() / 100.0),
        );
        flush_stdout();

        config_summaries.push(CliffConfigSummary {
            name: spec.name.to_string(),
            rust_threshold_50pct_db: rust_t50,
            jt9_threshold_50pct_db: jt9_t50,
            threshold_delta_db: delta,
            rust_ap_counts,
            jt9_ap_counts,
            points,
        });
    }

    Ok(SuiteCReport {
        trials_per_step_rust: trials_rust,
        trials_per_step_jt9: trials_jt9,
        configs: config_summaries,
    })
}

fn match_lists_strict(
    rust: &[DecodedMessage],
    jt9: &[Jt9Decode],
) -> (usize, usize, f32, f32, i32) {
    let mut matched = 0usize;
    let mut within_tol = 0usize;
    let mut max_af = 0.0_f32;
    let mut max_dt = 0.0_f32;
    let mut max_snr = 0_i32;

    let mut used = vec![false; jt9.len()];
    for r in rust {
        let best = jt9
            .iter()
            .enumerate()
            .filter(|(idx, j)| !used[*idx] && j.message == r.message)
            .min_by(|(_, a), (_, b)| {
                (a.af_hz - r.af_hz)
                    .abs()
                    .total_cmp(&(b.af_hz - r.af_hz).abs())
            })
            .map(|(idx, _)| idx);
        if let Some(idx) = best {
            used[idx] = true;
            let j = &jt9[idx];
            let d_af = (r.af_hz - j.af_hz).abs();
            let d_dt = (r.dt_sec - j.dt_sec).abs();
            let d_snr = (i32::from(r.snr_db) - i32::from(j.snr_db)).abs();
            matched += 1;
            if d_af <= 2.0 && d_dt <= 0.1 && d_snr <= 2 {
                within_tol += 1;
            }
            max_af = max_af.max(d_af);
            max_dt = max_dt.max(d_dt);
            max_snr = max_snr.max(d_snr);
        }
    }
    (matched, within_tol, max_af, max_dt, max_snr)
}

fn run_suite_d_crowded_and_corpus(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    jobs: usize,
    ctx: &mut ResumeCtx,
) -> Result<SuiteDReport, Stage3Error> {
    if let Some(cached) = ctx.load_json::<SuiteDReport>("suite_d/report.json") {
        println!(
            "[PASS] Suite D [cached]: crowded={}/{} ({:.1}%), real={}/{} ({:.1}%), tol={:.2}%",
            cached.crowded_rust_3stage_tp,
            cached.crowded_jt9_tp,
            cached.crowded_ratio_3stage * 100.0,
            cached.real_total_rust_3stage,
            cached.real_total_jt9,
            cached.real_ratio_3stage * 100.0,
            cached.matched_within_tol_ratio * 100.0,
        );
        flush_stdout();
        return Ok(cached);
    }
    ctx.check_deadline("Suite D")?;

    let mut decoder = Ft8Decoder::new();
    let cancel = AtomicBool::new(false);

    let mut cfg = DecodeConfig::for_mode(Mode::Ft8);
    cfg.depth = 3;
    cfg.freq_min_hz = 200.0;
    cfg.freq_max_hz = 4000.0;
    cfg.rx_freq_hz = 1500.0;
    cfg.qso = Some(QsoContext {
        qso_progress: 0,
        ap_enabled: true,
        tx_freq_hz: 0.0,
        ..Default::default()
    });

    // 1. Crowded-band synthetic test: 20 slots x 18 overlapping signals spanning +5..-20 dB
    //    with deliberate adjacent 12-28 Hz collisions.
    let call_pairs = [
        ("CQ N0AAA EM10", "K9BBB AA1AA -04"),
        ("CQ DX BB2BB FN20", "VE3CCC DL4DDD R-11"),
        ("JA5EEE VK6FFF RR73", "CQ G7GGG IO91"),
        ("F8HHH EA9III 73", "PY0JJJ N0AAA -12"),
        ("CQ K9BBB DM79", "AA1AA VE3CCC RRR"),
        ("DL4DDD JA5EEE -08", "CQ VK6FFF QF22"),
        ("G7GGG F8HHH R+02", "EA9III PY0JJJ -16"),
        ("CQ AA1AA FN31", "N0AAA DL4DDD R-14"),
        ("BB2BB JA5EEE 73", "CQ VE3CCC EN52"),
    ];
    let mut rng = ChaCha8Rng::from_seed(derive_seed("v1", "stage3_suite_d", Mode::Ft8, "crowd", 0));
    let mut crowded_slots: Vec<(Vec<SignalSpec>, Vec<i16>)> = Vec::with_capacity(20);
    let mut crowded_truth_signals = 0usize;

    for sidx in 0..20 {
        let mut signals = Vec::with_capacity(18);
        let mut templates = Vec::with_capacity(18);
        for (pidx, &(m_strong, m_weak)) in call_pairs.iter().enumerate() {
            let base_af = 350.0 + (pidx as f64) * 260.0 + rng.gen_range(-20.0_f64..20.0_f64);
            let sep = rng.gen_range(12.0_f64..28.0_f64);
            let snr_hi = 5.0 - (pidx as f64) * 1.4;
            let snr_lo = -12.0 - (pidx as f64) * 0.9;
            let dt_a = rng.gen_range(-0.3_f64..0.3_f64);
            let dt_b = rng.gen_range(-0.3_f64..0.3_f64);

            signals.push(SignalSpec {
                message: m_strong.to_string(),
                af_hz: base_af,
                dt_sec: dt_a,
                snr_db: snr_hi,
                late_start_ms: None,
            });
            templates.push(build_ft8_unit_template(m_strong, base_af));
            signals.push(SignalSpec {
                message: m_weak.to_string(),
                af_hz: base_af + sep,
                dt_sec: dt_b,
                snr_db: snr_lo,
                late_start_ms: None,
            });
            templates.push(build_ft8_unit_template(m_weak, base_af + sep));
        }
        crowded_truth_signals += signals.len();
        let slot_spec = SlotSpec {
            mode: Mode::Ft8,
            signals: signals.clone(),
            truncate_ms: None,
        };
        let seed = derive_seed("v1", "stage3_crowded", Mode::Ft8, "slot", sidx);
        let synth = synthesize_slot_from_templates(seed, &slot_spec, &templates);
        crowded_slots.push((signals, synth.samples));
    }

    let jt9_crowded_outs = run_jt9_parallel(jt9, wisdom, jobs, &crowded_slots, |_, (_, samples)| {
        (
            Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::BlindCqAp, 1),
            samples.clone(),
        )
    })?;

    let mut crowded_jt9_tp = 0usize;
    let mut crowded_rust_3stage_tp = 0usize;
    let mut crowded_rust_singleshot_tp = 0usize;
    let mut matched_total = 0usize;
    let mut matched_within_tol = 0usize;
    let mut max_af = 0.0_f32;
    let mut max_dt = 0.0_f32;
    let mut max_snr = 0_i32;

    for ((signals, samples), j_decs) in crowded_slots.iter().zip(jt9_crowded_outs.iter()) {
        let truth_set: BTreeSet<&str> = signals.iter().map(|s| s.message.as_str()).collect();
        let j_filtered: Vec<Jt9Decode> = j_decs
            .iter()
            .filter(|d| !d.ap_tag.is_deferred_a7_a8())
            .cloned()
            .collect();
        crowded_jt9_tp += j_filtered
            .iter()
            .filter(|d| truth_set.contains(d.message.as_str()))
            .count();

        let ht3 = CallsignHashTable::new();
        let r3 = decoder.decode_i16(
            samples,
            &cfg,
            &ht3,
            &cancel,
            Ft8DecodeOptions::wsjtx_three_stage(),
            |_| {},
        )?;
        crowded_rust_3stage_tp += r3
            .iter()
            .filter(|d| truth_set.contains(d.message.as_str()))
            .count();

        let ht1 = CallsignHashTable::new();
        let r1 = decoder.decode_i16(
            samples,
            &cfg,
            &ht1,
            &cancel,
            Ft8DecodeOptions::default(),
            |_| {},
        )?;
        crowded_rust_singleshot_tp += r1
            .iter()
            .filter(|d| truth_set.contains(d.message.as_str()))
            .count();

        let (m, wt, af_d, dt_d, snr_d) = match_lists_strict(&r3, &j_filtered);
        matched_total += m;
        matched_within_tol += wt;
        max_af = max_af.max(af_d);
        max_dt = max_dt.max(dt_d);
        max_snr = max_snr.max(snr_d);
    }

    let crowded_ratio_3stage = crowded_rust_3stage_tp as f64 / crowded_jt9_tp.max(1) as f64;
    let crowded_ratio_singleshot =
        crowded_rust_singleshot_tp as f64 / crowded_jt9_tp.max(1) as f64;
    if crowded_ratio_3stage < 0.95 {
        return Err(Stage3Error::GateFailed(format!(
            "Suite D crowded-band 3-stage ratio {crowded_rust_3stage_tp}/{crowded_jt9_tp} ({crowded_ratio_3stage:.3}) < 0.95"
        )));
    }
    println!(
        "[PASS] Suite D [crowded 20x18]: jt9={crowded_jt9_tp}/{crowded_truth_signals}, rust_3stage={crowded_rust_3stage_tp} ({:.1}%), rust_singleshot={crowded_rust_singleshot_tp} ({:.1}%)",
        crowded_ratio_3stage * 100.0,
        crowded_ratio_singleshot * 100.0,
    );

    // 2. Real-world & committed corpus diff
    let corpus_paths: [(&str, PathBuf); 5] = [
        (
            "wsjtx_sample_ft8_210703_133430",
            wsjtx_root.join("samples/FT8/210703_133430.wav"),
        ),
        (
            "digidx_fixture_ft8_191111_110200",
            rust_ftx_root.join("fixtures/committed/ft8/191111_110200.wav"),
        ),
        (
            "ft8_crowded_20db",
            rust_ftx_root.join("fixtures/committed/ft8/000000_000101.wav"),
        ),
        (
            "ft8_fading_mm",
            rust_ftx_root.join("fixtures/committed/ft8/000000_000102.wav"),
        ),
        (
            "ft8_late_tx_5s",
            rust_ftx_root.join("fixtures/committed/ft8/000000_000103.wav"),
        ),
    ];

    let jcfg = Jt9RunConfig::new(Mode::Ft8, 3, Jt9Profile::BlindCqAp, 1);
    let mut real_corpus = Vec::new();
    let mut real_total_jt9 = 0usize;
    let mut real_total_rust_3stage = 0usize;
    let mut real_total_rust_singleshot = 0usize;

    for (fid, path) in &corpus_paths {
        let samples = wav::read_wav_12k_mono(path)?;
        let jres = jt9.run_wav(path, &jcfg, wisdom)?;
        let mut j_a7_a8 = 0usize;
        let mut j_valid = Vec::new();
        for d in jres.decodes {
            if d.ap_tag.is_deferred_a7_a8() {
                j_a7_a8 += 1;
            } else {
                j_valid.push(d);
            }
        }

        let ht3 = CallsignHashTable::new();
        let t3 = Instant::now();
        let r3 = decoder.decode_i16(
            &samples,
            &cfg,
            &ht3,
            &cancel,
            Ft8DecodeOptions::wsjtx_three_stage(),
            |_| {},
        )?;
        let r3_ms = t3.elapsed().as_secs_f64() * 1000.0;

        let ht1 = CallsignHashTable::new();
        let t1 = Instant::now();
        let r1 = decoder.decode_i16(
            &samples,
            &cfg,
            &ht1,
            &cancel,
            Ft8DecodeOptions::default(),
            |_| {},
        )?;
        let r1_ms = t1.elapsed().as_secs_f64() * 1000.0;

        let (m3, wt3, af3, dt3, snr3) = match_lists_strict(&r3, &j_valid);
        let (m1, _, _, _, _) = match_lists_strict(&r1, &j_valid);
        matched_total += m3;
        matched_within_tol += wt3;
        max_af = max_af.max(af3);
        max_dt = max_dt.max(dt3);
        max_snr = max_snr.max(snr3);

        real_total_jt9 += j_valid.len();
        real_total_rust_3stage += r3.len();
        real_total_rust_singleshot += r1.len();

        println!(
            "[INFO] {fid}: jt9={} ({:.1} ms, excl a7/a8={j_a7_a8}) | rust_3stage={} ({r3_ms:.1} ms, matched={m3}) | rust_1shot={} ({r1_ms:.1} ms, matched={m1})",
            j_valid.len(),
            jres.elapsed_ms,
            r3.len(),
            r1.len(),
        );
        if m3 < j_valid.len() {
            let rmsgs: BTreeSet<&str> = r3.iter().map(|d| d.message.as_str()).collect();
            for j in &j_valid {
                if !rmsgs.contains(j.message.as_str()) {
                    println!(
                        "       [MISS] snr={} dt={:.2} af={:.1} ap={:?} msg={}",
                        j.snr_db, j.dt_sec, j.af_hz, j.wsjtx_metadata(), j.message
                    );
                }
            }
        }

        real_corpus.push(SuiteDFixtureSummary {
            fixture_id: (*fid).to_string(),
            jt9_count: j_valid.len(),
            jt9_a7_a8_excluded: j_a7_a8,
            rust_3stage_count: r3.len(),
            rust_singleshot_count: r1.len(),
            matched_3stage: m3,
            matched_singleshot: m1,
            jt9_elapsed_ms: jres.elapsed_ms,
            rust_3stage_elapsed_ms: r3_ms,
            rust_singleshot_elapsed_ms: r1_ms,
        });
    }

    let real_ratio_3stage = real_total_rust_3stage as f64 / real_total_jt9.max(1) as f64;
    let real_ratio_singleshot = real_total_rust_singleshot as f64 / real_total_jt9.max(1) as f64;
    let matched_within_tol_ratio = matched_within_tol as f64 / matched_total.max(1) as f64;

    if real_ratio_3stage < 0.95 {
        return Err(Stage3Error::GateFailed(format!(
            "Suite D real-world corpus ratio {real_total_rust_3stage}/{real_total_jt9} ({real_ratio_3stage:.3}) < 0.95"
        )));
    }
    if matched_within_tol_ratio < 0.99 {
        return Err(Stage3Error::GateFailed(format!(
            "Suite D measurement parity {matched_within_tol}/{matched_total} ({matched_within_tol_ratio:.3}) < 0.99 (max dAF={max_af:.2}, dDT={max_dt:.2}, dSNR={max_snr})"
        )));
    }

    println!(
        "[PASS] Suite D [real corpus]: 3stage={real_total_rust_3stage}/{real_total_jt9} ({:.1}%), singleshot={real_total_rust_singleshot}/{real_total_jt9} ({:.1}%), measurement_within_tol={matched_within_tol}/{matched_total} ({:.2}%)",
        real_ratio_3stage * 100.0,
        real_ratio_singleshot * 100.0,
        matched_within_tol_ratio * 100.0,
    );

    let report = SuiteDReport {
        crowded_slots: crowded_slots.len(),
        crowded_truth_signals,
        crowded_jt9_tp,
        crowded_rust_3stage_tp,
        crowded_rust_singleshot_tp,
        crowded_ratio_3stage,
        crowded_ratio_singleshot,
        real_corpus,
        real_total_jt9,
        real_total_rust_3stage,
        real_total_rust_singleshot,
        real_ratio_3stage,
        real_ratio_singleshot,
        matched_total,
        matched_within_tol,
        matched_within_tol_ratio,
        max_af_diff_hz: max_af,
        max_dt_diff_sec: max_dt,
        max_snr_diff_db: max_snr,
    };
    ctx.record_step("suite_d/report.json", &report)?;
    flush_stdout();
    Ok(report)
}

/// Exact mirror of `digi-dx/src/domain/ft8.rs::strip_decoder_metadata` and
/// `digi-dx/src/qso/reconciler.rs::confidence_rank` to verify AP tag contract compatibility.
fn digidx_audit_metadata(msg: &str, meta: Option<&str>) -> bool {
    let raw = match meta {
        Some(m) => format!("{msg} {m}"),
        None => msg.to_string(),
    };
    let trimmed = raw.trim();
    let (stripped_msg, stripped_meta) = if let Some(idx) = trimmed.find('?') {
        let m = trimmed[..idx].trim_end();
        let t = trimmed[idx..].trim();
        (m, if t.is_empty() { None } else { Some(t) })
    } else if let Some(last_ws) = trimmed.rfind(char::is_whitespace) {
        let tail = trimmed[last_ws..].trim();
        let bytes = tail.as_bytes();
        let is_ap = (bytes.len() == 2 || bytes.len() == 3)
            && bytes[0].is_ascii_lowercase()
            && bytes[1..].iter().all(u8::is_ascii_digit);
        if is_ap {
            (trimmed[..last_ws].trim_end(), Some(tail))
        } else {
            (trimmed, None)
        }
    } else {
        (trimmed, None)
    };

    let rank = match stripped_meta.map(str::trim).filter(|s| !s.is_empty()) {
        None => 2u8,
        Some(m) if m.contains('?') => 0u8,
        Some(_) => 1u8,
    };
    let expected_rank = match meta {
        None => 2u8,
        Some(m) if m.contains('?') => 0u8,
        Some(_) => 1u8,
    };

    // Note: free-text messages containing literal '?' are not produced with AP metadata,
    // and without metadata only strip at '?' when '?' appears — for standard/CQ AP messages
    // `stripped_msg` must match `msg` and `stripped_meta` must match `meta`.
    if !msg.contains('?') {
        stripped_msg == msg && stripped_meta == meta && rank == expected_rank
    } else {
        meta.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedNoiseChunk {
    r_tot: usize,
    r_ver: usize,
    r_apc: usize,
    r_apu: usize,
    j_tot: usize,
    j_ver: usize,
    j_apc: usize,
    j_apu: usize,
    audit_ok: bool,
}

fn run_suite_e_noise_fp(
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    noise_slots: usize,
    jobs: usize,
    ctx: &mut ResumeCtx,
) -> Result<SuiteEReport, Stage3Error> {
    let cancel = AtomicBool::new(false);

    struct NoiseCfg {
        name: &'static str,
        qso: QsoContext,
        run_jt9: bool,
        jt9_profile: Jt9Profile,
        jt9_my: Option<&'static str>,
        jt9_his: Option<&'static str>,
        jt9_prog: Option<u8>,
    }

    let cfgs = vec![
        NoiseCfg {
            name: "no_ap",
            qso: QsoContext {
                ap_enabled: false,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            run_jt9: true,
            jt9_profile: Jt9Profile::BlindNoAp,
            jt9_my: None,
            jt9_his: None,
            jt9_prog: None,
        },
        NoiseCfg {
            name: "blind_cq_ap",
            qso: QsoContext {
                qso_progress: 0,
                ap_enabled: true,
                tx_freq_hz: 0.0,
                ..Default::default()
            },
            run_jt9: true,
            jt9_profile: Jt9Profile::BlindCqAp,
            jt9_my: None,
            jt9_his: None,
            jt9_prog: None,
        },
        NoiseCfg {
            name: "full_qso_ap_a3_a6",
            qso: QsoContext {
                my_call: Some("N0AAA".into()),
                his_call: Some("K9BBB".into()),
                qso_progress: 3,
                rx_freq_hz: 1500.0,
                tx_freq_hz: 0.0,
                ap_width_hz: 75.0,
                ap_enabled: true,
                ..Default::default()
            },
            run_jt9: true,
            jt9_profile: Jt9Profile::QsoAp,
            jt9_my: Some("N0AAA"),
            jt9_his: Some("K9BBB"),
            jt9_prog: Some(3),
        },
    ];

    let mut summaries = Vec::new();
    let mut audit_ok = true;
    let mut deferred_err: Option<Stage3Error> = None;

    for c in &cfgs {
        let mut r_tot = 0usize;
        let mut r_ver = 0usize;
        let mut r_apc = 0usize;
        let mut r_apu = 0usize;
        let mut j_tot = 0usize;
        let mut j_ver = 0usize;
        let mut j_apc = 0usize;
        let mut j_apu = 0usize;

        const CHUNK: usize = 100;
        let mut chunk_start = 0usize;
        while chunk_start < noise_slots || (noise_slots == 0 && chunk_start == 0) {
            let chunk_end = (chunk_start + CHUNK).min(noise_slots);
            let cache_rel = format!("suite_e/{}_s{chunk_start}_e{chunk_end}.json", c.name);
            let chunk_res: CachedNoiseChunk = if let Some(cached) = ctx.load_json(&cache_rel) {
                cached
            } else {
                ctx.check_deadline(&format!("Suite E [{}] slots {chunk_start}..{chunk_end}", c.name))?;

                let slots: Vec<(usize, Vec<i16>)> = (chunk_start..chunk_end)
                    .map(|idx| {
                        let seed = derive_seed("v1", "stage3_noise", Mode::Ft8, "awgn", idx);
                        let mut nrng = GaussianRng::from_seed(seed);
                        let samples: Vec<i16> = (0..NMAX)
                            .map(|_| (nrng.next_gaussian() * 200.0).round().clamp(-32_767.0, 32_767.0) as i16)
                            .collect();
                        (idx, samples)
                    })
                    .collect();

                let n_slots = slots.len();
                let slot_outs: Vec<std::sync::Mutex<Option<Vec<DecodedMessage>>>> =
                    (0..n_slots).map(|_| std::sync::Mutex::new(None)).collect();
                let next_idx = AtomicUsize::new(0);
                let err_slot: std::sync::Mutex<Option<Stage3Error>> = std::sync::Mutex::new(None);
                let workers = jobs.max(1).min(n_slots.max(1));

                if n_slots > 0 {
                    std::thread::scope(|s| {
                        for _ in 0..workers {
                            let outs_ref = &slot_outs;
                            let next_ref = &next_idx;
                            let err_ref = &err_slot;
                            let slots_ref = &slots;
                            let cancel_ref = &cancel;
                            s.spawn(move || {
                                let mut local_decoder = Ft8Decoder::new();
                                let dcfg = DecodeConfig {
                                    mode: Mode::Ft8,
                                    depth: 3,
                                    enable_telemetry: true,
                                    qso: Some(c.qso.clone()),
                                    ..DecodeConfig::for_mode(Mode::Ft8)
                                };
                                loop {
                                    if err_ref.lock().unwrap().is_some() {
                                        break;
                                    }
                                    let idx = next_ref.fetch_add(1, Ordering::Relaxed);
                                    if idx >= n_slots {
                                        break;
                                    }
                                    let ht = CallsignHashTable::new();
                                    match local_decoder.decode_i16(
                                        &slots_ref[idx].1,
                                        &dcfg,
                                        &ht,
                                        cancel_ref,
                                        Ft8DecodeOptions::wsjtx_three_stage(),
                                        |_| {},
                                    ) {
                                        Ok(decs) => *outs_ref[idx].lock().unwrap() = Some(decs),
                                        Err(e) => {
                                            *err_ref.lock().unwrap() = Some(Stage3Error::Core(e));
                                            break;
                                        }
                                    }
                                }
                            });
                        }
                    });
                }
                if let Some(e) = err_slot.into_inner().unwrap() {
                    return Err(e);
                }

                let mut cr_tot = 0usize;
                let mut cr_ver = 0usize;
                let mut cr_apc = 0usize;
                let mut cr_apu = 0usize;
                let mut ca_ok = true;
                for ((slot_idx, _), m) in slots.iter().zip(slot_outs) {
                    let decs = m.into_inner().unwrap().unwrap_or_default();
                    for d in &decs {
                        cr_tot += 1;
                        eprintln!(
                            "[FP_DEBUG rust {} slot={}] msg={:?} af={:.1} dt={:.2} snr={} ap={:?} tel={:?}",
                            c.name, slot_idx, d.message, d.af_hz, d.dt_sec, d.snr_db, d.ap_tag, d.telemetry
                        );
                        if !digidx_audit_metadata(&d.message, d.wsjtx_metadata().as_deref()) {
                            ca_ok = false;
                        }
                        match d.ap_tag {
                            ApTag::None => cr_ver += 1,
                            ApTag::Ap { uncertain: false, .. } => cr_apc += 1,
                            ApTag::Ap { uncertain: true, .. } => cr_apu += 1,
                        }
                    }
                }

                let mut cj_tot = 0usize;
                let mut cj_ver = 0usize;
                let mut cj_apc = 0usize;
                let mut cj_apu = 0usize;
                if c.run_jt9 && !slots.is_empty() {
                    let j_outs = run_jt9_parallel(jt9, wisdom, jobs, &slots, |_, (_, samples)| {
                        let mut jcfg = Jt9RunConfig::new(Mode::Ft8, 3, c.jt9_profile, 1);
                        jcfg.my_call = c.jt9_my.map(str::to_string);
                        jcfg.his_call = c.jt9_his.map(str::to_string);
                        jcfg.qso_progress = c.jt9_prog;
                        (jcfg, samples.clone())
                    })?;
                    for ((slot_idx, _), decs) in slots.iter().zip(j_outs) {
                        for d in decs {
                            if d.ap_tag.is_deferred_a7_a8() {
                                return Err(Stage3Error::GateFailed(format!(
                                    "Suite E leaked deferred {:?} from jt9",
                                    d.ap_tag
                                )));
                            }
                            cj_tot += 1;
                            eprintln!(
                                "[FP_DEBUG jt9  {} slot={}] msg={:?} af={:.1} dt={:.2} snr={} ap={:?} unc={}",
                                c.name, slot_idx, d.message, d.af_hz, d.dt_sec, d.snr_db, d.ap_tag, d.ap_uncertain
                            );
                            if !d.ap_tag.is_ap() {
                                cj_ver += 1;
                            } else if d.ap_uncertain {
                                cj_apu += 1;
                            } else {
                                cj_apc += 1;
                            }
                        }
                    }
                }

                let chunk_val = CachedNoiseChunk {
                    r_tot: cr_tot,
                    r_ver: cr_ver,
                    r_apc: cr_apc,
                    r_apu: cr_apu,
                    j_tot: cj_tot,
                    j_ver: cj_ver,
                    j_apc: cj_apc,
                    j_apu: cj_apu,
                    audit_ok: ca_ok,
                };
                ctx.record_step(&cache_rel, &chunk_val)?;
                chunk_val
            };

            r_tot += chunk_res.r_tot;
            r_ver += chunk_res.r_ver;
            r_apc += chunk_res.r_apc;
            r_apu += chunk_res.r_apu;
            j_tot += chunk_res.j_tot;
            j_ver += chunk_res.j_ver;
            j_apc += chunk_res.j_apc;
            j_apu += chunk_res.j_apu;
            audit_ok &= chunk_res.audit_ok;

            if noise_slots == 0 {
                break;
            }
            chunk_start = chunk_end;
        }

        println!(
            "[PASS] Suite E [{}]: {noise_slots} slots | rust total={r_tot} (ver={r_ver}, a*={r_apc}, ?a*={r_apu}) | jt9 total={j_tot} (ver={j_ver}, a*={j_apc}, ?a*={j_apu})",
            c.name
        );
        flush_stdout();

        if c.name == "no_ap" && (r_tot as f64 / noise_slots.max(1) as f64) >= 0.002 && r_tot > j_tot {
            deferred_err.get_or_insert_with(|| {
                Stage3Error::GateFailed(format!(
                    "Suite E [no_ap] FP rate too high: rust={r_tot}/{noise_slots} vs jt9={j_tot}/{noise_slots}"
                ))
            });
        }
        if c.run_jt9 && r_ver > j_ver + 1 {
            deferred_err.get_or_insert_with(|| {
                Stage3Error::GateFailed(format!(
                    "Suite E [{}] verified FPs exceeded jt9: rust={r_ver} vs jt9={j_ver}",
                    c.name
                ))
            });
        }
        if c.run_jt9 && r_tot > j_tot + 3 {
            deferred_err.get_or_insert_with(|| {
                Stage3Error::GateFailed(format!(
                    "Suite E [{}] total FPs exceeded jt9 tolerance: rust={r_tot} vs jt9={j_tot}",
                    c.name
                ))
            });
        }

        summaries.push(SuiteEConfigSummary {
            name: c.name.to_string(),
            slots: noise_slots,
            rust_fps_total: r_tot,
            rust_fps_verified: r_ver,
            rust_fps_ap_certain: r_apc,
            rust_fps_ap_uncertain: r_apu,
            jt9_fps_total: j_tot,
            jt9_fps_verified: j_ver,
            jt9_fps_ap_certain: j_apc,
            jt9_fps_ap_uncertain: j_apu,
        });
    }
    if let Some(e) = deferred_err {
        return Err(e);
    }

    // Explicit synthetic boundary audit for AP tags matching digi-dx reconciler contract.
    for kind in 1..=6u8 {
        assert!(digidx_audit_metadata(
            "N0AAA K9BBB RR73",
            ApTag::from_wsjtx(kind, 0.25).wsjtx_metadata().as_deref()
        ));
        assert!(digidx_audit_metadata(
            "N0AAA K9BBB RR73",
            ApTag::from_wsjtx(kind, 0.16).wsjtx_metadata().as_deref()
        ));
    }
    assert!(digidx_audit_metadata("CQ N0AAA EM10", None));
    if !audit_ok {
        return Err(Stage3Error::GateFailed(
            "Suite E digi-dx AP tag audit failed".into(),
        ));
    }

    Ok(SuiteEReport {
        configs: summaries,
        digidx_metadata_audit_passed: audit_ok,
    })
}

pub fn run_stage3_suites(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
    jt9: &Jt9Runner,
    opts: &Stage3Options,
    out_json: Option<&Path>,
) -> Result<Stage3Scorecard, Stage3Error> {
    if opts.cliff_configs.is_some() && out_json.is_some() {
        return Err(Stage3Error::GateFailed(
            "--out cannot be combined with partial --cliff-configs".into(),
        ));
    }

    let ft8_warmup = wsjtx_root.join("samples/FT8/210703_133430.wav");
    let ft4_warmup = wsjtx_root.join("samples/FT4/000000_000002.wav");
    let wisdom = jt9.ensure_warm_wisdom(1, &ft8_warmup, &ft4_warmup)?;

    let mut resume_ctx = ResumeCtx::new(
        opts.resume_cache.then(|| cache_dir.join("stage3-resume")),
        opts.max_wall_secs,
    );

    let want = |s: &str| opts.suites.contains("all") || opts.suites.contains(s);

    let suite_a = if want("a") {
        Some(run_suite_a_osd_oracle(rust_ftx_root, bin_dir, cache_dir, None)?)
    } else {
        None
    };

    let suite_b = if want("b") {
        Some(run_suite_b_intermediate_diff(
            wsjtx_root,
            rust_ftx_root,
            bin_dir,
            cache_dir,
        )?)
    } else {
        None
    };

    let suite_c = if want("c") {
        Some(run_suite_c_awgn_cliff(
            jt9,
            wisdom.as_deref(),
            opts.trials,
            opts.jt9_trials,
            opts.jobs,
            opts.cliff_configs.as_ref(),
            &mut resume_ctx,
        )?)
    } else {
        None
    };

    let suite_d = if want("d") {
        Some(run_suite_d_crowded_and_corpus(
            wsjtx_root,
            rust_ftx_root,
            jt9,
            wisdom.as_deref(),
            opts.jobs,
            &mut resume_ctx,
        )?)
    } else {
        None
    };

    let suite_e = if want("e") {
        Some(run_suite_e_noise_fp(
            jt9,
            wisdom.as_deref(),
            opts.noise_slots,
            opts.jobs,
            &mut resume_ctx,
        )?)
    } else {
        None
    };

    let scorecard = Stage3Scorecard {
        suite_a,
        suite_b,
        suite_c,
        suite_d,
        suite_e,
    };

    if let Some(out_path) = out_json {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(
            out_path,
            format!("{}\n", serde_json::to_string_pretty(&scorecard)?),
        )?;
        println!("Wrote Stage 3 parity scorecard to {}", out_path.display());
        flush_stdout();
    }

    Ok(scorecard)
}
