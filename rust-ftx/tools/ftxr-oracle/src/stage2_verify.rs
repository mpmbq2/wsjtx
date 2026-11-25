// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Stage-by-stage Fortran intermediate array verification (`stage2-verify`) enforcing `<= 1e-4`
//! relative error across `baseline`, `sync8`, `ft8_downsample`, `sync8d`, `ft8b`, `decode174_91`,
//! `subtractft8`, and `twkfreq1`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ftxr_core::dsp::baseline::{
    build_scaled_baseline_window, compute_ft8_spectrum_baseline, BaselineScratch,
};
use ftxr_core::dsp::downsample::{DownsampleSnapshotScratch, Ft8Downsampler, Ft8SpectrumSnapshot};
use ftxr_core::dsp::fft::Ft8FftPlans;
use ftxr_core::dsp::twkfreq::twkfreq1;
use ftxr_core::fec::bp::LDPC_N;
use ftxr_core::fec::decode174_91::decode174_91;
use ftxr_core::fec::osd::OsdScratch;
use ftxr_core::ft8::ft8b::{
    compute_1sym_bmeta, ft8b_extract_stage2_intermediates, normalize_bmet, Ft8bScratch,
};
use ftxr_core::ft8::params::{DOWN_NFFT2, NH1, NMAX, NN, NP2};
use ftxr_core::ft8::subtract::{Ft8Subtractor, SubtractScratch};
use ftxr_core::ft8::sync8::{sync8, BitMetricMode, Ft8Candidate, Sync8Scratch};
use ftxr_core::ft8::sync8d::Sync8dTables;
use ftxr_core::Mode;
use num_complex::Complex32;
use serde::Serialize;
use thiserror::Error;

use crate::sim::{SimError, SimRunner};
use crate::synth::{derive_seed, synthesize_slot, SignalSpec, SlotSpec};
use crate::wav::{self, WavError};

const MAX_REL_ERR: f64 = 1e-4;

#[derive(Debug, Error)]
pub enum Stage2VerifyError {
    #[error("simulator error: {0}")]
    Sim(#[from] SimError),
    #[error("WAV error: {0}")]
    Wav(#[from] WavError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to build or run ft8_stage_dump: {0}")]
    FortranDriver(String),
    #[error("stage-by-stage verification gate failed: {0}")]
    GateFailed(String),
}

#[derive(Debug, Serialize)]
pub struct CandidateCheckSummary {
    pub index: usize,
    pub freq_hz: f32,
    pub dt_sec: f32,
    pub cd0_rel_err: f64,
    pub sync8d_rel_err: f64,
    pub cd0_ref_rel_err: f64,
    pub cs_rel_err: f64,
    pub s8_rel_err: f64,
    pub bmeta_rel_err: f64,
    pub llra_rel_err: f64,
    pub nsync_match: bool,
    pub bp_cw_exact: bool,
}

#[derive(Debug, Serialize)]
pub struct FixtureStageReport {
    pub label: String,
    pub max_samples: usize,
    pub clamped_bounds_match: bool,
    pub sbase_rel_err: f64,
    pub sync8_cand_count_f90: usize,
    pub sync8_cand_count_rs: usize,
    pub sync8_top_sync_rel_err: f64,
    pub candidate_checks: Vec<CandidateCheckSummary>,
    pub sub_false_rel_err: Option<f64>,
    pub sub_true_rel_err: Option<f64>,
    pub twkfreq1_rel_err: f64,
}

#[derive(Debug, Serialize)]
pub struct Stage2VerifyReport {
    pub fixtures: Vec<FixtureStageReport>,
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
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.read_f32());
        }
        out
    }

    fn read_c32_vec(&mut self, n: usize) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let re = self.read_f32();
            let im = self.read_f32();
            out.push(Complex32::new(re, im));
        }
        out
    }

    fn read_i8_vec(&mut self, n: usize) -> Vec<u8> {
        let out = self.bytes[self.pos..self.pos + n].to_vec();
        self.pos += n;
        out
    }

    fn read_i32_vec(&mut self, n: usize) -> Vec<i32> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.read_i32());
        }
        out
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

fn l2_rel_err_c32(a: &[Complex32], b: &[Complex32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let dr = f64::from(x.re) - f64::from(y.re);
        let di = f64::from(x.im) - f64::from(y.im);
        num += dr * dr + di * di;
        den += f64::from(y.re) * f64::from(y.re) + f64::from(y.im) * f64::from(y.im);
    }
    if den == 0.0 {
        num.sqrt()
    } else {
        (num / den).sqrt()
    }
}

pub fn ensure_ft8_stage_dump_binary(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
) -> Result<PathBuf, Stage2VerifyError> {
    let src = rust_ftx_root.join("tools/ftxr-oracle/fortran_dump/ft8_stage_dump.f90");
    let lib_fort = bin_dir.join("libwsjt_fort.a");
    let lib_cxx = bin_dir.join("libwsjt_cxx.a");
    if !lib_fort.exists() || !lib_cxx.exists() {
        return Err(Stage2VerifyError::FortranDriver(format!(
            "missing libwsjt_fort.a or libwsjt_cxx.a in {}",
            bin_dir.display()
        )));
    }

    let out_dir = cache_dir.join("bin");
    fs::create_dir_all(&out_dir)?;
    let exe = out_dir.join("ft8_stage_dump");

    let needs_rebuild = if exe.exists() {
        let exe_mtime = fs::metadata(&exe)?.modified()?;
        let src_mtime = fs::metadata(&src)?.modified()?;
        let lib_mtime = fs::metadata(&lib_fort)?.modified()?;
        src_mtime > exe_mtime || lib_mtime > exe_mtime
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
            return Err(Stage2VerifyError::FortranDriver(format!(
                "gfortran compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }

    Ok(exe)
}

fn verify_one_fixture(
    dump_exe: &Path,
    label: &str,
    samples_i16: &[i16],
    max_samples: usize,
) -> Result<FixtureStageReport, Stage2VerifyError> {
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
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        return Err(Stage2VerifyError::FortranDriver(format!(
            "ft8_stage_dump failed on {label}: {}",
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

    // 1. Baseline check
    let base_bytes = fs::read(dump_dir.join("baseline.bin"))?;
    let mut br = BinReader::new(&base_bytes);
    let nfa_f90 = br.read_i32() as f32;
    let nfb_f90 = br.read_i32() as f32;
    let sbase_f90 = br.read_f32_vec(NH1);

    let win = build_scaled_baseline_window();
    let mut b_scratch = BaselineScratch::new(&plans);
    let mut sbase_rs = [0.0_f32; NH1 + 1];
    let (nfa_rs, nfb_rs) =
        compute_ft8_spectrum_baseline(&dd, 200.0, 4000.0, &plans, &win, &mut b_scratch, &mut sbase_rs);
    let clamped_bounds_match = (nfa_rs - nfa_f90).abs() < 1e-6 && (nfb_rs - nfb_f90).abs() < 1e-6;
    let sbase_rel_err = l2_rel_err_f32(&sbase_rs[1..=NH1], &sbase_f90);
    if !clamped_bounds_match || sbase_rel_err > MAX_REL_ERR {
        return Err(Stage2VerifyError::GateFailed(format!(
            "[{label}] baseline mismatch: bounds=({nfa_rs},{nfb_rs}) vs ({nfa_f90},{nfb_f90}), rel_err={sbase_rel_err:.3e}"
        )));
    }

    // 2. Sync8 check
    let sync_bytes = fs::read(dump_dir.join("sync8.bin"))?;
    let mut sr = BinReader::new(&sync_bytes);
    let ncand_f90 = sr.read_i32() as usize;
    let mut cands_f90 = Vec::with_capacity(ncand_f90);
    for _ in 0..ncand_f90 {
        cands_f90.push(Ft8Candidate {
            freq_hz: sr.read_f32(),
            dt_sec: sr.read_f32(),
            sync: sr.read_f32(),
        });
    }

    let mut sync_scratch = Sync8Scratch::new(&plans);
    let cands_rs = sync8(&dd, 200.0, 4000.0, 2.1, 1500.0, 1000, &plans, &mut sync_scratch);
    let check_n = ncand_f90.min(cands_rs.len()).min(50);
    let mut syncs_f90 = Vec::with_capacity(check_n);
    let mut syncs_rs = Vec::with_capacity(check_n);
    for i in 0..check_n {
        if (cands_f90[i].freq_hz - cands_rs[i].freq_hz).abs() > 1e-3
            || (cands_f90[i].dt_sec - cands_rs[i].dt_sec).abs() > 1e-3
        {
            return Err(Stage2VerifyError::GateFailed(format!(
                "[{label}] sync8 candidate {i} mismatch: rs={:?} vs f90={:?}",
                cands_rs[i], cands_f90[i]
            )));
        }
        syncs_f90.push(cands_f90[i].sync);
        syncs_rs.push(cands_rs[i].sync);
    }
    let sync8_top_sync_rel_err = if check_n > 0 {
        l2_rel_err_f32(&syncs_rs, &syncs_f90)
    } else {
        0.0
    };
    if sync8_top_sync_rel_err > MAX_REL_ERR {
        return Err(Stage2VerifyError::GateFailed(format!(
            "[{label}] sync8 sync rel_err={sync8_top_sync_rel_err:.3e} > {MAX_REL_ERR}"
        )));
    }

    // 3-6. Top-5 candidates intermediate array parity
    let mut snap_scratch = DownsampleSnapshotScratch::new(&plans);
    let mut snapshot = Ft8SpectrumSnapshot::new();
    downsampler.compute_snapshot(&dd, &plans, &mut snap_scratch, &mut snapshot);

    let mut ft8b_scratch = Ft8bScratch::new(&plans);
    let mut ifft_scratch = vec![Complex32::ZERO; plans.c2c_inv_3200.get_inplace_scratch_len()];
    let mut cd0_pre = [Complex32::ZERO; DOWN_NFFT2];
    let mut osd_scratch = OsdScratch::new();
    let mut candidate_checks = Vec::new();

    for (idx, cand) in cands_f90.iter().take(5).enumerate() {
        let cand_path = dump_dir.join(format!("cand_{}.bin", idx + 1));
        let bytes = fs::read(&cand_path)?;
        let mut cr = BinReader::new(&bytes);

        let _f1_in = cr.read_f32();
        let _xdt_in = cr.read_f32();
        let _i0_f90 = cr.read_i32();
        let ibest_f90 = cr.read_i32() as isize;
        let delfbest_f90 = cr.read_f32();
        let f1_ref_f90 = cr.read_f32();
        let xdt_ref_f90 = cr.read_f32();
        let _sync_fine_f90 = cr.read_f32();
        let nsync_f90 = cr.read_i32() as u8;

        let cd0_f90 = cr.read_c32_vec(DOWN_NFFT2);
        let cd0_ref_f90 = cr.read_c32_vec(DOWN_NFFT2);
        let sync21_f90 = cr.read_f32_vec(21);
        let cs_f90 = cr.read_c32_vec(8 * NN);
        let s8_f90 = cr.read_f32_vec(8 * NN);
        let bmeta_raw_f90 = cr.read_f32_vec(LDPC_N);
        let llra_f90 = cr.read_f32_vec(LDPC_N);
        let ntype_f90 = cr.read_i32();
        let nhard_f90 = cr.read_i32();
        let dmin_f90 = cr.read_f32();
        let cw_f90 = cr.read_i8_vec(LDPC_N);
        let msg91_f90 = cr.read_i8_vec(91);

        downsampler.downsample_from_snapshot(
            &snapshot,
            cand.freq_hz,
            &plans,
            &mut ifft_scratch,
            &mut cd0_pre,
        );
        let cd0_rel_err = l2_rel_err_c32(&cd0_pre, &cd0_f90);

        // Run with depth=3 (syncmin=6) or inspect cs/s8 directly even if nsync <= 8.
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

        let cd0_ref_rel_err = l2_rel_err_c32(&ft8b_scratch.cd0, &cd0_ref_f90);
        let mut cs_flat_rs = Vec::with_capacity(8 * NN);
        let mut s8_flat_rs = Vec::with_capacity(8 * NN);
        for k in 0..NN {
            cs_flat_rs.extend_from_slice(&ft8b_scratch.cs[k]);
            s8_flat_rs.extend_from_slice(&ft8b_scratch.s8[k]);
        }
        let cs_rel_err = l2_rel_err_c32(&cs_flat_rs, &cs_f90);
        let s8_rel_err = l2_rel_err_f32(&s8_flat_rs, &s8_f90);

        let bmeta_raw_rs = compute_1sym_bmeta(&ft8b_scratch.cs, BitMetricMode::Amplitude);
        let mut bmeta_norm_rs = bmeta_raw_rs;
        normalize_bmet(&mut bmeta_norm_rs);
        let mut llra_rs = [0.0_f32; LDPC_N];
        for (d, &s) in llra_rs.iter_mut().zip(bmeta_norm_rs.iter()) {
            *d = 2.83 * s;
        }
        let bmeta_rel_err = l2_rel_err_f32(&bmeta_raw_rs, &bmeta_raw_f90);
        let llra_rel_err = l2_rel_err_f32(&llra_rs, &llra_f90);

        let (sync8d_rel_err, nsync_match) = if let Some(inter) = &inter_opt {
            if inter.ibest != ibest_f90
                || (inter.delf_best_hz - delfbest_f90).abs() > 1e-4
                || (inter.f1_refined_hz - f1_ref_f90).abs() > 1e-4
                || (inter.dt_raw_sec - xdt_ref_f90).abs() > 1e-4
            {
                return Err(Stage2VerifyError::GateFailed(format!(
                    "[{label}] cand {idx} fine sync mismatch: rs=(ibest={}, delf={}, f1={}, xdt={}) vs f90=(ibest={}, delf={}, f1={}, xdt={})",
                    inter.ibest, inter.delf_best_hz, inter.f1_refined_hz, inter.dt_raw_sec,
                    ibest_f90, delfbest_f90, f1_ref_f90, xdt_ref_f90
                )));
            }
            (
                l2_rel_err_f32(&inter.sync8d_coarse, &sync21_f90),
                inter.nsync_hard == nsync_f90,
            )
        } else {
            // Candidate had nsync <= 6 so ft8b bailed early; verify sync21 curve manually.
            let i0 = ((cand.dt_sec + 0.5) * 200.0).round() as isize;
            let curve: Vec<f32> = ((i0 - 10)..=(i0 + 10))
                .map(|idt| sync8d_tables.sync8d(&cd0_pre, idt, None))
                .collect();
            (l2_rel_err_f32(&curve, &sync21_f90), nsync_f90 <= 6)
        };

        let bp_res = decode174_91(&llra_rs, 91, -1, 2, &[0; LDPC_N], &mut osd_scratch);
        let bp_cw_exact = match (ntype_f90 == 1, bp_res) {
            (true, Some(dec)) => {
                let dmin_err = ((dec.dmin - dmin_f90).abs() / dmin_f90.abs().max(1e-6)) as f64;
                dec.codeword.as_slice() == cw_f90.as_slice()
                    && dec.message91.as_slice() == msg91_f90.as_slice()
                    && dec.nhard_errors == nhard_f90
                    && dmin_err <= MAX_REL_ERR
            }
            (false, None) => true,
            _ => false,
        };

        if cd0_rel_err > MAX_REL_ERR
            || sync8d_rel_err > MAX_REL_ERR
            || cd0_ref_rel_err > MAX_REL_ERR
            || cs_rel_err > MAX_REL_ERR
            || s8_rel_err > MAX_REL_ERR
            || bmeta_rel_err > MAX_REL_ERR
            || llra_rel_err > MAX_REL_ERR
            || !nsync_match
            || !bp_cw_exact
        {
            return Err(Stage2VerifyError::GateFailed(format!(
                "[{label}] cand {idx} stage diff exceeded tolerance: cd0={cd0_rel_err:.2e} sync8d={sync8d_rel_err:.2e} cd0_ref={cd0_ref_rel_err:.2e} cs={cs_rel_err:.2e} s8={s8_rel_err:.2e} bmeta={bmeta_rel_err:.2e} llra={llra_rel_err:.2e} nsync_match={nsync_match} bp_cw_exact={bp_cw_exact}"
            )));
        }

        candidate_checks.push(CandidateCheckSummary {
            index: idx,
            freq_hz: cand.freq_hz,
            dt_sec: cand.dt_sec,
            cd0_rel_err,
            sync8d_rel_err,
            cd0_ref_rel_err,
            cs_rel_err,
            s8_rel_err,
            bmeta_rel_err,
            llra_rel_err,
            nsync_match,
            bp_cw_exact,
        });
    }

    // 7. Subtraction verification (`refine_dt = false` and `true`)
    let sub_path = dump_dir.join("sub.bin");
    let (sub_false_rel_err, sub_true_rel_err) = if sub_path.exists() {
        let sbytes = fs::read(&sub_path)?;
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
        let dd_sub_f_f90 = subr.read_f32_vec(NMAX);
        let dd_sub_t_f90 = subr.read_f32_vec(NMAX);

        let mut sub_scratch = SubtractScratch::new(&plans);
        let mut dd_rs_f = dd.clone();
        subtractor.subtract(
            &mut dd_rs_f,
            &itone,
            f1_sub,
            xdt_sub,
            false,
            &plans,
            &mut sub_scratch,
        );
        let err_f = l2_rel_err_f32(&*dd_rs_f, &dd_sub_f_f90);

        let mut dd_rs_t = dd.clone();
        subtractor.subtract(
            &mut dd_rs_t,
            &itone,
            f1_sub,
            xdt_sub,
            true,
            &plans,
            &mut sub_scratch,
        );
        let err_t = l2_rel_err_f32(&*dd_rs_t, &dd_sub_t_f90);

        if err_f > MAX_REL_ERR || err_t > MAX_REL_ERR {
            return Err(Stage2VerifyError::GateFailed(format!(
                "[{label}] subtractft8 diff exceeded tolerance: refine_dt=false err={err_f:.3e}, refine_dt=true err={err_t:.3e}"
            )));
        }
        (Some(err_f), Some(err_t))
    } else {
        (None, None)
    };

    // 8. twkfreq1 check
    let twk_bytes = fs::read(dump_dir.join("twkfreq.bin"))?;
    let mut tr = BinReader::new(&twk_bytes);
    let cb_f90 = tr.read_c32_vec(NP2);
    let ca: Vec<Complex32> = (1..=NP2)
        .map(|i| {
            let p = 0.1 * (i as f32);
            Complex32::new(p.cos(), p.sin())
        })
        .collect();
    let mut cb_rs = vec![Complex32::ZERO; NP2];
    twkfreq1(&ca, 200.0, &[-1.5, 0.3, -0.05, 0.01, -0.002], &mut cb_rs);
    let twkfreq1_rel_err = l2_rel_err_c32(&cb_rs, &cb_f90);
    if twkfreq1_rel_err > MAX_REL_ERR {
        return Err(Stage2VerifyError::GateFailed(format!(
            "[{label}] twkfreq1 rel_err={twkfreq1_rel_err:.3e} > {MAX_REL_ERR}"
        )));
    }

    Ok(FixtureStageReport {
        label: label.to_string(),
        max_samples,
        clamped_bounds_match,
        sbase_rel_err,
        sync8_cand_count_f90: ncand_f90,
        sync8_cand_count_rs: cands_rs.len(),
        sync8_top_sync_rel_err,
        candidate_checks,
        sub_false_rel_err,
        sub_true_rel_err,
        twkfreq1_rel_err,
    })
}

pub fn run_stage2_verify(
    wsjtx_root: &Path,
    rust_ftx_root: &Path,
    bin_dir: &Path,
    cache_dir: &Path,
    out_json: Option<&Path>,
) -> Result<Stage2VerifyReport, Stage2VerifyError> {
    let dump_exe = ensure_ft8_stage_dump_binary(wsjtx_root, rust_ftx_root, bin_dir, cache_dir)?;
    let sim = SimRunner::new(bin_dir.to_path_buf(), cache_dir.to_path_buf());

    // 1. Clean synthetic -10 dB signal at f0 = 1500 Hz, DT = 0.0 s
    let seed = derive_seed("v1", "stage2_verify", Mode::Ft8, "synth_m10", 0);
    let slot = SlotSpec {
        mode: Mode::Ft8,
        signals: vec![SignalSpec {
            message: "CQ N0AAA EM10".to_string(),
            af_hz: 1500.0,
            dt_sec: 0.0,
            snr_db: -10.0,
            late_start_ms: None,
        }],
        truncate_ms: None,
    };
    let synth = synthesize_slot(&sim, seed, &slot)?;
    let rep_synth = verify_one_fixture(&dump_exe, "synth_m10_1500hz", &synth.samples, NMAX)?;

    // 2. Real-world samples/FT8/210703_133430.wav (both full 180k and jt9-disk 172.8k truncation)
    let real_samples = wav::read_wav_12k_mono(&wsjtx_root.join("samples/FT8/210703_133430.wav"))?;
    let rep_real_full = verify_one_fixture(&dump_exe, "real_210703_133430_180k", &real_samples, NMAX)?;
    let rep_real_disk = verify_one_fixture(&dump_exe, "real_210703_133430_172k", &real_samples, 172_800)?;

    for rep in [&rep_synth, &rep_real_full, &rep_real_disk] {
        println!(
            "[PASS] stage2-verify [{}]: sbase={:.2e} sync8={:.2e} (cands={}/{}) sub_f={:?} sub_t={:?} twk={:.2e}",
            rep.label,
            rep.sbase_rel_err,
            rep.sync8_top_sync_rel_err,
            rep.sync8_cand_count_rs,
            rep.sync8_cand_count_f90,
            rep.sub_false_rel_err.map(|v| format!("{v:.2e}")),
            rep.sub_true_rel_err.map(|v| format!("{v:.2e}")),
            rep.twkfreq1_rel_err,
        );
    }

    let report = Stage2VerifyReport {
        fixtures: vec![rep_synth, rep_real_full, rep_real_disk],
    };
    if let Some(out) = out_json {
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(out, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }

    Ok(report)
}
