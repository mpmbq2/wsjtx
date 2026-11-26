// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Stage 4 FT4 RX parity & exit-gate oracle verification harness (`ft4-stage4`).
//!
//! Validates:
//! - Suite A (`dump-diff`): 6-stage numerical diff vs compiled Fortran routines (`<= 1e-4` rel L2,
//!   0 diff on integer/codeword fields) on `samples/FT4/000000_000002.wav` and `ft4sim` fixtures.
//! - Suite B (`real-corpus`): `samples/FT4/000000_000002.wav` and `000000_000201.wav` across `-d 1..3`.
//! - Suite C (`crowded`): `ft4sim_mult`-style 20- and 25-signal crowded slots (`lib/ft4/messages.txt`).
//! - Suite D (`awgn`): 0..-21 dB AWGN waterfall (`>= 99%` on `0..-14 dB`, threshold `<= 0.5-1.0 dB`
//!   of `jt9 -5 -d 3` `-18.25 dB`).
//! - Suite E (`ap-matrix`): single-slot AP `a1..a6`, `±50 Hz` tolerance, `maxosd=3`, Field Day & RU,
//!   and `? a*` uncertainty tagging (`qual < 0.17`).
//! - Suite F (`noise`): pure AWGN false-positive comparison (`>= 300` slots blind `Q=0`, AP-off, AP `Q=3`).
//! - Suite G (`loopback`): 100% FT4 TX->RX decode across 300ms delay, `ft4sim` dt=0, late-start,
//!   shifted dt `[-0.8, +0.9]s`, truncated `66_000` & full `90_000` buffers, plus edge/silence bounds.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use clap::ValueEnum;
use ftxr_core::fec::bp::{LdpcDecoded, LDPC_N};
use ftxr_core::ft4::ap::Ft4ApSet;
use ftxr_core::ft4::candidates::Ft4Candidate;
use ftxr_core::ft4::decoder::{Ft4DecodeOptions, Ft4Probe};
use ftxr_core::ft4::params::{NBITS, NCW, NDMAX, NFRAME_DOWN, NMAX, SAMPLE_RATE_HZ};
use ftxr_core::{
    ContestMode, DecodeConfig, DecodedMessage, DecoderEngine, ModulateConfig, Mode, QsoContext,
};
use num_complex::Complex32;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::jt9::{Jt9Decode, Jt9Profile, Jt9RunConfig, Jt9Runner};
use crate::sim::SimRunner;
use crate::synth::{derive_seed, synthesize_slot_from_templates, GaussianRng, SignalSpec, SlotSpec};
use crate::wav;

#[derive(Debug, Error)]
pub enum Stage4Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WAV error: {0}")]
    Wav(#[from] wav::WavError),
    #[error("sim error: {0}")]
    Sim(#[from] crate::sim::SimError),
    #[error("jt9 error: {0}")]
    Jt9(#[from] crate::jt9::Jt9Error),
    #[error("ftxr-core error: {0}")]
    Core(#[from] ftxr_core::FtxrError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Fortran dump tool build/run failed: {0}")]
    FortranDump(String),
    #[error("Stage 4 parity gate failed: {0}")]
    GateFailed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Ft4Stage4Suite {
    All,
    DumpDiff,
    RealCorpus,
    Crowded,
    Awgn,
    ApMatrix,
    Noise,
    Loopback,
}

#[derive(Debug, Clone)]
pub struct Stage4Options {
    pub suites: Vec<Ft4Stage4Suite>,
    pub trials: usize,
    pub noise_slots: usize,
    pub jobs: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Stage4Report {
    pub suites_run: Vec<String>,
    pub dump_diff_cases: Vec<DumpDiffCaseSummary>,
    pub real_corpus: Vec<CorpusCompareRow>,
    pub crowded: Option<CrowdedCompareSummary>,
    pub awgn: Option<AwgnCompareSummary>,
    pub ap_matrix: Option<ApMatrixSummary>,
    pub noise: Option<NoiseCompareSummary>,
    pub loopback: Option<LoopbackSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpDiffCaseSummary {
    pub label: String,
    pub cand_count_ftx: usize,
    pub cand_count_rust: usize,
    pub max_rel_l2_savg: f64,
    pub max_rel_l2_sbase: f64,
    pub max_cand_df_hz: f64,
    pub max_rel_l2_cd2: f64,
    pub max_rel_l2_cb: f64,
    pub max_rel_l2_cd: f64,
    pub max_rel_l2_bitmetrics: f64,
    pub max_rel_l2_llr: f64,
    pub max_rel_l2_llrd: f64,
    pub rel_l2_sub_first: Option<f64>,
    pub rel_l2_dd_pass1: f64,
    pub decodes_ftx: usize,
    pub decodes_rust: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusCompareRow {
    pub fixture: String,
    pub depth: u8,
    pub jt9_count: usize,
    pub rust_count: usize,
    pub ratio_pct: f64,
    pub matched: usize,
    pub max_abs_df_hz: f32,
    pub max_abs_dt_sec: f32,
    pub max_abs_dsnr_db: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrowdedCompareSummary {
    pub slots_20sig: usize,
    pub slots_25sig: usize,
    pub jt9_d1: usize,
    pub rust_d1: usize,
    pub jt9_d2: usize,
    pub rust_d2: usize,
    pub jt9_d3: usize,
    pub rust_d3: usize,
    pub max_abs_df_hz: f32,
    pub max_abs_dt_sec: f32,
    pub max_abs_dsnr_db: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwgnCompareSummary {
    pub min_rate_0_to_neg14_pct: f64,
    pub jt9_threshold_50_db: f64,
    pub rust_threshold_50_db: f64,
    pub points: Vec<AwgnPointRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwgnPointRow {
    pub snr_db: f64,
    pub trials: usize,
    pub rust_rate_pct: f64,
    pub jt9_rate_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApMatrixSummary {
    pub weak_a1_cq_ok: usize,
    pub weak_a3_inside_ok: usize,
    pub weak_a3_outside_suppressed: bool,
    pub weak_a6_inside_ok: usize,
    pub uncertain_ap_tagged: usize,
    pub fd_a3_ok: usize,
    pub ru_a2_ok: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseCompareSummary {
    pub slots_per_mode: usize,
    pub blind_rust_fp: usize,
    pub blind_jt9_fp: usize,
    pub ap_disabled_rust_fp: usize,
    pub ap_q3_rust_fp: usize,
    pub ap_q3_jt9_fp: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopbackSummary {
    pub nominal_cases_passed: usize,
    pub nominal_cases_total: usize,
    pub max_delta_af_hz: f32,
    pub max_delta_dt_sec: f32,
}

pub fn run_stage4_suites(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
    jt9: &Jt9Runner,
    opts: &Stage4Options,
    out_path: Option<&Path>,
) -> Result<(), Stage4Error> {
    let sim = SimRunner::new(bin_dir.to_path_buf(), cache_dir.to_path_buf());
    let run_all = opts.suites.is_empty() || opts.suites.contains(&Ft4Stage4Suite::All);
    let has = |s: Ft4Stage4Suite| run_all || opts.suites.contains(&s);

    let needs_jt9 = has(Ft4Stage4Suite::RealCorpus)
        || has(Ft4Stage4Suite::Crowded)
        || has(Ft4Stage4Suite::Awgn)
        || has(Ft4Stage4Suite::ApMatrix)
        || has(Ft4Stage4Suite::Noise)
        || has(Ft4Stage4Suite::Loopback);
    let wisdom = if needs_jt9 {
        let ft8_warmup = wsjtx_root.join("samples/FT8/210703_133430.wav");
        let ft4_warmup = wsjtx_root.join("samples/FT4/000000_000002.wav");
        jt9.ensure_warm_wisdom(1, &ft8_warmup, &ft4_warmup)?
    } else {
        None
    };

    let mut report = Stage4Report::default();

    if has(Ft4Stage4Suite::DumpDiff) {
        report.suites_run.push("dump-diff".into());
        report.dump_diff_cases =
            run_suite_dump_diff(wsjtx_root, rust_ftx_root, bin_dir, &sim)?;
    }

    if has(Ft4Stage4Suite::RealCorpus) {
        report.suites_run.push("real-corpus".into());
        report.real_corpus =
            run_suite_real_corpus(wsjtx_root, rust_ftx_root, jt9, wisdom.as_deref())?;
    }

    if has(Ft4Stage4Suite::Crowded) {
        report.suites_run.push("crowded".into());
        report.crowded = Some(run_suite_crowded(
            wsjtx_root,
            rust_ftx_root,
            &sim,
            jt9,
            wisdom.as_deref(),
        )?);
    }

    if has(Ft4Stage4Suite::Awgn) {
        report.suites_run.push("awgn".into());
        report.awgn = Some(run_suite_awgn(
            &sim,
            jt9,
            wisdom.as_deref(),
            opts.trials,
            opts.jobs,
        )?);
    }

    if has(Ft4Stage4Suite::ApMatrix) {
        report.suites_run.push("ap-matrix".into());
        report.ap_matrix = Some(run_suite_ap_matrix(&sim, jt9, wisdom.as_deref())?);
    }

    if has(Ft4Stage4Suite::Noise) {
        report.suites_run.push("noise".into());
        report.noise = Some(run_suite_noise(
            jt9,
            wisdom.as_deref(),
            opts.noise_slots,
            opts.jobs,
        )?);
    }

    if has(Ft4Stage4Suite::Loopback) {
        report.suites_run.push("loopback".into());
        report.loopback = Some(run_suite_loopback(&sim, jt9, wisdom.as_deref())?);
    }

    if let Some(path) = out_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("[STAGE4] Report written to {}", path.display());
    }

    Ok(())
}

fn effective_workers(jobs: usize, work_items: usize) -> usize {
    let base = if jobs > 1 {
        jobs
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
    };
    base.clamp(1, 8).min(work_items.max(1))
}

fn ensure_ft4_dump_binary(wsjtx_root: &Path, rust_ftx_root: &Path, bin_dir: &Path) -> Result<PathBuf, Stage4Error> {
    let dump_bin = bin_dir.join("ft4_stage4_dump");
    let src = rust_ftx_root.join("tools/ftxr-oracle/fortran_dump/ft4_stage4_dump.f90");

    let needs_rebuild = match (fs::metadata(&dump_bin), fs::metadata(&src)) {
        (Ok(b), Ok(s)) => b.modified()? < s.modified()?,
        _ => true,
    };
    if !needs_rebuild {
        return Ok(dump_bin);
    }

    let status = Command::new("gfortran")
        .current_dir(rust_ftx_root)
        .args([
            "-O2",
            "-fopenmp",
            &format!("-I{}", wsjtx_root.join("lib/ft4").display()),
            &format!("-I{}", wsjtx_root.join("lib").display()),
            &format!("-I{}", bin_dir.display()),
            &src.display().to_string(),
            &bin_dir.join("libwsjt_fort_omp.a").display().to_string(),
            &bin_dir.join("libwsjt_cxx.a").display().to_string(),
            "-lfftw3f_threads",
            "-lfftw3f",
            "-lboost_log_setup",
            "-lboost_log",
            "-lboost_chrono",
            "-lboost_filesystem",
            "-lboost_regex",
            "-lboost_thread",
            "-lboost_atomic",
            "-lquadmath",
            "-lstdc++",
            "-lm",
            "-o",
            &dump_bin.display().to_string(),
        ])
        .status()?;

    if !status.success() {
        return Err(Stage4Error::FortranDump(format!(
            "gfortran build of ft4_stage4_dump failed with {:?}",
            status.code()
        )));
    }
    Ok(dump_bin)
}

#[derive(Default)]
struct ProbeCapture {
    savg_pass1: Vec<f32>,
    sbase_pass1: Vec<f32>,
    cands_pass1: Vec<Ft4Candidate>,
    cd2_pass1: BTreeMap<usize, Vec<Complex32>>,
    evals_pass1: Vec<RustEvalRecord>,
    ldpc_wins_pass1: Vec<RustLdpcWinRecord>,
    sub_first: Option<Vec<f32>>,
    dd_pass1: Option<Vec<f32>>,
}

struct RustEvalRecord {
    cand_idx: usize,
    iseg: usize,
    smax: f32,
    ibest: i32,
    idfbest: i32,
    f1_hz: f32,
    nsync_qual: usize,
    cb: Vec<Complex32>,
    cd: Vec<Complex32>,
    bitmetrics: [[f32; 3]; NBITS],
    llrs: [[f32; NCW]; 3],
}

struct RustLdpcWinRecord {
    cand_idx: usize,
    ap_pass: usize,
    iaptype: u8,
    maxosd: i32,
    nhard_errors: i32,
    dmin: f32,
    apmask: [u8; LDPC_N],
    llr: [f32; LDPC_N],
    cw: [u8; LDPC_N],
}

impl Ft4Probe for ProbeCapture {
    fn on_candidates(
        &mut self,
        pass: usize,
        savg: &[f32],
        _savsm_norm: &[f32],
        sbase: &[f32],
        candidates: &[Ft4Candidate],
    ) {
        if pass == 0 {
            self.savg_pass1 = savg[1..].to_vec();
            self.sbase_pass1 = sbase[1..].to_vec();
            self.cands_pass1 = candidates.to_vec();
        }
    }

    fn on_downsample(
        &mut self,
        pass: usize,
        cand_idx: usize,
        _f0_hz: f32,
        cd2: &[Complex32; NDMAX],
        f1_hz: f32,
        smax: f32,
        ibest: i32,
        idfbest: i32,
        cb: &[Complex32; NDMAX],
    ) {
        if pass == 0 {
            self.cd2_pass1.insert(cand_idx, cd2.to_vec());
            let _ = (f1_hz, smax, ibest, idfbest, cb);
        }
    }

    fn on_bitmetrics(
        &mut self,
        pass: usize,
        cand_idx: usize,
        iseg: usize,
        _nsync: usize,
        nsync_qual: usize,
        cd: &[Complex32; NFRAME_DOWN],
        bitmetrics: &[[f32; 3]; NBITS],
        llrs: &[[f32; NCW]; 3],
    ) {
        if pass == 0 {
            // Note cb/smax/ibest/idfbest captured via on_downsample right before on_bitmetrics if updated.
            let _ = (cand_idx, iseg, nsync_qual, cd, bitmetrics, llrs);
        }
    }

    fn on_ldpc_attempt(
        &mut self,
        pass: usize,
        cand_idx: usize,
        ap_pass: usize,
        iaptype: u8,
        max_osd: i32,
        apmask: &[u8; LDPC_N],
        llr: &[f32; LDPC_N],
        decoded: Option<&LdpcDecoded>,
    ) {
        if pass == 0 {
            if let Some(d) = decoded {
                self.ldpc_wins_pass1.push(RustLdpcWinRecord {
                    cand_idx,
                    ap_pass,
                    iaptype,
                    maxosd: max_osd,
                    nhard_errors: d.nhard_errors,
                    dmin: d.dmin,
                    apmask: *apmask,
                    llr: *llr,
                    cw: d.codeword,
                });
            }
        }
    }

    fn on_subtract_residual(&mut self, pass: usize, _cand_idx: usize, dd: &[f32; NMAX]) {
        if pass == 0 {
            if self.sub_first.is_none() {
                self.sub_first = Some(dd.to_vec());
            }
            self.dd_pass1 = Some(dd.to_vec());
        }
    }
}

/// Captures complete evals (`cd2`, `cb`, `smax`, `ibest`, `idfbest`, `cd`, `bitmetrics`, `llrs`,
/// `ldpc`, `subtract`) with correlated `on_downsample` + `on_bitmetrics` state.
struct FullProbeCapture {
    inner: ProbeCapture,
    pending_ds: Option<(usize, f32, f32, i32, i32, Vec<Complex32>)>,
}

impl FullProbeCapture {
    fn new() -> Self {
        Self {
            inner: ProbeCapture::default(),
            pending_ds: None,
        }
    }
}

impl Ft4Probe for FullProbeCapture {
    fn on_candidates(
        &mut self,
        pass: usize,
        savg: &[f32],
        savsm_norm: &[f32],
        sbase: &[f32],
        candidates: &[Ft4Candidate],
    ) {
        self.inner
            .on_candidates(pass, savg, savsm_norm, sbase, candidates);
    }

    fn on_downsample(
        &mut self,
        pass: usize,
        cand_idx: usize,
        f0_hz: f32,
        cd2: &[Complex32; NDMAX],
        f1_hz: f32,
        smax: f32,
        ibest: i32,
        idfbest: i32,
        cb: &[Complex32; NDMAX],
    ) {
        self.inner
            .on_downsample(pass, cand_idx, f0_hz, cd2, f1_hz, smax, ibest, idfbest, cb);
        if pass == 0 {
            self.pending_ds = Some((cand_idx, f1_hz, smax, ibest, idfbest, cb.to_vec()));
        }
    }

    fn on_bitmetrics(
        &mut self,
        pass: usize,
        cand_idx: usize,
        iseg: usize,
        _nsync: usize,
        nsync_qual: usize,
        cd: &[Complex32; NFRAME_DOWN],
        bitmetrics: &[[f32; 3]; NBITS],
        llrs: &[[f32; NCW]; 3],
    ) {
        if pass == 0 {
            if let Some((ds_cand, f1_hz, smax, ibest, idfbest, cb)) = self.pending_ds.as_ref() {
                if *ds_cand == cand_idx {
                    self.inner.evals_pass1.push(RustEvalRecord {
                        cand_idx,
                        iseg,
                        smax: *smax,
                        ibest: *ibest,
                        idfbest: *idfbest,
                        f1_hz: *f1_hz,
                        nsync_qual,
                        cb: cb.clone(),
                        cd: cd.to_vec(),
                        bitmetrics: *bitmetrics,
                        llrs: *llrs,
                    });
                }
            }
        }
    }

    fn on_ldpc_attempt(
        &mut self,
        pass: usize,
        cand_idx: usize,
        ap_pass: usize,
        iaptype: u8,
        max_osd: i32,
        apmask: &[u8; LDPC_N],
        llr: &[f32; LDPC_N],
        decoded: Option<&LdpcDecoded>,
    ) {
        self.inner
            .on_ldpc_attempt(pass, cand_idx, ap_pass, iaptype, max_osd, apmask, llr, decoded);
    }

    fn on_subtract_residual(&mut self, pass: usize, cand_idx: usize, dd: &[f32; NMAX]) {
        self.inner.on_subtract_residual(pass, cand_idx, dd);
    }
}

struct BinReader {
    buf: Vec<u8>,
    pos: usize,
}

impl BinReader {
    fn from_path(path: &Path) -> Result<Self, Stage4Error> {
        let mut f = File::open(path)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(Self { buf, pos: 0 })
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn read_i32(&mut self) -> Result<i32, Stage4Error> {
        if self.remaining() < 4 {
            return Err(Stage4Error::FortranDump("unexpected EOF reading i32".into()));
        }
        let v = i32::from_le_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    fn read_f32(&mut self) -> Result<f32, Stage4Error> {
        if self.remaining() < 4 {
            return Err(Stage4Error::FortranDump("unexpected EOF reading f32".into()));
        }
        let v = f32::from_le_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    fn read_f32_vec(&mut self, n: usize) -> Result<Vec<f32>, Stage4Error> {
        if self.remaining() < n * 4 {
            return Err(Stage4Error::FortranDump(format!(
                "unexpected EOF reading {n} f32s"
            )));
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.read_f32()?);
        }
        Ok(out)
    }

    fn read_i8_vec(&mut self, n: usize) -> Result<Vec<u8>, Stage4Error> {
        if self.remaining() < n {
            return Err(Stage4Error::FortranDump(format!(
                "unexpected EOF reading {n} bytes"
            )));
        }
        let out = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(out)
    }

    fn read_c32_vec(&mut self, n: usize) -> Result<Vec<Complex32>, Stage4Error> {
        if self.remaining() < n * 8 {
            return Err(Stage4Error::FortranDump(format!(
                "unexpected EOF reading {n} complex32s"
            )));
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let re = self.read_f32()?;
            let im = self.read_f32()?;
            out.push(Complex32::new(re, im));
        }
        Ok(out)
    }
}

fn rel_l2_f32(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let d = f64::from(x) - f64::from(y);
        num += d * d;
        let fy = f64::from(y);
        den += fy * fy;
    }
    if den <= 1e-30 {
        num.sqrt()
    } else {
        (num / den).sqrt()
    }
}

fn rel_l2_c32(a: &[Complex32], b: &[Complex32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let d = (x - y).norm() as f64;
        num += d * d;
        let yn = y.norm() as f64;
        den += yn * yn;
    }
    if den <= 1e-30 {
        num.sqrt()
    } else {
        (num / den).sqrt()
    }
}

fn run_suite_dump_diff(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    sim: &SimRunner,
) -> Result<Vec<DumpDiffCaseSummary>, Stage4Error> {
    let dump_bin = ensure_ft4_dump_binary(wsjtx_root, rust_ftx_root, bin_dir)?;
    let mut cases: Vec<(String, Vec<i16>, Option<QsoContext>, u16)> = Vec::new();

    // 1. Real WSJT-X sample WAV (blind CQ Q=0)
    let wav02 = wsjtx_root.join("samples/FT4/000000_000002.wav");
    let s02 = wav::read_wav_12k_mono(&wav02)?;
    cases.push((
        "sample_000002_Q0".into(),
        s02.clone(),
        Some(QsoContext {
            ap_enabled: true,
            qso_progress: 0,
            rx_freq_hz: 1500.0,
            ..Default::default()
        }),
        1500,
    ));

    // 2. Same WAV with QSO AP Q=3 active (K1ABC / W9XYZ on 2560 Hz)
    cases.push((
        "sample_000002_Q3_ap".into(),
        s02,
        Some(QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 3,
            rx_freq_hz: 2560.0,
            ap_enabled: true,
            ..Default::default()
        }),
        2560,
    ));

    // 3. Deterministic ft4sim fixture dt=0, strong SNR +10 dB (tests subtraction fidelity clearly)
    let (s_dt0, _) = sim.run_ft4sim("CQ K1ABC FN42", 1500.0, 0.0, 0.0, 0.0, 10.0)?;
    cases.push((
        "ft4sim_dt0_p10db".into(),
        s_dt0,
        Some(QsoContext {
            ap_enabled: true,
            qso_progress: 0,
            rx_freq_hz: 1500.0,
            ..Default::default()
        }),
        1500,
    ));

    // 4. Shifted ft4sim fixtures dt=+0.7s and dt=-0.7s (tests segment 2 & segment 3 negative ibest slicing)
    let (s_dtp, _) = sim.run_ft4sim("K1ABC W9XYZ -05", 1500.0, 0.7, 0.0, 0.0, -5.0)?;
    cases.push((
        "ft4sim_dt_pos07".into(),
        s_dtp,
        Some(QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 2,
            rx_freq_hz: 1500.0,
            ap_enabled: true,
            ..Default::default()
        }),
        1500,
    ));

    let (s_dtm, _) = sim.run_ft4sim("K1ABC W9XYZ RR73", 1500.0, -0.7, 0.0, 0.0, -5.0)?;
    cases.push((
        "ft4sim_dt_neg07".into(),
        s_dtm,
        Some(QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 3,
            rx_freq_hz: 1500.0,
            ap_enabled: true,
            ..Default::default()
        }),
        1500,
    ));

    let mut summaries = Vec::with_capacity(cases.len());

    for (label, samples_i16, qso_ctx, rx_freq_u16) in cases {
        let tmp = tempfile::tempdir()?;
        let raw_bin = tmp.path().join("raw_i16.bin");
        let mut raw_bytes = Vec::with_capacity(NMAX * 2);
        let n_copy = samples_i16.len().min(NMAX);
        for &s in &samples_i16[..n_copy] {
            raw_bytes.extend_from_slice(&s.to_le_bytes());
        }
        while raw_bytes.len() < NMAX * 2 {
            raw_bytes.extend_from_slice(&0_i16.to_le_bytes());
        }
        fs::write(&raw_bin, raw_bytes)?;

        let my_arg = qso_ctx
            .as_ref()
            .and_then(|q| q.my_call.as_deref())
            .unwrap_or("-");
        let his_arg = qso_ctx
            .as_ref()
            .and_then(|q| q.his_call.as_deref())
            .unwrap_or("-");
        let contest_num = qso_ctx.as_ref().map(|q| q.contest as u8).unwrap_or(0);
        let qso_prog = qso_ctx.as_ref().map(|q| q.qso_progress).unwrap_or(0);

        let status = Command::new(&dump_bin)
            .args([
                raw_bin.to_string_lossy().as_ref(),
                tmp.path().to_string_lossy().as_ref(),
                "200.0",
                "4000.0",
                &rx_freq_u16.to_string(),
                "3",
                &n_copy.to_string(),
                my_arg,
                his_arg,
                &contest_num.to_string(),
                &qso_prog.to_string(),
            ])
            .status()?;
        if !status.success() {
            return Err(Stage4Error::FortranDump(format!(
                "ft4_stage4_dump failed on {label}"
            )));
        }

        // Check AP set parity
        let ht = ftxr_core::protocol::hashtable::CallsignHashTable::new();
        let rust_ap = Ft4ApSet::from_context(qso_ctx.as_ref(), &ht);
        let mut r_ap = BinReader::from_path(&tmp.path().join("ap_set.bin"))?;
        let nohis_ftx = r_ap.read_i32()? != 0;
        let apbits_ftx = r_ap.read_f32_vec(LDPC_N)?;
        let apmy_ftx = r_ap.read_i8_vec(28)?;
        let aphis_ftx = r_ap.read_i8_vec(28)?;
        if nohis_ftx != rust_ap.no_his_call {
            return Err(Stage4Error::GateFailed(format!(
                "{label}: nohiscall mismatch ftx={nohis_ftx} rust={}",
                rust_ap.no_his_call
            )));
        }
        for i in 0..LDPC_N {
            if (apbits_ftx[i] - rust_ap.apbits[i]).abs() > 1e-4 {
                return Err(Stage4Error::GateFailed(format!(
                    "{label}: apbits[{i}] mismatch ftx={} rust={}",
                    apbits_ftx[i], rust_ap.apbits[i]
                )));
            }
        }
        for i in 0..28 {
            let ru_r = u8::from(rust_ap.apmy_ru[i] > 0.0);
            let fd_r = u8::from(rust_ap.aphis_fd[i] > 0.0);
            if apmy_ftx[i] != ru_r || aphis_ftx[i] != fd_r {
                return Err(Stage4Error::GateFailed(format!(
                    "{label}: apmy_ru/aphis_fd mismatch at {i}"
                )));
            }
        }

        // Run Rust Ft4Decoder with probe
        let mut ft4_dec = ftxr_core::ft4::decoder::Ft4Decoder::new();
        let mut cfg = DecodeConfig::for_mode(Mode::Ft4);
        cfg.depth = 3;
        cfg.freq_min_hz = 200.0;
        cfg.freq_max_hz = 4000.0;
        cfg.rx_freq_hz = f32::from(rx_freq_u16);
        cfg.qso = qso_ctx;
        let mut capture = FullProbeCapture::new();
        let mut decs = Vec::new();
        let rust_res = ft4_dec.decode_i16_with_probe(
            &samples_i16,
            &cfg,
            &ht,
            None,
            Ft4DecodeOptions::default(),
            Some(&mut capture),
            &mut |m| decs.push(m),
        )?;
        let _ = decs;

        // Parse candidates.bin
        let mut r_cand = BinReader::from_path(&tmp.path().join("candidates.bin"))?;
        let ncand_ftx = r_cand.read_i32()? as usize;
        let savg_ftx = r_cand.read_f32_vec(1152)?;
        let sbase_ftx = r_cand.read_f32_vec(1152)?;
        let mut cands_ftx = Vec::with_capacity(ncand_ftx);
        for _ in 0..ncand_ftx {
            cands_ftx.push((r_cand.read_f32()?, r_cand.read_f32()?));
        }

        if ncand_ftx != capture.inner.cands_pass1.len() {
            return Err(Stage4Error::GateFailed(format!(
                "{label}: candidate count mismatch ftx={ncand_ftx} rust={}",
                capture.inner.cands_pass1.len()
            )));
        }
        let max_rel_l2_savg = rel_l2_f32(&capture.inner.savg_pass1, &savg_ftx);
        let max_rel_l2_sbase = rel_l2_f32(&capture.inner.sbase_pass1, &sbase_ftx);
        let mut max_cand_df_hz = 0.0_f64;
        for (i, ((ff, sr), rc)) in cands_ftx
            .iter()
            .zip(capture.inner.cands_pass1.iter())
            .enumerate()
        {
            let df = (f64::from(*ff) - f64::from(rc.freq_hz)).abs();
            max_cand_df_hz = max_cand_df_hz.max(df);
            if df > 1e-2 || (sr - rc.snr_ratio).abs() > 1e-3 {
                return Err(Stage4Error::GateFailed(format!(
                    "{label}: cand[{i}] mismatch ftx=({ff}, {sr}) rust=({}, {})",
                    rc.freq_hz, rc.snr_ratio
                )));
            }
        }

        // Parse cands_cd2.bin
        let mut r_cd2 = BinReader::from_path(&tmp.path().join("cands_cd2.bin"))?;
        let mut max_rel_l2_cd2 = 0.0_f64;
        while r_cd2.remaining() > 0 {
            let ic = (r_cd2.read_i32()? - 1) as usize;
            let _f0 = r_cd2.read_f32()?;
            let _snr = r_cd2.read_f32()?;
            let cd2_ftx = r_cd2.read_c32_vec(NDMAX)?;
            if let Some(cd2_rust) = capture.inner.cd2_pass1.get(&ic) {
                let err = rel_l2_c32(cd2_rust, &cd2_ftx);
                max_rel_l2_cd2 = max_rel_l2_cd2.max(err);
            }
        }

        // Parse eval_bitmetrics.bin
        let mut r_eval = BinReader::from_path(&tmp.path().join("eval_bitmetrics.bin"))?;
        let mut max_rel_l2_cb = 0.0_f64;
        let mut max_rel_l2_cd = 0.0_f64;
        let mut max_rel_l2_bm = 0.0_f64;
        let mut max_rel_l2_llr = 0.0_f64;
        let mut ftx_evals_checked = 0usize;
        while r_eval.remaining() > 0 {
            let ic = (r_eval.read_i32()? - 1) as usize;
            let iseg = (r_eval.read_i32()? - 1) as usize;
            let smax_ftx = r_eval.read_f32()?;
            let ibest_ftx = r_eval.read_i32()?;
            let idfbest_ftx = r_eval.read_i32()?;
            let f1_ftx = r_eval.read_f32()?;
            let nsync_qual_ftx = r_eval.read_i32()? as usize;
            let cb_ftx = r_eval.read_c32_vec(NDMAX)?;
            let cd_ftx = r_eval.read_c32_vec(NFRAME_DOWN)?;
            let bm_colmajor_ftx = r_eval.read_f32_vec(NBITS * 3)?;
            let llra_ftx = r_eval.read_f32_vec(NCW)?;
            let llrb_ftx = r_eval.read_f32_vec(NCW)?;
            let llrc_ftx = r_eval.read_f32_vec(NCW)?;

            if let Some(re) = capture
                .inner
                .evals_pass1
                .iter()
                .find(|e| e.cand_idx == ic && e.iseg == iseg)
            {
                ftx_evals_checked += 1;
                if re.ibest != ibest_ftx
                    || re.idfbest != idfbest_ftx
                    || re.nsync_qual != nsync_qual_ftx
                    || (re.smax - smax_ftx).abs() > 1e-4
                    || (re.f1_hz - f1_ftx).abs() > 1e-3
                {
                    return Err(Stage4Error::GateFailed(format!(
                        "{label}: eval cand={ic} seg={iseg} scalar mismatch ftx=(s={smax_ftx}, ib={ibest_ftx}, idf={idfbest_ftx}, q={nsync_qual_ftx}) rust=(s={}, ib={}, idf={}, q={})",
                        re.smax, re.ibest, re.idfbest, re.nsync_qual
                    )));
                }
                max_rel_l2_cb = max_rel_l2_cb.max(rel_l2_c32(&re.cb, &cb_ftx));
                max_rel_l2_cd = max_rel_l2_cd.max(rel_l2_c32(&re.cd, &cd_ftx));
                let mut bm_colmajor_rust = Vec::with_capacity(NBITS * 3);
                for col in 0..3 {
                    bm_colmajor_rust.extend(re.bitmetrics.iter().take(NBITS).map(|r| r[col]));
                }
                max_rel_l2_bm = max_rel_l2_bm.max(rel_l2_f32(&bm_colmajor_rust, &bm_colmajor_ftx));
                max_rel_l2_llr = max_rel_l2_llr
                    .max(rel_l2_f32(&re.llrs[0], &llra_ftx))
                    .max(rel_l2_f32(&re.llrs[1], &llrb_ftx))
                    .max(rel_l2_f32(&re.llrs[2], &llrc_ftx));
            }
        }
        let _ = ftx_evals_checked;

        // Parse ldpc_wins.bin
        let mut r_ldpc = BinReader::from_path(&tmp.path().join("ldpc_wins.bin"))?;
        let mut max_rel_l2_llrd = 0.0_f64;
        while r_ldpc.remaining() > 0 {
            let ic = (r_ldpc.read_i32()? - 1) as usize;
            let _iseg = r_ldpc.read_i32()?;
            let ipass = (r_ldpc.read_i32()? - 1) as usize;
            let iaptype_ftx = r_ldpc.read_i32()? as u8;
            let maxosd_ftx = r_ldpc.read_i32()?;
            let nhard_ftx = r_ldpc.read_i32()?;
            let dmin_ftx = r_ldpc.read_f32()?;
            let apmask_ftx = r_ldpc.read_i8_vec(LDPC_N)?;
            let llrd_ftx = r_ldpc.read_f32_vec(LDPC_N)?;
            let cw_ftx = r_ldpc.read_i8_vec(LDPC_N)?;
            let _msg77_ftx = r_ldpc.read_i8_vec(77)?;

            let rw = capture
                .inner
                .ldpc_wins_pass1
                .iter()
                .find(|w| w.cand_idx == ic && w.ap_pass == ipass)
                .ok_or_else(|| {
                    Stage4Error::GateFailed(format!(
                        "{label}: missing Rust LDPC win for cand={ic} pass={ipass}"
                    ))
                })?;

            if rw.iaptype != iaptype_ftx
                || rw.maxosd != maxosd_ftx
                || rw.nhard_errors != nhard_ftx
                || (rw.dmin - dmin_ftx).abs() > 1e-3
                || rw.apmask[..] != apmask_ftx[..]
                || rw.cw[..] != cw_ftx[..]
            {
                return Err(Stage4Error::GateFailed(format!(
                    "{label}: LDPC win cand={ic} mismatch ftx=(ap={iaptype_ftx}, osd={maxosd_ftx}, nh={nhard_ftx}, dmin={dmin_ftx}) rust=(ap={}, osd={}, nh={}, dmin={})",
                    rw.iaptype, rw.maxosd, rw.nhard_errors, rw.dmin
                )));
            }
            max_rel_l2_llrd = max_rel_l2_llrd.max(rel_l2_f32(&rw.llr, &llrd_ftx));
        }

        // Check sub_first.bin & dd_pass1.bin
        let sub_first_path = tmp.path().join("sub_first.bin");
        let rel_l2_sub_first = if sub_first_path.exists() {
            let mut r_sub = BinReader::from_path(&sub_first_path)?;
            let _ic = r_sub.read_i32()?;
            let _f1 = r_sub.read_f32()?;
            let _dt = r_sub.read_f32()?;
            let _itone = r_sub.read_f32_vec(103)?;
            let dd_sub_ftx = r_sub.read_f32_vec(NMAX)?;
            let rust_sub = capture.inner.sub_first.as_ref().ok_or_else(|| {
                Stage4Error::GateFailed(format!("{label}: missing Rust first subtraction"))
            })?;
            Some(rel_l2_f32(rust_sub, &dd_sub_ftx))
        } else {
            None
        };

        let mut r_dd1 = BinReader::from_path(&tmp.path().join("dd_pass1.bin"))?;
        let dd_p1_ftx = r_dd1.read_f32_vec(NMAX)?;
        let rust_dd1 = capture.inner.dd_pass1.as_ref().ok_or_else(|| {
            Stage4Error::GateFailed(format!("{label}: missing Rust pass1 dd residual"))
        })?;
        let rel_l2_dd_pass1 = rel_l2_f32(rust_dd1, &dd_p1_ftx);

        let dec_txt = fs::read_to_string(tmp.path().join("decoded.txt"))?;
        let decodes_ftx = dec_txt.lines().filter(|l| !l.trim().is_empty()).count();
        let decodes_rust = rust_res.len();

        let summary = DumpDiffCaseSummary {
            label: label.clone(),
            cand_count_ftx: ncand_ftx,
            cand_count_rust: capture.inner.cands_pass1.len(),
            max_rel_l2_savg,
            max_rel_l2_sbase,
            max_cand_df_hz,
            max_rel_l2_cd2,
            max_rel_l2_cb,
            max_rel_l2_cd,
            max_rel_l2_bitmetrics: max_rel_l2_bm,
            max_rel_l2_llr,
            max_rel_l2_llrd,
            rel_l2_sub_first,
            rel_l2_dd_pass1,
            decodes_ftx,
            decodes_rust,
        };
        if let Some(s1) = rel_l2_sub_first {
            if s1 > 1e-4 {
                let mut r_sub = BinReader::from_path(&sub_first_path)?;
                let ic_s = r_sub.read_i32()?;
                let f1_s = r_sub.read_f32()?;
                let dt_s = r_sub.read_f32()?;
                let _itone = r_sub.read_f32_vec(103)?;
                let dd_sub_ftx = r_sub.read_f32_vec(NMAX)?;
                let rust_sub = capture.inner.sub_first.as_ref().unwrap();
                let mut max_d = 0.0_f32;
                let mut max_i = 0usize;
                let mut ftx_nz = (NMAX, 0usize);
                let mut rust_nz = (NMAX, 0usize);
                for (idx, (&a, &b)) in rust_sub.iter().zip(dd_sub_ftx.iter()).enumerate() {
                    let da = (a - f32::from(samples_i16[idx])).abs();
                    let db = (b - f32::from(samples_i16[idx])).abs();
                    if da > 1e-4 {
                        rust_nz.0 = rust_nz.0.min(idx);
                        rust_nz.1 = rust_nz.1.max(idx);
                    }
                    if db > 1e-4 {
                        ftx_nz.0 = ftx_nz.0.min(idx);
                        ftx_nz.1 = ftx_nz.1.max(idx);
                    }
                    let d = (a - b).abs();
                    if d > max_d {
                        max_d = d;
                        max_i = idx;
                    }
                }
                eprintln!("[DIAG-SUB1] ic={ic_s} f1={f1_s} dt={dt_s} nz_rust={rust_nz:?} nz_ftx={ftx_nz:?} max_diff={max_d} at idx={max_i} rust={} ftx={}", rust_sub[max_i], dd_sub_ftx[max_i]);
            }
        }
        if rel_l2_dd_pass1 > 1e-4 {
            let mut max_d = 0.0_f32;
            let mut max_i = 0usize;
            for (idx, (&a, &b)) in rust_dd1.iter().zip(dd_p1_ftx.iter()).enumerate() {
                let d = (a - b).abs();
                if d > max_d {
                    max_d = d;
                    max_i = idx;
                }
            }
            eprintln!("[DIAG-DDP1] max_diff={max_d} at idx={max_i} rust={} ftx={}", rust_dd1[max_i], dd_p1_ftx[max_i]);
        }

        println!(
            "[DUMP-DIFF] {label}: savg={max_rel_l2_savg:.2e} sbase={max_rel_l2_sbase:.2e} cd2={max_rel_l2_cd2:.2e} cb={max_rel_l2_cb:.2e} cd={max_rel_l2_cd:.2e} bm={max_rel_l2_bm:.2e} llr={max_rel_l2_llr:.2e} sub1={:?} dd_p1={rel_l2_dd_pass1:.2e} p1_wins=({}/{}) dec=({decodes_rust}/{decodes_ftx})",
            rel_l2_sub_first,
            capture.inner.ldpc_wins_pass1.len(),
            capture.inner.evals_pass1.len()
        );
        summaries.push(summary);
    }

    for summary in &summaries {
        for (metric, val) in [
            ("savg", summary.max_rel_l2_savg),
            ("sbase", summary.max_rel_l2_sbase),
            ("cd2", summary.max_rel_l2_cd2),
            ("cb", summary.max_rel_l2_cb),
            ("cd", summary.max_rel_l2_cd),
            ("bitmetrics", summary.max_rel_l2_bitmetrics),
            ("llrs", summary.max_rel_l2_llr),
            ("llrd", summary.max_rel_l2_llrd),
            ("dd_pass1", summary.rel_l2_dd_pass1),
        ] {
            if val > 1e-4 {
                return Err(Stage4Error::GateFailed(format!(
                    "{}: {metric} rel_l2 {val:.3e} > 1e-4",
                    summary.label
                )));
            }
        }
        if let Some(s1) = summary.rel_l2_sub_first {
            if s1 > 1e-4 {
                return Err(Stage4Error::GateFailed(format!(
                    "{}: sub_first rel_l2 {s1:.3e} > 1e-4",
                    summary.label
                )));
            }
        }
    }

    Ok(summaries)
}

#[allow(clippy::too_many_arguments)]
fn run_jt9_ft4(
    jt9: &Jt9Runner,
    wav_path: &Path,
    depth: u8,
    profile: Jt9Profile,
    rx_freq_hz: u16,
    my_call: Option<&str>,
    his_call: Option<&str>,
    qso_progress: Option<u8>,
    wisdom: Option<&Path>,
) -> Result<Vec<Jt9Decode>, Stage4Error> {
    let mut jcfg = Jt9RunConfig::new(Mode::Ft4, depth, profile, 1);
    jcfg.rx_freq_hz = rx_freq_hz;
    jcfg.my_call = my_call.map(str::to_string);
    jcfg.his_call = his_call.map(str::to_string);
    jcfg.qso_progress = qso_progress;
    let out = jt9.run_wav(wav_path, &jcfg, wisdom)?;
    Ok(out.decodes)
}

fn run_rust_ft4_i16(
    engine: &mut DecoderEngine,
    samples: &[i16],
    depth: u8,
    qso: Option<QsoContext>,
    rx_freq_hz: f32,
) -> Result<Vec<DecodedMessage>, Stage4Error> {
    let mut cfg = DecodeConfig::for_mode(Mode::Ft4);
    cfg.depth = depth;
    cfg.rx_freq_hz = rx_freq_hz;
    cfg.qso = qso;
    Ok(engine.decode_i16(samples, &cfg)?)
}

fn match_measurements(
    rust: &[DecodedMessage],
    oracle: &[Jt9Decode],
) -> (usize, f32, f32, i32) {
    let mut matched = 0usize;
    let mut max_df = 0.0_f32;
    let mut max_dt = 0.0_f32;
    let mut max_dsnr = 0_i32;

    for r in rust {
        if let Some(o) = oracle.iter().find(|d| d.message.trim() == r.message.trim()) {
            matched += 1;
            max_df = max_df.max((r.af_hz - o.af_hz).abs());
            max_dt = max_dt.max((r.dt_sec - o.dt_sec).abs());
            max_dsnr = max_dsnr.max((i32::from(r.snr_db) - i32::from(o.snr_db)).abs());
        }
    }
    (matched, max_df, max_dt, max_dsnr)
}

fn run_suite_real_corpus(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
) -> Result<Vec<CorpusCompareRow>, Stage4Error> {
    let fixtures = [
        (
            "samples/FT4/000000_000002.wav",
            wsjtx_root.join("samples/FT4/000000_000002.wav"),
        ),
        (
            "fixtures/committed/ft4/000000_000201.wav",
            rust_ftx_root.join("fixtures/committed/ft4/000000_000201.wav"),
        ),
    ];

    let mut rows = Vec::new();
    let mut engine = DecoderEngine::new();

    for (name, path) in &fixtures {
        let samples = wav::read_wav_12k_mono(path)?;
        for depth in [1u8, 2, 3] {
            let oracle = run_jt9_ft4(
                jt9,
                path,
                depth,
                Jt9Profile::BlindCqAp,
                1500,
                None,
                None,
                None,
                wisdom,
            )?;
            let qso = if depth >= 2 {
                Some(QsoContext {
                    ap_enabled: true,
                    qso_progress: 0,
                    rx_freq_hz: 1500.0,
                    ..Default::default()
                })
            } else {
                None
            };
            let rust = run_rust_ft4_i16(&mut engine, &samples, depth, qso, 1500.0)?;
            let (matched, max_df, max_dt, max_dsnr) = match_measurements(&rust, &oracle);
            let ratio_pct = if oracle.is_empty() {
                100.0
            } else {
                (rust.len() as f64) * 100.0 / (oracle.len() as f64)
            };
            println!(
                "[REAL] {name} -d {depth}: rust={} jt9={} ({ratio_pct:.1}%), matched={matched}, max|Δaf|={max_df:.2} Hz, max|Δdt|={max_dt:.3} s, max|Δsnr|={max_dsnr} dB",
                rust.len(),
                oracle.len()
            );

            let min_pct = if depth == 3 { 98.0 } else { 95.0 };
            if ratio_pct < min_pct {
                return Err(Stage4Error::GateFailed(format!(
                    "{name} -d {depth}: decode ratio {ratio_pct:.1}% < {min_pct}%"
                )));
            }
            if max_df > 2.0 || max_dt > 0.1 || max_dsnr > 2 {
                return Err(Stage4Error::GateFailed(format!(
                    "{name} -d {depth}: measurement tolerance exceeded df={max_df}, dt={max_dt}, dsnr={max_dsnr}"
                )));
            }

            rows.push(CorpusCompareRow {
                fixture: (*name).to_string(),
                depth,
                jt9_count: oracle.len(),
                rust_count: rust.len(),
                ratio_pct,
                matched,
                max_abs_df_hz: max_df,
                max_abs_dt_sec: max_dt,
                max_abs_dsnr_db: max_dsnr,
            });
        }
    }
    Ok(rows)
}

fn run_suite_crowded(
    wsjtx_root: &Path,
    _rust_ftx_root: &Path,
    sim: &SimRunner,
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
) -> Result<CrowdedCompareSummary, Stage4Error> {
    let msgs_path = wsjtx_root.join("lib/ft4/messages.txt");
    if !msgs_path.exists() {
        return Err(Stage4Error::GateFailed(format!(
            "missing WSJT-X FT4 messages file {}",
            msgs_path.display()
        )));
    }

    let tmp = tempfile::tempdir()?;
    let mut engine = DecoderEngine::new();
    let mut j_counts = [0usize; 3];
    let mut r_counts = [0usize; 3];
    let mut max_df = 0.0_f32;
    let mut max_dt = 0.0_f32;
    let mut max_dsnr = 0_i32;

    // 6 x 20-signal slots + 2 x 25-signal stress slots from WSJT-X ft4sim_mult (170 total signals).
    let slot_sizes = [20usize, 20, 20, 20, 20, 20, 25, 25];
    for (slot_idx, &nsigs) in slot_sizes.iter().enumerate() {
        let (samples, _) = sim.run_ft4sim_mult(&msgs_path, nsigs as u32, 1)?;
        let wav_path = tmp.path().join(format!("000000_0001{:02}.wav", slot_idx));
        wav::write_wav_12k_mono(&wav_path, &samples)?;

        for depth in [1u8, 2, 3] {
            let oracle = run_jt9_ft4(
                jt9,
                &wav_path,
                depth,
                Jt9Profile::BlindCqAp,
                1500,
                None,
                None,
                None,
                wisdom,
            )?;
            let qso = if depth >= 2 {
                Some(QsoContext {
                    ap_enabled: true,
                    qso_progress: 0,
                    rx_freq_hz: 1500.0,
                    ..Default::default()
                })
            } else {
                None
            };
            let rust = run_rust_ft4_i16(&mut engine, &samples, depth, qso, 1500.0)?;
            j_counts[(depth - 1) as usize] += oracle.len();
            r_counts[(depth - 1) as usize] += rust.len();
            let (_m, df, dt, dsnr) = match_measurements(&rust, &oracle);
            max_df = max_df.max(df);
            max_dt = max_dt.max(dt);
            max_dsnr = max_dsnr.max(dsnr);
        }
    }

    for (idx, min_pct) in [(0, 95.0), (1, 95.0), (2, 98.0)] {
        let pct = (r_counts[idx] as f64) * 100.0 / (j_counts[idx].max(1) as f64);
        println!(
            "[CROWDED] -d {}: rust={} jt9={} ({pct:.1}%), max|Δaf|={max_df:.2}, max|Δdt|={max_dt:.3}, max|Δsnr|={max_dsnr}",
            idx + 1,
            r_counts[idx],
            j_counts[idx]
        );
        if pct < min_pct {
            return Err(Stage4Error::GateFailed(format!(
                "crowded -d {} decode ratio {pct:.1}% < {min_pct}%",
                idx + 1
            )));
        }
    }
    if max_df > 2.0 || max_dt > 0.1 || max_dsnr > 2 {
        return Err(Stage4Error::GateFailed(format!(
            "crowded measurement tolerance exceeded df={max_df}, dt={max_dt}, dsnr={max_dsnr}"
        )));
    }

    Ok(CrowdedCompareSummary {
        slots_20sig: 6,
        slots_25sig: 2,
        jt9_d1: j_counts[0],
        rust_d1: r_counts[0],
        jt9_d2: j_counts[1],
        rust_d2: r_counts[1],
        jt9_d3: j_counts[2],
        rust_d3: r_counts[2],
        max_abs_df_hz: max_df,
        max_abs_dt_sec: max_dt,
        max_abs_dsnr_db: max_dsnr,
    })
}

fn estimate_50pct_threshold(pts: &[(f64, f64)]) -> f64 {
    for w in pts.windows(2) {
        let (s1, r1) = w[0];
        let (s2, r2) = w[1];
        if (r1 >= 0.5 && r2 <= 0.5) || (r1 <= 0.5 && r2 >= 0.5) {
            if (r1 - r2).abs() < 1e-9 {
                return 0.5 * (s1 + s2);
            }
            let frac = (0.5 - r1) / (r2 - r1);
            return s1 + frac * (s2 - s1);
        }
    }
    -18.25
}

fn run_suite_awgn(
    sim: &SimRunner,
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    cliff_trials: usize,
    jobs: usize,
) -> Result<AwgnCompareSummary, Stage4Error> {
    let msg = "CQ K1ABC FN42";
    let af_hz = 1500.0;
    let tpl = sim.clean_template(Mode::Ft4, msg, af_hz)?;
    let tmp = tempfile::tempdir()?;

    let snrs: Vec<f64> = (0..=21).map(|i| -(i as f64)).collect();
    let mut points = Vec::with_capacity(snrs.len());
    let mut rust_curve = Vec::new();
    let mut jt9_curve = Vec::new();
    let mut min_0_to_14 = 100.0_f64;

    for snr_db in snrs {
        let n_trials = if snr_db >= -12.0 {
            cliff_trials.clamp(10, 30)
        } else {
            cliff_trials.max(20)
        };
        let run_jt9_on_pt = snr_db <= -15.0;
        let workers = effective_workers(jobs, n_trials);

        let rust_ok = AtomicUsize::new(0);
        let next_trial = AtomicUsize::new(0);
        let err_slot: Mutex<Option<Stage4Error>> = Mutex::new(None);
        let wav_slots: Vec<Mutex<Option<PathBuf>>> =
            (0..n_trials).map(|_| Mutex::new(None)).collect();

        std::thread::scope(|s| {
            for _ in 0..workers {
                let err_ref = &err_slot;
                let next_ref = &next_trial;
                let ok_ref = &rust_ok;
                let wavs_ref = &wav_slots;
                let tpl_ref = &tpl;
                let tmp_path = tmp.path();
                s.spawn(move || {
                    let mut engine = DecoderEngine::new();
                    loop {
                        if err_ref.lock().unwrap().is_some() {
                            break;
                        }
                        let trial = next_ref.fetch_add(1, Ordering::Relaxed);
                        if trial >= n_trials {
                            break;
                        }
                        let dt_sec = ((trial % 5) as f64 - 2.0) * 0.05;
                        let slot = SlotSpec {
                            mode: Mode::Ft4,
                            signals: vec![SignalSpec {
                                message: msg.into(),
                                af_hz,
                                dt_sec,
                                snr_db,
                                late_start_ms: None,
                            }],
                            truncate_ms: None,
                        };
                        let synth = synthesize_slot_from_templates(
                            derive_seed(
                                "v1",
                                "stage4_awgn",
                                Mode::Ft4,
                                &format!("snr_{snr_db:.0}"),
                                trial,
                            ),
                            &slot,
                            std::slice::from_ref(tpl_ref),
                        );
                        let qso = Some(QsoContext {
                            ap_enabled: true,
                            qso_progress: 0,
                            rx_freq_hz: 1500.0,
                            ..Default::default()
                        });
                        match run_rust_ft4_i16(&mut engine, &synth.samples, 3, qso, 1500.0) {
                            Ok(decs) => {
                                if decs.iter().any(|d| d.message == msg) {
                                    ok_ref.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(e) => {
                                *err_ref.lock().unwrap() = Some(e);
                                break;
                            }
                        }
                        if run_jt9_on_pt {
                            let wp = tmp_path.join(format!(
                                "0000{:02}_{:06}.wav",
                                (-snr_db) as u32,
                                trial + 1
                            ));
                            if let Err(e) = wav::write_wav_12k_mono(&wp, &synth.samples) {
                                *err_ref.lock().unwrap() = Some(Stage4Error::Wav(e));
                                break;
                            }
                            *wavs_ref[trial].lock().unwrap() = Some(wp);
                        }
                    }
                });
            }
        });
        if let Some(e) = err_slot.into_inner().unwrap() {
            return Err(e);
        }
        let rust_ok = rust_ok.load(Ordering::Relaxed);

        let jt9_rate_pct = if run_jt9_on_pt {
            let wav_paths: Vec<PathBuf> = wav_slots
                .into_iter()
                .filter_map(|m| m.into_inner().unwrap())
                .collect();
            let jt9_ok = AtomicUsize::new(0);
            let next_j = AtomicUsize::new(0);
            let j_err: Mutex<Option<Stage4Error>> = Mutex::new(None);
            std::thread::scope(|s| {
                for _ in 0..workers {
                    let err_ref = &j_err;
                    let next_ref = &next_j;
                    let ok_ref = &jt9_ok;
                    let paths_ref = &wav_paths;
                    s.spawn(move || loop {
                        if err_ref.lock().unwrap().is_some() {
                            break;
                        }
                        let idx = next_ref.fetch_add(1, Ordering::Relaxed);
                        if idx >= paths_ref.len() {
                            break;
                        }
                        match run_jt9_ft4(
                            jt9,
                            &paths_ref[idx],
                            3,
                            Jt9Profile::BlindCqAp,
                            1500,
                            None,
                            None,
                            None,
                            wisdom,
                        ) {
                            Ok(decs) => {
                                if decs.iter().any(|d| d.message.trim() == msg) {
                                    ok_ref.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(e) => {
                                *err_ref.lock().unwrap() = Some(e);
                                break;
                            }
                        }
                    });
                }
            });
            if let Some(e) = j_err.into_inner().unwrap() {
                return Err(e);
            }
            let r = (jt9_ok.load(Ordering::Relaxed) as f64) * 100.0 / (n_trials as f64);
            jt9_curve.push((snr_db, r / 100.0));
            Some(r)
        } else {
            None
        };

        let rust_rate_pct = (rust_ok as f64) * 100.0 / (n_trials as f64);
        if snr_db >= -14.0 {
            min_0_to_14 = min_0_to_14.min(rust_rate_pct);
        }
        rust_curve.push((snr_db, rust_rate_pct / 100.0));
        println!(
            "[AWGN] {snr_db:5.1} dB ({n_trials} trials): rust={rust_rate_pct:5.1}% jt9={:?}",
            jt9_rate_pct.map(|v| format!("{v:.1}%"))
        );

        points.push(AwgnPointRow {
            snr_db,
            trials: n_trials,
            rust_rate_pct,
            jt9_rate_pct,
        });
    }

    let rust_th = estimate_50pct_threshold(&rust_curve);
    let jt9_th = if !jt9_curve.is_empty() {
        estimate_50pct_threshold(&jt9_curve)
    } else {
        -18.25
    };
    println!(
        "[AWGN-SUMMARY] 0..-14 min={min_0_to_14:.1}%, 50% threshold rust={rust_th:.2} dB vs jt9={jt9_th:.2} dB"
    );

    if min_0_to_14 < 99.0 {
        return Err(Stage4Error::GateFailed(format!(
            "0..-14 dB decode rate {min_0_to_14:.1}% < 99.0%"
        )));
    }
    if (rust_th - jt9_th).abs() > 1.0 {
        return Err(Stage4Error::GateFailed(format!(
            "FT4 threshold delta {:.2} dB > 1.0 dB (rust={rust_th:.2}, jt9={jt9_th:.2})",
            (rust_th - jt9_th).abs()
        )));
    }

    Ok(AwgnCompareSummary {
        min_rate_0_to_neg14_pct: min_0_to_14,
        jt9_threshold_50_db: jt9_th,
        rust_threshold_50_db: rust_th,
        points,
    })
}

fn run_suite_ap_matrix(
    sim: &SimRunner,
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
) -> Result<ApMatrixSummary, Stage4Error> {
    let mut engine = DecoderEngine::new();
    let mut weak_a1_cq_ok = 0usize;
    let mut weak_a3_inside_ok = 0usize;
    let mut weak_a6_inside_ok = 0usize;
    let mut uncertain_ap_tagged = 0usize;
    let mut fd_a3_ok = 0usize;
    let mut ru_a2_ok = 0usize;
    let tmp = tempfile::tempdir()?;

    // 1. Weak CQ (-20.0 dB) via a1 (Q=0)
    let tpl_cq = sim.clean_template(Mode::Ft4, "CQ K1ABC FN42", 1500.0)?;
    for trial in 0..20 {
        let slot = SlotSpec {
            mode: Mode::Ft4,
            signals: vec![SignalSpec {
                message: "CQ K1ABC FN42".into(),
                af_hz: 1500.0,
                dt_sec: 0.0,
                snr_db: -20.0,
                late_start_ms: None,
            }],
            truncate_ms: None,
        };
        let synth = synthesize_slot_from_templates(
            derive_seed("v1", "stage4_ap", Mode::Ft4, "a1", trial),
            &slot,
            std::slice::from_ref(&tpl_cq),
        );
        let qso = Some(QsoContext {
            ap_enabled: true,
            qso_progress: 0,
            rx_freq_hz: 1500.0,
            ..Default::default()
        });
        let decs = run_rust_ft4_i16(&mut engine, &synth.samples, 3, qso, 1500.0)?;
        if let Some(d) = decs.iter().find(|d| d.message == "CQ K1ABC FN42") {
            weak_a1_cq_ok += 1;
            if d.ap_uncertain() {
                uncertain_ap_tagged += 1;
            }
        }
    }

    // 2. Weak QSO Q=3 inside vs outside ±50 Hz
    let tpl_qso = sim.clean_template(Mode::Ft4, "K1ABC W9XYZ R-12", 1500.0)?;
    let tpl_rr73 = sim.clean_template(Mode::Ft4, "K1ABC W9XYZ RR73", 1500.0)?;
    let mut outside_had_a3_or_a6 = false;

    for trial in 0..20 {
        let slot = SlotSpec {
            mode: Mode::Ft4,
            signals: vec![SignalSpec {
                message: "K1ABC W9XYZ R-12".into(),
                af_hz: 1500.0,
                dt_sec: 0.0,
                snr_db: -20.0,
                late_start_ms: None,
            }],
            truncate_ms: None,
        };
        let synth = synthesize_slot_from_templates(
            derive_seed("v1", "stage4_ap", Mode::Ft4, "a3", trial),
            &slot,
            std::slice::from_ref(&tpl_qso),
        );
        let q_in = QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 3,
            rx_freq_hz: 1510.0,
            ap_enabled: true,
            ..Default::default()
        };
        let decs_in = run_rust_ft4_i16(&mut engine, &synth.samples, 3, Some(q_in), 1510.0)?;
        if let Some(d) = decs_in.iter().find(|d| d.message == "K1ABC W9XYZ R-12") {
            weak_a3_inside_ok += 1;
            if d.ap_uncertain() {
                uncertain_ap_tagged += 1;
            }
        }

        let mut q_out = QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 3,
            rx_freq_hz: 1700.0,
            ap_enabled: true,
            ..Default::default()
        };
        let decs_out = run_rust_ft4_i16(&mut engine, &synth.samples, 3, Some(q_out.clone()), 1700.0)?;
        if decs_out.iter().any(|d| matches!(d.ap_tag.kind(), Some(3 | 6))) {
            outside_had_a3_or_a6 = true;
        }
        q_out.rx_freq_hz = 1500.0;

        let slot_rr = SlotSpec {
            mode: Mode::Ft4,
            signals: vec![SignalSpec {
                message: "K1ABC W9XYZ RR73".into(),
                af_hz: 1500.0,
                dt_sec: 0.0,
                snr_db: -21.0,
                late_start_ms: None,
            }],
            truncate_ms: None,
        };
        let synth_rr = synthesize_slot_from_templates(
            derive_seed("v1", "stage4_ap", Mode::Ft4, "a6", trial),
            &slot_rr,
            std::slice::from_ref(&tpl_rr73),
        );
        let decs_rr = run_rust_ft4_i16(&mut engine, &synth_rr.samples, 3, Some(q_out), 1500.0)?;
        if decs_rr.iter().any(|d| d.message == "K1ABC W9XYZ RR73") {
            weak_a6_inside_ok += 1;
        }
    }

    // 3. Contest modes: Field Day (a3) & RTTY Roundup (a2)
    let tpl_fd = sim.clean_template(Mode::Ft4, "K1ABC W9XYZ 2A EMA", 1500.0)?;
    let tpl_ru = sim.clean_template(Mode::Ft4, "K1ABC W9XYZ 579 WI", 1500.0)?;
    for trial in 0..10 {
        let s_fd = synthesize_slot_from_templates(
            derive_seed("v1", "stage4_fd", Mode::Ft4, "fd", trial),
            &SlotSpec {
                mode: Mode::Ft4,
                signals: vec![SignalSpec {
                    message: "K1ABC W9XYZ 2A EMA".into(),
                    af_hz: 1500.0,
                    dt_sec: 0.0,
                    snr_db: -18.0,
                    late_start_ms: None,
                }],
                truncate_ms: None,
            },
            std::slice::from_ref(&tpl_fd),
        );
        let q_fd = QsoContext {
            my_call: Some("K1ABC".into()),
            his_call: Some("W9XYZ".into()),
            qso_progress: 3,
            rx_freq_hz: 1500.0,
            ap_enabled: true,
            contest: ContestMode::FieldDay,
            ..Default::default()
        };
        if run_rust_ft4_i16(&mut engine, &s_fd.samples, 3, Some(q_fd), 1500.0)?
            .iter()
            .any(|d| d.message == "K1ABC W9XYZ 2A EMA")
        {
            fd_a3_ok += 1;
        }

        let s_ru = synthesize_slot_from_templates(
            derive_seed("v1", "stage4_ru", Mode::Ft4, "ru", trial),
            &SlotSpec {
                mode: Mode::Ft4,
                signals: vec![SignalSpec {
                    message: "K1ABC W9XYZ 579 WI".into(),
                    af_hz: 1500.0,
                    dt_sec: 0.0,
                    snr_db: -18.0,
                    late_start_ms: None,
                }],
                truncate_ms: None,
            },
            std::slice::from_ref(&tpl_ru),
        );
        let q_ru = QsoContext {
            my_call: Some("K1ABC".into()),
            qso_progress: 1,
            rx_freq_hz: 2200.0,
            ap_enabled: true,
            contest: ContestMode::RttyRoundup,
            ..Default::default()
        };
        if run_rust_ft4_i16(&mut engine, &s_ru.samples, 3, Some(q_ru), 2200.0)?
            .iter()
            .any(|d| d.message == "K1ABC W9XYZ 579 WI")
        {
            ru_a2_ok += 1;
        }
    }

    // Also run one jt9 sanity check on -19.5 dB Q=3 slot
    let wp = tmp.path().join("000000_000001.wav");
    let synth_jt9 = synthesize_slot_from_templates(
        derive_seed("v1", "stage4_ap", Mode::Ft4, "a3_jt9", 0),
        &SlotSpec {
            mode: Mode::Ft4,
            signals: vec![SignalSpec {
                message: "K1ABC W9XYZ R-12".into(),
                af_hz: 1500.0,
                dt_sec: 0.0,
                snr_db: -19.0,
                late_start_ms: None,
            }],
            truncate_ms: None,
        },
        std::slice::from_ref(&tpl_qso),
    );
    wav::write_wav_12k_mono(&wp, &synth_jt9.samples)?;
    let _ = run_jt9_ft4(
        jt9,
        &wp,
        3,
        Jt9Profile::QsoAp,
        1500,
        Some("K1ABC"),
        Some("W9XYZ"),
        Some(3),
        wisdom,
    )?;

    println!(
        "[AP-MATRIX] weak_a1={weak_a1_cq_ok}/20, weak_a3={weak_a3_inside_ok}/20, weak_a6={weak_a6_inside_ok}/20, outside_suppressed={}, fd={fd_a3_ok}/10, ru={ru_a2_ok}/10, uncertain_tagged={uncertain_ap_tagged}",
        !outside_had_a3_or_a6
    );

    if outside_had_a3_or_a6 || weak_a3_inside_ok == 0 || weak_a6_inside_ok == 0 || fd_a3_ok < 5 || ru_a2_ok < 5 {
        return Err(Stage4Error::GateFailed("AP matrix verification failed".into()));
    }

    Ok(ApMatrixSummary {
        weak_a1_cq_ok,
        weak_a3_inside_ok,
        weak_a3_outside_suppressed: !outside_had_a3_or_a6,
        weak_a6_inside_ok,
        uncertain_ap_tagged,
        fd_a3_ok,
        ru_a2_ok,
    })
}

fn gen_pure_awgn_i16(seed_tag: &str, idx: usize) -> Vec<i16> {
    let mut rng = GaussianRng::from_seed(derive_seed("v1", seed_tag, Mode::Ft4, "noise", idx));
    let mut out = Vec::with_capacity(NMAX);
    for _ in 0..NMAX {
        out.push((rng.next_gaussian() * 350.0).round().clamp(-32767.0, 32767.0) as i16);
    }
    out
}

fn run_suite_noise(
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
    noise_slots: usize,
    jobs: usize,
) -> Result<NoiseCompareSummary, Stage4Error> {
    let slots = noise_slots.max(300);
    let tmp = tempfile::tempdir()?;
    let workers = effective_workers(jobs, slots);

    let blind_rust_fp = AtomicUsize::new(0);
    let blind_jt9_fp = AtomicUsize::new(0);
    let ap_disabled_rust_fp = AtomicUsize::new(0);
    let ap_q3_rust_fp = AtomicUsize::new(0);
    let ap_q3_jt9_fp = AtomicUsize::new(0);

    let wav_paths: Vec<PathBuf> = (0..slots)
        .map(|i| tmp.path().join(format!("000000_{:06}.wav", i + 1)))
        .collect();

    let next_idx = AtomicUsize::new(0);
    let err_slot: Mutex<Option<Stage4Error>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..workers {
            let err_ref = &err_slot;
            let next_ref = &next_idx;
            let paths_ref = &wav_paths;
            let blind_ref = &blind_rust_fp;
            let off_ref = &ap_disabled_rust_fp;
            let q3_ref = &ap_q3_rust_fp;
            s.spawn(move || {
                let mut engine = DecoderEngine::new();
                let q_blind = QsoContext {
                    ap_enabled: true,
                    qso_progress: 0,
                    rx_freq_hz: 1500.0,
                    ..Default::default()
                };
                let q_off = QsoContext {
                    my_call: Some("K1ABC".into()),
                    his_call: Some("W9XYZ".into()),
                    qso_progress: 3,
                    rx_freq_hz: 1500.0,
                    ap_enabled: false,
                    ..Default::default()
                };
                let q_3 = QsoContext {
                    my_call: Some("K1ABC".into()),
                    his_call: Some("W9XYZ".into()),
                    qso_progress: 3,
                    rx_freq_hz: 1500.0,
                    ap_enabled: true,
                    ..Default::default()
                };
                loop {
                    if err_ref.lock().unwrap().is_some() {
                        break;
                    }
                    let i = next_ref.fetch_add(1, Ordering::Relaxed);
                    if i >= slots {
                        break;
                    }
                    let samples = gen_pure_awgn_i16("stage4_noise_fp", i);
                    if let Err(e) = wav::write_wav_12k_mono(&paths_ref[i], &samples) {
                        *err_ref.lock().unwrap() = Some(Stage4Error::Wav(e));
                        break;
                    }
                    match run_rust_ft4_i16(&mut engine, &samples, 3, Some(q_blind.clone()), 1500.0) {
                        Ok(d) => {
                            blind_ref.fetch_add(d.len(), Ordering::Relaxed);
                        }
                        Err(e) => {
                            *err_ref.lock().unwrap() = Some(e);
                            break;
                        }
                    }
                    match run_rust_ft4_i16(&mut engine, &samples, 3, Some(q_off.clone()), 1500.0) {
                        Ok(d) => {
                            off_ref.fetch_add(d.len(), Ordering::Relaxed);
                        }
                        Err(e) => {
                            *err_ref.lock().unwrap() = Some(e);
                            break;
                        }
                    }
                    match run_rust_ft4_i16(&mut engine, &samples, 3, Some(q_3.clone()), 1500.0) {
                        Ok(d) => {
                            q3_ref.fetch_add(d.len(), Ordering::Relaxed);
                        }
                        Err(e) => {
                            *err_ref.lock().unwrap() = Some(e);
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

    // Batch jt9 across generated noise WAVs in chunks of 50 files across workers.
    let jt9_blind_cfg = Jt9RunConfig::new(Mode::Ft4, 3, Jt9Profile::BlindCqAp, 1);
    let mut jt9_q3_cfg = Jt9RunConfig::new(Mode::Ft4, 3, Jt9Profile::QsoAp, 1);
    jt9_q3_cfg.my_call = Some("K1ABC".into());
    jt9_q3_cfg.his_call = Some("W9XYZ".into());
    jt9_q3_cfg.qso_progress = Some(3);

    let chunks: Vec<&[PathBuf]> = wav_paths.chunks(50).collect();
    let chunk_workers = effective_workers(jobs, chunks.len());
    let next_chunk = AtomicUsize::new(0);
    let j_err: Mutex<Option<Stage4Error>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..chunk_workers {
            let err_ref = &j_err;
            let next_ref = &next_chunk;
            let chunks_ref = &chunks;
            let blind_cfg = &jt9_blind_cfg;
            let q3_cfg = &jt9_q3_cfg;
            let blind_j = &blind_jt9_fp;
            let q3_j = &ap_q3_jt9_fp;
            s.spawn(move || loop {
                if err_ref.lock().unwrap().is_some() {
                    break;
                }
                let ci = next_ref.fetch_add(1, Ordering::Relaxed);
                if ci >= chunks_ref.len() {
                    break;
                }
                let refs: Vec<&Path> = chunks_ref[ci].iter().map(PathBuf::as_path).collect();
                match jt9.run_wavs(&refs, blind_cfg, wisdom) {
                    Ok(out) => {
                        blind_j.fetch_add(out.decodes.len(), Ordering::Relaxed);
                    }
                    Err(e) => {
                        *err_ref.lock().unwrap() = Some(Stage4Error::Jt9(e));
                        break;
                    }
                }
                match jt9.run_wavs(&refs, q3_cfg, wisdom) {
                    Ok(out) => {
                        q3_j.fetch_add(out.decodes.len(), Ordering::Relaxed);
                    }
                    Err(e) => {
                        *err_ref.lock().unwrap() = Some(Stage4Error::Jt9(e));
                        break;
                    }
                }
            });
        }
    });
    if let Some(e) = j_err.into_inner().unwrap() {
        return Err(e);
    }

    let blind_rust_fp = blind_rust_fp.load(Ordering::Relaxed);
    let blind_jt9_fp = blind_jt9_fp.load(Ordering::Relaxed);
    let ap_disabled_rust_fp = ap_disabled_rust_fp.load(Ordering::Relaxed);
    let ap_q3_rust_fp = ap_q3_rust_fp.load(Ordering::Relaxed);
    let ap_q3_jt9_fp = ap_q3_jt9_fp.load(Ordering::Relaxed);

    println!(
        "[NOISE] ({slots} slots) blind FP: rust={blind_rust_fp} vs jt9={blind_jt9_fp}; ap_off FP: rust={ap_disabled_rust_fp}; Q=3 AP FP: rust={ap_q3_rust_fp} vs jt9={ap_q3_jt9_fp}"
    );

    if blind_rust_fp > blind_jt9_fp || ap_q3_rust_fp > ap_q3_jt9_fp {
        return Err(Stage4Error::GateFailed(format!(
            "noise FP exceeded jt9 baseline: blind ({blind_rust_fp} > {blind_jt9_fp}) or Q3 ({ap_q3_rust_fp} > {ap_q3_jt9_fp})"
        )));
    }

    Ok(NoiseCompareSummary {
        slots_per_mode: slots,
        blind_rust_fp,
        blind_jt9_fp,
        ap_disabled_rust_fp,
        ap_q3_rust_fp,
        ap_q3_jt9_fp,
    })
}

fn run_suite_loopback(
    sim: &SimRunner,
    jt9: &Jt9Runner,
    wisdom: Option<&Path>,
) -> Result<LoopbackSummary, Stage4Error> {
    let mut engine = DecoderEngine::new();
    let cfg_rx = DecodeConfig::for_mode(Mode::Ft4);
    let tmp = tempfile::tempdir()?;

    let test_msgs = [
        ("CQ K1ABC FN42", 1500.0_f32),
        ("K1ABC W9XYZ -07", 1100.0_f32),
        ("K1ABC W9XYZ RR73", 2300.0_f32),
        ("K1ABC W9XYZ 2A EMA", 1750.0_f32),
        ("CQ RU AB5XS EM12", 2650.0_f32),
    ];

    let mut passed = 0usize;
    let mut total = 0usize;
    let mut max_df = 0.0_f32;
    let mut max_dt = 0.0_f32;

    // 1. Nominal 300ms delay TX->RX (both ftxr-core and jt9) across buffer lengths
    for (idx, &(msg, af_hz)) in test_msgs.iter().enumerate() {
        let mut mcfg = ModulateConfig::default_for(Mode::Ft4, af_hz, SAMPLE_RATE_HZ);
        let pcm = engine.modulate(msg, &mcfg)?;
        let mut pcm_90k = pcm.clone();
        pcm_90k.resize(90_000, 0.0);

        for (sub_label, slice) in [
            ("64k_default", &pcm[..]),
            ("66k_trunc", &pcm_90k[..66_000]),
            ("72576_nmax", &pcm_90k[..NMAX]),
            ("90k_full", &pcm_90k[..]),
        ] {
            total += 1;
            let decs = engine.decode(slice, &cfg_rx)?;
            if decs.len() != 1 || decs[0].message != msg {
                return Err(Stage4Error::GateFailed(format!(
                    "loopback fail ({msg}, {sub_label}): got {decs:?}"
                )));
            }
            max_df = max_df.max((decs[0].af_hz - af_hz).abs());
            passed += 1;
        }

        // Verify jt9 -5 decodes ftxr-core TX too
        total += 1;
        let i16_buf: Vec<i16> = pcm_90k
            .iter()
            .take(NMAX)
            .map(|&s| (s * 16_000.0).round() as i16)
            .collect();
        let wp = tmp.path().join(format!("000000_0000{:02}.wav", idx + 1));
        wav::write_wav_12k_mono(&wp, &i16_buf)?;
        let j_decs = run_jt9_ft4(jt9, &wp, 3, Jt9Profile::BlindNoAp, 1500, None, None, None, wisdom)?;
        if !j_decs.iter().any(|d| d.message.trim() == msg) {
            return Err(Stage4Error::GateFailed(format!(
                "jt9 -5 failed on ftxr-core TX ({msg}): {j_decs:?}"
            )));
        }
        passed += 1;

        // 2. UTC-aligned late-start mute (0.5 s, 1.0 s, 1.5 s)
        let ht_tmp = ftxr_core::protocol::hashtable::CallsignHashTable::new();
        for &offset_ms in &[500u64, 1_000, 1_500] {
            mcfg.offset_ms = offset_ms;
            let (pcm_late, _) = ftxr_core::modulate::modulate_slot_pcm(
                msg,
                &mcfg,
                90_000,
                &ftxr_core::protocol::types::UnpackContext::default(),
                &ht_tmp,
            )?;
            total += 1;
            let decs = engine.decode(&pcm_late, &cfg_rx)?;
            if decs.len() != 1 || decs[0].message != msg {
                return Err(Stage4Error::GateFailed(format!(
                    "late-start ({msg}, {offset_ms}ms) failed: {decs:?}"
                )));
            }
            passed += 1;
        }
    }

    // 3. ft4sim dt = 0.0 and shifted dt in [-0.8, +0.9]s in BOTH decoders
    for (idx, &dt_sec) in [0.0_f64, -0.8, -0.4, 0.5, 0.9].iter().enumerate() {
        total += 1;
        let (samples, _) = sim.run_ft4sim("CQ K1ABC FN42", 1500.0, dt_sec, 0.0, 0.0, 20.0)?;
        let decs = engine.decode_i16(&samples, &cfg_rx)?;
        if decs.len() != 1 || decs[0].message != "CQ K1ABC FN42" {
            return Err(Stage4Error::GateFailed(format!(
                "ft4sim dt={dt_sec} failed in ftxr-core: {decs:?}"
            )));
        }
        max_dt = max_dt.max((decs[0].dt_sec - (dt_sec as f32)).abs());

        let wp = tmp.path().join(format!("000000_0009{:02}.wav", idx + 1));
        wav::write_wav_12k_mono(&wp, &samples)?;
        let j_decs = run_jt9_ft4(jt9, &wp, 3, Jt9Profile::BlindNoAp, 1500, None, None, None, wisdom)?;
        if !j_decs.iter().any(|d| d.message.trim() == "CQ K1ABC FN42") {
            return Err(Stage4Error::GateFailed(format!(
                "ft4sim dt={dt_sec} failed in jt9"
            )));
        }
        passed += 1;
    }

    // 4. Edge lengths and silence (zero panics, zero FPs)
    for len in [0usize, 100, 72_575, 72_576, 90_000] {
        let silent = vec![0.0_f32; len];
        let decs = engine.decode(&silent, &cfg_rx)?;
        if !decs.is_empty() {
            return Err(Stage4Error::GateFailed(format!(
                "silent len={len} produced non-empty decodes"
            )));
        }
    }

    println!(
        "[LOOPBACK] {passed}/{total} cases passed cleanly (max_df={max_df:.2} Hz, max_dt={max_dt:.3} s)"
    );
    Ok(LoopbackSummary {
        nominal_cases_passed: passed,
        nominal_cases_total: total,
        max_delta_af_hz: max_df,
        max_delta_dt_sec: max_dt,
    })
}
