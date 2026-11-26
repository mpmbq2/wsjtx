// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Warm shared FFT plan cache, immutable precomputed DSP/FEC tables, and RAII thread-safe scratch pools.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use num_complex::Complex32;

use crate::dsp::baseline::BaselineScratch;
use crate::dsp::downsample::{
    DownsampleSnapshotScratch, Ft4DownsampleScratch, Ft4Downsampler, Ft4PassSpectrum,
    Ft8Downsampler, Ft8SpectrumSnapshot,
};
use crate::dsp::fft::{Ft4FftPlans, Ft8FftPlans};
use crate::fec::bp::LDPC_N;
use crate::fec::decode174_91::FecScratch;
use crate::ft4::ap::Ft4ApTables;
use crate::ft4::bitmetrics::Ft4BitMetricsScratch;
use crate::ft4::candidates::{Ft4CandidatePlan, Ft4CandidateScratch};
use crate::ft4::params as ft4p;
use crate::ft4::subtract::{Ft4SubtractScratch, Ft4Subtractor};
use crate::ft4::sync4d::Ft4SyncTables;
use crate::ft8::ft8b::Ft8bScratch;
use crate::ft8::params as ft8p;
use crate::ft8::subtract::{Ft8Subtractor, SubtractScratch};
use crate::ft8::sync8::Sync8Scratch;
use crate::ft8::sync8d::Sync8dTables;
use crate::protocol::hashtable::PassHashSnapshot;

/// Shared immutable cache holding warm FT8 and FT4 FFT plans built once at engine startup.
#[derive(Debug, Clone)]
pub struct FftPlanCache {
    pub ft8: Arc<Ft8FftPlans>,
    pub ft4: Arc<Ft4FftPlans>,
}

impl Default for FftPlanCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FftPlanCache {
    pub fn new() -> Self {
        Self {
            ft8: Arc::new(Ft8FftPlans::new()),
            ft4: Arc::new(Ft4FftPlans::new()),
        }
    }
}

/// Shared immutable precomputed tables (windows, Costas templates, AP prototypes, subtractors)
/// built once from `FftPlanCache` and shared lock-free across worker threads.
pub struct PrecomputedTables {
    pub fft: Arc<FftPlanCache>,
    pub ft8_downsampler: Ft8Downsampler,
    pub sync8d: Sync8dTables,
    pub ft8_subtractor: Ft8Subtractor,
    pub ft4_cand: Ft4CandidatePlan,
    pub ft4_downsampler: Ft4Downsampler,
    pub ft4_sync: Ft4SyncTables,
    pub ft4_ap: Ft4ApTables,
    pub ft4_subtractor: Ft4Subtractor,
}

impl std::fmt::Debug for PrecomputedTables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrecomputedTables")
            .field("fft", &self.fft)
            .finish_non_exhaustive()
    }
}

impl PrecomputedTables {
    pub fn new(fft: Arc<FftPlanCache>) -> Self {
        let ft8_downsampler = Ft8Downsampler::new();
        let sync8d = Sync8dTables::new();
        let ft8_subtractor = Ft8Subtractor::new(&fft.ft8);
        let ft4_cand = Ft4CandidatePlan::new();
        let ft4_downsampler = Ft4Downsampler::new();
        let ft4_sync = Ft4SyncTables::new();
        let ft4_ap = Ft4ApTables::new();
        let ft4_subtractor = Ft4Subtractor::new(&fft.ft4);
        Self {
            fft,
            ft8_downsampler,
            sync8d,
            ft8_subtractor,
            ft4_cand,
            ft4_downsampler,
            ft4_sync,
            ft4_ap,
            ft4_subtractor,
        }
    }
}

/// Pass-level scratch workspace checked out once per RX slot decode pass (`Send`).
pub struct PassScratch {
    pub dd: Box<[f32; ft8p::NMAX]>,
    pub dd_raw: Box<[f32; ft8p::NMAX]>,
    pub ft8_snap_scratch: DownsampleSnapshotScratch,
    pub ft8_snapshot: Ft8SpectrumSnapshot,
    pub sync8_scratch: Sync8Scratch,
    pub ft8_sub_scratch: SubtractScratch,
    pub ft4_cand: Ft4CandidateScratch,
    pub ft4_down: Ft4DownsampleScratch,
    pub ft4_spectrum: Ft4PassSpectrum,
    pub ft4_sub: Ft4SubtractScratch,
    pub baseline: BaselineScratch,
    pub hash_snapshot: PassHashSnapshot,
}

impl PassScratch {
    pub fn new(fft: &FftPlanCache) -> Self {
        Self {
            dd: vec![0.0_f32; ft8p::NMAX]
                .into_boxed_slice()
                .try_into()
                .expect("exact NMAX length"),
            dd_raw: vec![0.0_f32; ft8p::NMAX]
                .into_boxed_slice()
                .try_into()
                .expect("exact NMAX length"),
            ft8_snap_scratch: DownsampleSnapshotScratch::new(&fft.ft8),
            ft8_snapshot: Ft8SpectrumSnapshot::new(),
            sync8_scratch: Sync8Scratch::new(&fft.ft8),
            ft8_sub_scratch: SubtractScratch::new(&fft.ft8),
            ft4_cand: Ft4CandidateScratch::new(&fft.ft4),
            ft4_down: Ft4DownsampleScratch::new(&fft.ft4),
            ft4_spectrum: Ft4PassSpectrum::new(),
            ft4_sub: Ft4SubtractScratch::new(&fft.ft4),
            baseline: BaselineScratch::new(&fft.ft8),
            hash_snapshot: PassHashSnapshot::new(),
        }
    }

    /// Resets transient mutable buffers without releasing underlying heap capacities.
    pub fn reset_for_reuse(&mut self) {
        self.dd.fill(0.0);
        self.dd_raw.fill(0.0);
    }
}

/// Candidate-level worker scratch checked out once per Rayon thread (`Send`).
pub struct CandidateScratch {
    pub ft8b: Ft8bScratch,
    pub ft4_down: Ft4DownsampleScratch,
    pub ft4_cd0: Box<[Complex32; ft4p::NDMAX]>,
    pub ft4_cb: Box<[Complex32; ft4p::NDMAX]>,
    pub ft4_cd: Box<[Complex32; ft4p::NFRAME_DOWN]>,
    pub ft4_bm: Ft4BitMetricsScratch,
    pub ft4_bitmetrics: [[f32; 3]; ft4p::NBITS],
    pub ft4_pass_llr: [f32; LDPC_N],
    pub ft4_apmask: [u8; LDPC_N],
    pub fec: FecScratch,
}

impl CandidateScratch {
    pub fn new(fft: &FftPlanCache) -> Self {
        Self {
            ft8b: Ft8bScratch::new(&fft.ft8),
            ft4_down: Ft4DownsampleScratch::new(&fft.ft4),
            ft4_cd0: vec![Complex32::ZERO; ft4p::NDMAX]
                .into_boxed_slice()
                .try_into()
                .expect("exact NDMAX length"),
            ft4_cb: vec![Complex32::ZERO; ft4p::NDMAX]
                .into_boxed_slice()
                .try_into()
                .expect("exact NDMAX length"),
            ft4_cd: vec![Complex32::ZERO; ft4p::NFRAME_DOWN]
                .into_boxed_slice()
                .try_into()
                .expect("exact NFRAME_DOWN length"),
            ft4_bm: Ft4BitMetricsScratch::new(&fft.ft4),
            ft4_bitmetrics: [[0.0_f32; 3]; ft4p::NBITS],
            ft4_pass_llr: [0.0_f32; LDPC_N],
            ft4_apmask: [0u8; LDPC_N],
            fec: FecScratch::new(),
        }
    }

    pub fn reset_for_reuse(&mut self) {
        self.ft4_apmask.fill(0);
    }
}

/// Thread-safe RAII pool of pass-level workspaces eliminating steady-state per-slot allocations.
pub struct PassScratchPool {
    fft: Arc<FftPlanCache>,
    free: Mutex<Vec<Box<PassScratch>>>,
    created: AtomicUsize,
}

impl std::fmt::Debug for PassScratchPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassScratchPool")
            .field("created", &self.created.load(Ordering::Relaxed))
            .field("available", &self.available())
            .finish()
    }
}

impl PassScratchPool {
    pub fn new(fft: Arc<FftPlanCache>) -> Self {
        Self {
            fft,
            free: Mutex::new(Vec::new()),
            created: AtomicUsize::new(0),
        }
    }

    pub fn prewarm(&self, count: usize) {
        let mut guard = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while self.created.load(Ordering::Relaxed) < count {
            guard.push(Box::new(PassScratch::new(&self.fft)));
            self.created.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn checkout(&self) -> PooledPassScratch<'_> {
        let item = {
            let mut guard = self.free.lock().unwrap_or_else(|e| e.into_inner());
            guard.pop()
        };
        let mut scratch = item.unwrap_or_else(|| {
            self.created.fetch_add(1, Ordering::Relaxed);
            Box::new(PassScratch::new(&self.fft))
        });
        scratch.reset_for_reuse();
        PooledPassScratch {
            pool: self,
            inner: Some(scratch),
        }
    }

    pub fn created_count(&self) -> usize {
        self.created.load(Ordering::Relaxed)
    }

    pub fn available(&self) -> usize {
        self.free.lock().map(|g| g.len()).unwrap_or(0)
    }
}

/// RAII checkout guard for `PassScratch`; automatically returns to `PassScratchPool` on drop.
pub struct PooledPassScratch<'a> {
    pool: &'a PassScratchPool,
    inner: Option<Box<PassScratch>>,
}

impl Deref for PooledPassScratch<'_> {
    type Target = PassScratch;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("PassScratch present")
    }
}

impl DerefMut for PooledPassScratch<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("PassScratch present")
    }
}

impl Drop for PooledPassScratch<'_> {
    fn drop(&mut self) {
        if let Some(item) = self.inner.take() {
            let mut guard = self.pool.free.lock().unwrap_or_else(|e| e.into_inner());
            guard.push(item);
        }
    }
}

/// Thread-safe RAII pool of per-worker-thread candidate workspaces.
pub struct CandidateScratchPool {
    fft: Arc<FftPlanCache>,
    free: Mutex<Vec<Box<CandidateScratch>>>,
    created: AtomicUsize,
}

impl std::fmt::Debug for CandidateScratchPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CandidateScratchPool")
            .field("created", &self.created.load(Ordering::Relaxed))
            .field("available", &self.available())
            .finish()
    }
}

impl CandidateScratchPool {
    pub fn new(fft: Arc<FftPlanCache>) -> Self {
        Self {
            fft,
            free: Mutex::new(Vec::new()),
            created: AtomicUsize::new(0),
        }
    }

    pub fn prewarm(&self, count: usize) {
        let mut guard = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while self.created.load(Ordering::Relaxed) < count {
            guard.push(Box::new(CandidateScratch::new(&self.fft)));
            self.created.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn checkout(&self) -> PooledCandidateScratch<'_> {
        let item = {
            let mut guard = self.free.lock().unwrap_or_else(|e| e.into_inner());
            guard.pop()
        };
        let mut scratch = item.unwrap_or_else(|| {
            self.created.fetch_add(1, Ordering::Relaxed);
            Box::new(CandidateScratch::new(&self.fft))
        });
        scratch.reset_for_reuse();
        PooledCandidateScratch {
            pool: self,
            inner: Some(scratch),
        }
    }

    pub fn created_count(&self) -> usize {
        self.created.load(Ordering::Relaxed)
    }

    pub fn available(&self) -> usize {
        self.free.lock().map(|g| g.len()).unwrap_or(0)
    }
}

/// RAII checkout guard for `CandidateScratch`; automatically returns to `CandidateScratchPool` on drop.
pub struct PooledCandidateScratch<'a> {
    pool: &'a CandidateScratchPool,
    inner: Option<Box<CandidateScratch>>,
}

impl Deref for PooledCandidateScratch<'_> {
    type Target = CandidateScratch;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("CandidateScratch present")
    }
}

impl DerefMut for PooledCandidateScratch<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("CandidateScratch present")
    }
}

impl Drop for PooledCandidateScratch<'_> {
    fn drop(&mut self) {
        if let Some(item) = self.inner.take() {
            let mut guard = self.pool.free.lock().unwrap_or_else(|e| e.into_inner());
            guard.push(item);
        }
    }
}

/// Evaluates `items` in parallel on `rx_pool` using pooled `CandidateScratch` workspaces, while
/// delivering outcomes to `on_result` on the caller thread in strict ascending `idx` order as soon
/// as earlier peers finish (guaranteeing deterministic deduplication, hash-table commits, and subtraction).
pub(crate) fn run_ordered_candidates<T, R, E, F>(
    items: &[T],
    parallel: Option<(&CandidateScratchPool, &rayon::ThreadPool)>,
    cancel: &AtomicBool,
    eval: E,
    mut on_result: F,
) where
    T: Sync,
    R: Send,
    E: Fn(&T, &mut CandidateScratch) -> Option<R> + Sync,
    F: FnMut(usize, R),
{
    if items.is_empty() || cancel.load(Ordering::Relaxed) {
        return;
    }

    if let Some((cand_pool, rx_pool)) = parallel {
        if rx_pool.current_num_threads() > 1 && items.len() > 1 {
            let (tx, rx) = std::sync::mpsc::channel::<(usize, Option<R>)>();
            let eval_ref = &eval;
            std::thread::scope(|scope| {
                scope.spawn(move || {
                    rx_pool.install(|| {
                        use rayon::prelude::*;
                        items.par_iter().enumerate().for_each_init(
                            || (cand_pool.checkout(), tx.clone()),
                            |(scratch, sender), (idx, item)| {
                                if cancel.load(Ordering::Relaxed) {
                                    let _ = sender.send((idx, None));
                                    return;
                                }
                                let res = eval_ref(item, scratch);
                                let _ = sender.send((idx, res));
                            },
                        );
                    });
                    drop(tx);
                });

                let mut reorder: std::collections::BTreeMap<usize, Option<R>> =
                    std::collections::BTreeMap::new();
                let mut next_idx = 0usize;
                for (idx, res) in rx {
                    reorder.insert(idx, res);
                    while let Some(item_opt) = reorder.remove(&next_idx) {
                        if !cancel.load(Ordering::Relaxed) {
                            if let Some(res_val) = item_opt {
                                on_result(next_idx, res_val);
                            }
                        }
                        next_idx += 1;
                    }
                }
            });
            return;
        }
    }

    if let Some((cand_pool, _)) = parallel {
        let mut scratch = cand_pool.checkout();
        for (idx, item) in items.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if let Some(res) = eval(item, &mut scratch) {
                on_result(idx, res);
            }
        }
    } else {
        let mut scratch = CandidateScratch::new(&FftPlanCache::new());
        for (idx, item) in items.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if let Some(res) = eval(item, &mut scratch) {
                on_result(idx, res);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn plans_tables_and_pools_are_send_sync_and_fast_to_construct() {
        assert_send_sync::<FftPlanCache>();
        assert_send_sync::<PrecomputedTables>();
        assert_send_sync::<PassScratchPool>();
        assert_send_sync::<CandidateScratchPool>();

        let t0 = Instant::now();
        let fft = Arc::new(FftPlanCache::new());
        let _tables = PrecomputedTables::new(Arc::clone(&fft));
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Generous ceiling (<2s debug, typically <50ms release); verify fast startup.
        assert!(elapsed_ms < 2000.0, "plan build took {elapsed_ms:.1} ms");
    }

    #[test]
    fn scratch_pools_reuse_instances_without_steady_state_growth() {
        let fft = Arc::new(FftPlanCache::new());
        let pass_pool = PassScratchPool::new(Arc::clone(&fft));
        let cand_pool = CandidateScratchPool::new(Arc::clone(&fft));

        pass_pool.prewarm(1);
        cand_pool.prewarm(4);
        assert_eq!(pass_pool.created_count(), 1);
        assert_eq!(cand_pool.created_count(), 4);

        for _ in 0..10 {
            let p = pass_pool.checkout();
            let c1 = cand_pool.checkout();
            let c2 = cand_pool.checkout();
            assert_eq!(p.dd.len(), ft8p::NMAX);
            assert_eq!(c1.ft4_cd.len(), ft4p::NFRAME_DOWN);
            drop((c1, c2, p));
        }

        assert_eq!(pass_pool.created_count(), 1);
        assert_eq!(pass_pool.available(), 1);
        assert_eq!(cand_pool.created_count(), 4);
        assert_eq!(cand_pool.available(), 4);
    }
}
