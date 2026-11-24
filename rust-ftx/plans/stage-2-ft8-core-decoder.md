# Stage 2 Plan — Core FT8 Decoder MVP (Sync, Soft LLRs, BP & Subtraction)

## 1. Header & Scope

- **Stage:** 2 of 7 — Core FT8 Decoder MVP (`ftxr-core` DSP + LDPC BP + Classic FT8 Receive Pipeline)
- **Workspace Location:** `/home/antigravity/mpmbq2/wsjtx/rust-ftx/` (development branch inside `wsjtx`; zero edits to upstream WSJT-X sources or `digi-dx`)
- **Primary Oracle Targets:**
  1. Direct intermediate-array oracle against WSJT-X `libwsjt_fort` classic FT8 subroutines (`sync8`, `get_spectrum_baseline`, `baseline`, `ft8_downsample`, `sync8d`, `twkfreq1`, `ft8b`, `decode174_91`, `subtractft8`).
  2. End-to-end differential oracle against classic single-threaded `jt9 -8 -d 1 -c b -x b` (`lib/ft8/`, explicitly **NOT** `-M` / `lib/ft8var/`).
- **Licensing Constraint:** `crates/ftxr-core` is **GPLv3** pure Rust with zero `wsjtx`/Qt/Fortran/FFTW/`digi-dx` runtime dependencies; all Cargo dependencies must be GPLv3-compatible (MIT / Apache-2.0 / BSD). Unused Numerical Recipes code in upstream WSJT-X (`bessi0` in `lib/ft8/ft8b.f90:509-530`, `indexx.f90` quicksort, `shell.f90` shellsort) is explicitly excluded and replaced by idiomatic Rust `std` equivalents.

### Prerequisites (Consumed from Stages 0–1)
- **Stage 0:** Built WSJT-X reference binaries (`jt9`, `ft8sim`, `ft8code`) + static library `libwsjt_fort.a` / `libwsjt_cxx.a` in the WSJT-X build tree; golden FT8 corpus (`samples/FT8/210703_133430.wav`, `191111_110200.wav`, plus curated fixtures); deterministic seeded AWGN sweep generator and `jt9 -8 -d 1` baseline JSON scorecards in `tools/ftxr-oracle`.
- **Stage 1:** Complete `crates/ftxr-core/src/protocol/` (`pack77`, `unpack77`, thread-safe `CallsignHashTable`, `crc14`, `encode174_91`, `get_ft8_tones_from_77bits`) and `crates/ftxr-core/src/modulate/` (specifically exposing complex analytic GFSK reference generation `gen_ft8_complex_wave` matching `lib/ft8/gen_ft8wave.f90` with `icmplx=1` and `nramp=240` edge ramps).

### Stage 2 Scope & Deliverables
1. **Shared pure-Rust DSP primitives (`crates/ftxr-core/src/dsp/`):**
   - Reusable `rustfft` / `realfft` plan cache and scratch management (`fft.rs`).
   - Periodic Nuttall window and `cos^2` filter window generators (`window.rs`).
   - Spectrum baseline estimator + `f64` 5-term polynomial fit + percentile (`baseline.rs`).
   - Complex polynomial frequency-tweak mixer `twkfreq1` (`twkfreq.rs`).
   - Reusable baseband downsampler with cached spectrum snapshot `cx` (`downsample.rs`).
2. **LDPC(174,91) Belief Propagation decoder (`crates/ftxr-core/src/fec/`):**
   - 0-based parity-check tables (`MN`, `NM`, `NRW`) transcribed and verified from `lib/ft8/ldpc_174_91_c_parity.f90`.
   - Piecewise-linear `platanh`, log-domain sum-product BP (`iter = 0..=30`), early-stopping heuristic, CRC-14 validation, `nharderrors`/`dmin` metrics, and `zsum`/`zsave` accumulation hooks ready for Stage 3 OSD (`bp.rs`, `decode174_91.rs`, plus `osd.rs` stub).
3. **Classic FT8 Core RX Pipeline (`crates/ftxr-core/src/ft8/`):**
   - `params.rs`: all FT8 constants (`ft8_params.f90`).
   - `sync8.rs`: 3840-pt STFT spectrogram, 7×7 Costas correlation (`sync_abc` / `sync_bc`), `mlag=13` + `JZ=62` 40th-percentile baseline normalization, near-dupe suppression, `nfqso ± 10 Hz` priority ordering.
   - `sync8d.rs`: 200 Hz baseband 32-sample Costas correlator with frequency-tweak `ctwk`.
   - `ft8b.rs`: coarse DT search, ±2.5 Hz frequency peakup, refined re-downsample, fine DT peakup, 79×32-pt symbol FFTs (`cs`, `s8`), hard sync quality gate `nsync`, 1-symbol soft-bit LLRs `bmeta` (`imetric = 1 | 2`), `normalizebmet`, `scalefac = 2.83`, BP decode (`maxosd = -1`), post-decode `(i3, n3)` / `/R` / `TU; ` filters, SNR estimation (`xsnr`/`xsnr2`), and `nsync <= 10 && xsnr < -25.0` false-decode filter.
   - `subtract.rs`: 180,000-pt overlap-free FFT LPF complex amplitude subtraction with `NFILT=4000` `cos^2` window, `endcorrection`, `peakup`, and full `lrefinedt` support.
   - `decoder.rs`: multi-pass orchestration (`ipass = 1..=npass`, `ndepth = 1 => npass = 2, syncmin = 2.1, imetric = 1 then 2`, `xbase`, `xdt - 0.5`, exact `msg37` deduplication, streaming callback emission, and `DecoderEngine` wiring).
4. **Stage-by-Stage Fortran Intermediate Verification Harness (`tools/ftxr-oracle/`):**
   - Dev-only Fortran shim linking against WSJT-X's already-compiled `libwsjt_fort.a` (zero edits to WSJT-X files) to dump binary intermediate arrays (`sbase`, `candidate`, `cx`, `cd0`, `ibest`/`f1`, `cs`/`s8`, `bmeta`/`llra`, BP `cw`/`nharderrors`/`dmin`, post-subtraction residual `dd`) and verify Rust matches Fortran within `≤ 1e-4` relative error.

### Stage 2 Exit Gate Checklist
- [ ] All intermediate pipeline arrays (`sbase`, `candidate`, `cd0`, `ibest`/`f1`, `cs`, `s8`, `bmeta`/`llra`, BP `cw`/`dmin`, subtracted `dd`) match WSJT-X Fortran within `≤ 1e-4` relative error on reference fixtures.
- [ ] **≥ 99% decode rate** on single-signal deterministic synthetic FT8 AWGN sweep from `0 dB` down to `-15 dB` (in 1 dB steps).
- [ ] **≥ 90% of `jt9 -8 -d 1 -c b -x b` decode count** across the real-world FT8 `.wav` golden corpus.
- [ ] Measurement agreement against `jt9 -8 -d 1`: AF within `±3 Hz`, DT within `±0.15 s`, SNR within `±2 dB`.
- [ ] **0% false decodes** across pure-AWGN noise slots (≥ 100 slots).
- [ ] Passes **partial-slot** (`11.8 s` and `13.5 s` zero-padded to 15 s), **5.0 s UTC-aligned late-start TX**, and **mid-slot hot-swap TX** decode tests.
- [ ] Zero `wsjtx`/Fortran/FFTW dependencies in `crates/ftxr-core`; zero Numerical Recipes code (`bessi0`, `indexx`, `shell`); clean `cargo test` and `cargo clippy -- -D warnings`.

---

## 2. Source Audit & Technical Reference

Every constant, formula, loop bound, and quirk below was verified directly from `/home/antigravity/mpmbq2/wsjtx/lib/`.

### 2.1 Core FT8 Constants (`lib/ft8/ft8_params.f90`)

| Constant | Value | Meaning |
| :--- | ---: | :--- |
| `KK` | `91` | Information bits (`77` payload + `14` CRC-14) |
| `LDPC_N` | `174` | Codeword length (`M = 83` parity checks, `ncw = 3`) |
| `ND` | `58` | Data symbols (`29` + `29`, `58 * 3 = 174` bits) |
| `NS` | `21` | Sync symbols (`3 × 7` Costas arrays at 0-based symbols `0..7`, `36..43`, `72..79`) |
| `NN` | `79` | Total channel symbols (`NS + ND`) |
| `NSPS` | `1920` | Samples per symbol at `12,000 S/s` (`0.160 s`, `baud = 6.25 Hz`) |
| `NZ` / `NFRAME` | `151_680` | Active waveform samples (`79 * 1920 = 12.64 s`) |
| `NMAX` / `NPTS` | `180_000` | Full 15 s buffer samples at `12,000 S/s` |
| `NFFT1` (sync8) | `3840` | STFT FFT size (`2 * NSPS`, `df = 3.125 Hz`, 2× freq oversampling) |
| `NH1` | `1920` | `NFFT1 / 2` (positive frequency bins `1..=1920`, skipping DC bin `0`) |
| `NSTEP` | `480` | Rough STFT hop size (`NSPS / 4`, `tstep = 0.040 s`, 4× time oversampling) |
| `NHSYM` | `372` | `NMAX / NSTEP - 3 = 375 - 3` STFT frames (last frame covers `178080..180000`) |
| `NDOWN` | `60` | Baseband decimation factor (`fs2 = 12000 / 60 = 200.0 Hz`, `dt2 = 0.005 s`, `32` samples/sym) |
| `DOWN_NFFT1` | `192_000` | Long r2c FFT in `ft8_downsample.f90` (`df_down = 0.0625 Hz = 1/16 Hz`) |
| `DOWN_NFFT2` | `3200` | Baseband c2c IFFT in `ft8_downsample.f90` (`192000 / 60`) |
| `NP2` | `2812` | Valid non-tail baseband sample count in `sync8d.f90` and `ft8b.f90` (`14.06 s`) |
| `SUB_NFFT` | `180_000` | Subtraction FFT length (`NMAX`) in `subtractft8.f90` |
| `SUB_NFILT` | `4000` | Subtraction `cos^2` LPF span (`-2000..=+2000`, `4001` taps, `sumw = 2000.0`) |
| `ICOS7` | `[3, 1, 4, 0, 6, 5, 2]` | 7×7 Costas tone array (`sync8.f90:25`, `sync8d.f90:13`, `ft8b.f90:39`) |
| `GRAYMAP` | `[0, 1, 3, 2, 5, 6, 4, 7]` | 3-bit MSB-first index `(b0<<2)|(b1<<1)|b2` → 8-FSK tone (`ft8b.f90:49`, `genft8.f90:15`) |

### 2.2 FFT Sign & Normalization Conventions (`lib/four2a.f90` vs `rustfft` / `realfft`)

`lib/four2a.f90` wraps single-precision FFTW3 (`sfftw_plan_dft_1d` / `sfftw_plan_dft_r2c_1d`) and applies **zero normalization in either direction**. Both `rustfft` (`6.x`) and `realfft` (`3.x`) use the **exact same DFT sign and unnormalized convention**:
- Forward (`isign = -1`, `FFTW_FORWARD`): $X[k] = \sum_{n=0}^{N-1} x[n]\, e^{-j 2\pi k n / N}$
- Inverse (`isign = +1`, `FFTW_BACKWARD`): $x[n] = \sum_{k=0}^{N-1} X[k]\, e^{+j 2\pi k n / N}$

| WSJT-X Call Site | Fortran Call | FFTW Direction & Type | Rust Equivalent (`realfft` / `rustfft`) | Post-FFT Scaling in WSJT-X |
| :--- | :--- | :--- | :--- | :--- |
| `sync8.f90:38` | `four2a(x, 3840, 1, -1, 0)` | r2c Forward, $N=3840$ | `realfft::RealToComplex<f32>(3840)` → `0..=1920` | Input pre-scaled by `1.0/300.0`; FFT unnormalized |
| `get_spectrum_baseline.f90:33` | `four2a(x, 3840, 1, -1, 0)` | r2c Forward, $N=3840$ | `realfft::RealToComplex<f32>(3840)` → `0..=1920` | Window pre-scaled by `3840.0 / (sum(win) * 300.0)` |
| `ft8_downsample.f90:27` | `four2a(cx, 192000, 1, -1, 0)` | r2c Forward, $N=192000$ | `realfft::RealToComplex<f32>(192000)` → `0..=96000` | None (zero-padded `180000..192000`) |
| `ft8_downsample.f90:46` | `four2a(c1, 3200, 1, +1, 1)` | c2c Backward, $N=3200$ | `rustfft::plan_fft_inverse(3200)` | Multiply output by `1.0 / (192000.0_f32 * 3200.0_f32).sqrt()` |
| `ft8b.f90:159` | `four2a(csymb, 32, 1, -1, 1)` | c2c Forward, $N=32$ | `rustfft::plan_fft_forward(32)` | `cs = csymb[0..8] / 1e3`; `s8 = csymb[0..8].norm()` (unscaled!) |
| `subtractft8.f90:36` | `four2a(cw, 180000, 1, -1, 1)` | c2c Forward, $N=180000$ | `rustfft::plan_fft_forward(180000)` | Multiply `cw` by `1.0 / 180000.0_f32` (once at init) |
| `subtractft8.f90:78` | `four2a(cfilt, 180000, 1, -1, 1)` | c2c Forward, $N=180000$ | `rustfft::plan_fft_forward(180000)` | Pointwise product `cfilt[k] *= cw[k]` |
| `subtractft8.f90:80` | `four2a(cfilt, 180000, 1, +1, 1)` | c2c Backward, $N=180000$ | `rustfft::plan_fft_inverse(180000)` | None (`1/N` already baked into `cw`) |
| `subtractft8.f90:94` | `four2a(cx, 180000, 1, -1, 0)` | r2c Forward, $N=180000$ | `realfft::RealToComplex<f32>(180000)` → `0..=90000` | None (`lrefinedt` energy only) |

All five FFT lengths are **5-smooth** ($32 = 2^5$, $3200 = 2^7\cdot 5^2$, $3840 = 2^8\cdot 3\cdot 5$, $180000 = 2^5\cdot 3^2\cdot 5^4$, $192000 = 2^9\cdot 3\cdot 5^3$), so `rustfft`/`realfft` execute pure mixed-radix butterflies with no Bluestein fallback. Note that `realfft::process_with_scratch` mutates its input `&mut [f32]` buffer, so callers must always copy into a dedicated scratch slice first.

### 2.3 Exact 1-Based Fortran → 0-Based Rust Indexing Translation Table

Off-by-one indexing errors silently degrade weak-signal sensitivity by 1–3 dB even when strong signals still decode. The table below locks every critical array across the pipeline:

| Array | Fortran Declaration & Bounds | Rust Representation & Bounds | Exact Index Mapping & Critical Notes |
| :--- | :--- | :--- | :--- |
| `dd` / `dd0` | `real dd(1:180000)` | `dd: [f32; 180_000]` (or `Vec<f32>`) | Fortran `dd(k)` ↔ Rust `dd[k - 1]`. Direct cast `i16 as f32` (NOT `/ 32768.0`) matches Fortran `dd = iwave` verbatim. |
| `s` (STFT power) | `real s(1:1920, 1:372)` (`s(i, j)` = `\|cx(i)\|^2`) | `s: Vec<f32>` laid out `[frame * (NH1 + 1) + bin]`, `frame in 0..372`, `bin in 0..=1920` | **CRITICAL:** Fortran `s(1, j)` is `cx(1)` (`3.125 Hz`), NOT DC `cx(0)`. Keeping `bin = 0` unused (length `NH1 + 1 = 1921`) makes Rust `bin` equal Fortran `i` 1-for-1 (`f = bin * 3.125 Hz`), eliminating `i-1` shifts; `frame = j - 1` (`0..372`). |
| `sbase` | `real sbase(1:1920)` | `sbase: [f32; NH1 + 1]` (`0..=1920`) | Same bin convention as `s`: Rust `sbase[bin]` ↔ Fortran `sbase(bin)` at `bin * 3.125 Hz`; `xbase = 10_f32.powf(0.1 * (sbase[bin] - 40.0))` with `bin = (f1 / 3.125).round().clamp(1.0, 1920.0) as usize`. |
| `sync2d` | `real sync2d(1:1920, -62:+62)` | `sync2d` row-major `[bin][lag_idx]` where `lag_idx = (j + JZ) as usize` (`0..=124`) | Fortran `j in -62..=62` ↔ Rust `lag_idx = (j + 62) as usize`; `m_1based = j + 12 + 4*n` (`jstrt = 12` truncated from `0.5/0.04 = 12.5`!) ↔ 0-based `frame = m_1based - 1` guarded `0 <= frame < 372`. |
| `cx` (`ft8_downsample`) | `complex cx(0:96000)` | `cx: Vec<Complex32>` len `96_001` (`0..=96000`) | **0-based in both!** Fortran `cx(k)` ↔ Rust `cx[k]` (`f = k * 0.0625 Hz`). `ib = max(1, round(fb/df))`, `it = min(round(ft/df), 96000)`. |
| `cd0` / `c1` | `complex cd0(0:3199)` | `cd0: [Complex32; 3200]` | **0-based in both!** Fortran `cd0(k)` ↔ Rust `cd0[k]`. Valid non-tail window is `0..NP2` (`0..2812`, i.e., `k <= 2811`). `cshift(c1, i0 - ib)` ↔ `c1.rotate_left((i0 - ib).rem_euclid(3200) as usize)`. |
| `ibest` & `xdt` | `ibest` (0-based into `cd0`); `xdt = (ibest - 1) * dt2` | `ibest: isize` (0-based into `cd0`); `xdt_raw = (ibest - 1) as f32 * 0.005` | **PRESERVE `(ibest - 1)` VERBATIM:** `sync8d` & symbol extraction slice `cd0[ibest + sym*32 .. +32]`, yet `ft8b.f90:152` sets `xdt = (ibest - 1)*dt2`, `subtractft8` uses `nstart = int(xdt*12000) + 1`, and `ft8_decode.f90:215` reports `dt_sec = xdt - 0.5`. |
| `cs`, `s8` | `complex cs(0:7, 1:79)`, `real s8(0:7, 1:79)` | `cs: [[Complex32; 8]; 79]`, `s8: [[f32; 8]; 79]` | Fortran `cs(tone, k)` ↔ Rust `cs[k - 1][tone]` (`k = 1..=79`, `sym = 0..79`, `tone = 0..8`). `cs` is divided by `1000.0`; `s8` is **NOT** divided by `1000.0`. Costas blocks at 0-based `sym = 0..7`, `36..43`, `72..79`; data blocks at `7..36` and `43..72`. |
| `bmeta`, `llra`, `cw` | `real bmeta(1:174)`, `integer*1 cw(1:174)` | `[f32; 174]`, `[u8; 174]` | Fortran `bmeta(b)` ↔ Rust `bmeta[b - 1]`. Data sym `d in 0..58` (`d < 29 => sym = d + 7`, `d >= 29 => sym = d + 14`) maps to bits `3*d + 0` (MSB, weight 4), `3*d + 1` (weight 2), `3*d + 2` (LSB, weight 1). Positive LLR = bit `1`. |
| `Mn`, `Nm`, `nrw` | `Mn(1:3, 1:174)`, `Nm(1:7, 1:83)`, `nrw(1:83)` (1-based IDs) | `MN: [[usize; 3]; 174]`, `NM: [[usize; 7]; 83]`, `NRW: [usize; 83]` (0-based IDs) | Fortran column-major `Mn(k, bit)` (values `1..83`) ↔ Rust `MN[bit - 1][k - 1]` (values `0..83`). Fortran `Nm(k, chk)` (values `1..174`, `0` pad) ↔ Rust `NM[chk - 1][k - 1]` (values `0..174`, only `0..NRW[chk - 1]` accessed). |
| `cw` (filter), `cfilt`, `endcorrection` (`subtractft8`) | `cw(1:180000)`, `cfilt(1:180000)`, `endcorrection(1:2001)`, `window(-2000:2000)` | `cw_fft: Vec<Complex32>` (180k), `cfilt: Vec<Complex32>` (180k), `endcorr: [f32; 2001]` | `window[idx]` (`idx = j + 2000 in 0..=4000`) placed in `cw[0..=4000] / sumw`, then **`cw.rotate_left(NFILT / 2 + 1)` (`rotate_left(2001)`)** matching Fortran `cshift(cw, 2001)` (NOT `2000`!). `endcorr[idx]` multiplies `cfilt[idx]` and `cfilt[151679 - idx]` for `idx = 0..=2000`. Subtraction index: `start0 = (dt * 12000.0) as isize + idt`; `j0 = start0 + i0` for `i0 in 0..151680`. |

### 2.4 Verified Algorithm Details & Non-Obvious Codebase Gotchas

1. **Stale comment in `lib/ft8_decode.f90:173` vs actual `ndepth=1` behavior (`lines 102-108, 176-197`):**
   - The comment says `ndepth=1: 1 pass, bp`, **which is outdated**. Line 178 executes `if (ndepth.eq.1) npass=2`, running **2 subtraction passes**:
     - `ipass = 1`: `syncmin = 2.1`, `imetric = 1` (amplitude `|cs|`), `lsubtract = true`.
     - `ipass = 2`: `syncmin = 2.1`, `imetric = 2` (power `|cs|^2`), `lsubtract = true`.
   - At `ndepth = 1`, `ft8_decode` skips `nzhsym = 41` and `nzhsym = 47` (`if (ndepth.eq.1 .and. nzhsym.lt.50) return`) and executes only on `nzhsym = 50`.
2. **Disk WAV truncation at `172,800` samples (`14.4 s`) in `lib/jt9.f90:438-440`:**
   - When classic `jt9 -8` (without `-M`) decodes a `.wav` file from disk, `jt9.f90` copies only `50 * 3456 = 172,800` samples into `id2a` and **zeroes `id2a(172801:180000)`** (`14.4 s..15.0 s`). Our Rust decoder processes all provided samples up to `180,000` (zero-padding shorter inputs), while `ftxr-oracle` applies an explicit `172,800`-sample truncation option ONLY during exact Fortran intermediate array diffing against `jt9` disk runs.
3. **`jt9 -8 -d 1` runs `llrb..llre` and `a1` (CQ AP) even with `-c b -x b`:**
   - `jt9.f90:350` hardcodes `lft8apon = .true.` and `jt9.f90:30` initializes `nQSOProg = 0` (contradicting the `-Q` help string on line 95 which claims `default PROGRESS=1`!).
   - Inside `ft8b.f90:272-336`, `ndepth = 1` sets `maxosd = -1` (BP-only), **yet still loops `ipass = 1..5` (`llra..llre`) plus `ipass = 6..7` (`a1` CQ with `llra`/`llrc`)** because `iaptype = 1` does not require `mycall`.
   - Stage 2 MVP deliberately implements only 1-symbol `bmeta`/`llra` + BP-only (no multi-symbol `bmetb..e` and no AP — those belong to Stage 3). This explains why Stage 2's real-world gate is **≥ 90% of `jt9 -8 -d 1`** rather than 100%, and why exact intermediate verification compares `bmeta`/`llra` and single-pass `decode174_91` directly.
4. **Immutable spectrum snapshot `cx` within each pass (`lib/ft8_decode.f90:180`, `ft8b.f90:106,141,462`):**
   - `newdat` is set `.true.` once at the start of each `ipass`, cleared to `.false.` on the first `ft8_downsample`, and **never reset to `.true.` after `subtractft8` mutates `dd0`**. Therefore all candidates within a single `ipass` downsample from the exact same `cx` snapshot; subtraction updates `dd0` immediately for subsequent subtractions in the same pass and for the next `ipass`'s `sync8` and fresh `cx`.
5. **In-place clamping of `nfa`/`nfb` inside `get_spectrum_baseline.f90:38-50`:**
   - Fortran passes `nfa`/`nfb` by reference from `sync8` to `get_spectrum_baseline`, which clamps `nfa >= 100` and `nfb <= 4910` (preserving `nwin = nfb - nfa` when `nwin < 100`, e.g., `nagain` ±20 Hz window). Because `sync8` computes `ia`/`ib` *after* calling `get_spectrum_baseline`, `sync8` inherits the clamped bounds.
6. **Dead `twkfreq1` overwrite in `lib/ft8/ft8b.f90:135-142`:**
   - Line 137 mixes `cd0` by `-delfbest` via `twkfreq1(cd0, NP2, fs2, a, cd0)`, and line 141 immediately overwrites the entire `cd0(0:3199)` via `ft8_downsample(dd0, .false., f1, cd0)`! Because `f1` from `sync8` is a multiple of `3.125 Hz = 50 * 0.0625 Hz` and `delfbest` is a multiple of `0.5 Hz = 8 * 0.0625 Hz`, the refined `f1` hits an exact FFT bin in `cx` while re-centering the 1001-bin cosine bandpass filter. We still implement `src/dsp/twkfreq.rs` cleanly (verified against Fortran `twkfreq1` for Stage 4 FT4 / Stage 5 drift telemetry) and execute the exact second `ft8_downsample` in `ft8b.rs`.
7. **`jstrt = 12` truncation & `sync_bc` 5 s late-start mechanism (`lib/ft8/sync8.f90:50,75-83,123`):**
   - `jstrt = 0.5 / 0.04 = 12.5` truncates to integer `12`; `candidate0(2, k) = (jpeak - 0.5) * 0.04` compensates the half-step.
   - `sync2d(i, j) = max(sync_abc, sync_bc)` computes Costas A+B+C vs Costas B+C alone (each normalized by the 6 non-Costas tones among tones `0..=6`, **excluding tone 7**). A 5 s UTC-aligned late-start TX misses Costas A entirely (`0.5–1.62 s`), so `sync_bc` locks cleanly onto Costas B (`6.26 s`) and C (`12.02 s`), and LDPC BP recovers the missing initial symbols treated as near-zero LLR erasures.
8. **Hard-sync `nsync` tie-breaking & `syncmin` strict `<=` (`lib/ft8/ft8b.f90:168-184`):**
   - `maxloc(s8(:, k))` returns the **lowest tone index** on ties (important on zeroed Costas blocks where `s8 == 0` picks tone `0`, matching `icos7[3] == 0` for `is = 1`).
   - Rejection is strict `nsync <= syncmin`: for `ndepth <= 2`, `syncmin = 8` → requires **`nsync >= 9`**; for `ndepth == 3`, `syncmin = 6` (`imetric == 1`) or `7` (`imetric == 2`).
9. **BP early stopping, `platanh`, and 96-bit CRC-14 layout (`lib/ft8/decode174_91.f90:61-133`):**
   - Runs `iter = 0..=30` (31 checks; `iter = 0` checks raw channel hard decisions).
   - `zsum` accumulates `zn` starting from `iter = 0`; `zsave[:, iter - 1] = zsum` for `1 <= iter <= maxosd` (or `zsave[:, 0] = llr` when `maxosd == 0`).
   - CRC-14 checks a 96-bit vector `[cw[0..77], 0, 0, 0, 0, 0, cw[77..91]]` (5 zero bits between payload and 14-bit CRC, matching Stage 1 `crc14` over 10-byte zero-padded 77-bit message).
   - Early stopping fires when `iter >= 10 && ncnt >= 5 && ncheck > 15` (`ncnt` resets only when `ncheck < nclast`).
   - `platanh` saturates at `±7.0`, preventing `±inf`/`NaN` when `|Tmn| == 1.0`.
10. **SNR estimation quirks (`lib/ft8/ft8b.f90:465-488`) & safe subtraction ordering:**
    - For `nagain == true`, noise tone offset is `ios = (itone[i] + 4) % 7` (modulo **7**, not 8).
    - Normal mode uses `xsnr2 = 10*log10(arg) - 27.0` with `arg = (xsig / xbase / 3.0e6 - 1.0)` if `> 0.1` else `0.001`.
    - In Fortran, `subtractft8` executes at line 462 *before* the `if (nsync <= 10 && xsnr < -25.0)` false-decode rejection at line 483. Because `xsnr` depends solely on `s8` and `xbase` (not `dd0`), our Rust `ft8b` checks `nsync <= 10 && xsnr < -25.0` **before** calling `subtractft8` so rejected false decodes never corrupt `dd0` (zero effect on valid decodes; cleaner handoff to Stage 5).
11. **Zero/silence NaN guards:**
    - In `baseline.f90:18`, `10*log10(0.0) = -inf` can poison `polyfit` on silent slots; clamp `savg[i].max(1e-30)` and return `sbase = 0.0` if `delta.abs() < 1e-30`.
    - In `sync8.f90:78,82,110,116`, guard `t0 <= 0.0` (`sync = 0.0`) and `base <= 0.0 || !base.is_finite()` (`return vec![]`).
    - In `normalizebmet`, guard `bmetsig <= 0.0` (leave `bmet` zeroed instead of `0.0 / 0.0 = NaN`).
    - In `peakup`, guard `c.abs() < 1e-30` (return `f32::INFINITY` so `lrefinedt` skips subtraction instead of `NaN` bypassing `dx.abs() > 1.0`).

---

## 3. Architecture & Concrete Rust Design

### 3.1 Crate Dependencies (`crates/ftxr-core/Cargo.toml`)

All new dependencies are pure Rust and GPLv3-compatible:

| Crate | Version | License | Role |
| :--- | :--- | :--- | :--- |
| `rustfft` | `"6"` | MIT OR Apache-2.0 | Complex-to-complex FFTs (`32`, `3200`, `180000`; plus FT4 sizes in Stage 4) |
| `realfft` | `"3"` | MIT | Real-to-complex FFTs (`3840`, `180000`, `192000`; plus FT4 sizes in Stage 4) |
| `num-complex` | `"0.4"` | MIT OR Apache-2.0 | `Complex32` type (shared with `rustfft`/`realfft`) |

*(Zero C FFI, zero FFTW, zero `wsjtx`, zero `digi-dx`, zero `rayon` yet — `rayon` parallelism is added in Stage 5 over the immutable `cx` snapshot architecture established here.)*

### 3.2 Files to Create / Update

```text
rust-ftx/
├── crates/ftxr-core/
│   ├── Cargo.toml                  # Add rustfft, realfft, num-complex
│   └── src/
│       ├── lib.rs                  # Wire DecoderEngine::decode_ft8, DecodeConfig, DecodedMessage
│       ├── telemetry.rs            # Populate DecodeTelemetry (pass, sync, hard_errors, dmin, fec_path)
│       ├── dsp/
│       │   ├── mod.rs
│       │   ├── fft.rs              # FftPlanCache (warm reusable Arc<dyn Fft/RealToComplex> + scratch)
│       │   ├── window.rs           # nuttall_window(), cos2_lpf_window()
│       │   ├── baseline.rs         # pctile(), polyfit_f64(), fit_baseline_db(), ft8_spectrum_baseline()
│       │   ├── twkfreq.rs          # twkfreq1()
│       │   └── downsample.rs       # SpectrumSnapshot (cx) + Ft8Downsampler
│       ├── fec/
│       │   ├── mod.rs
│       │   ├── bp.rs               # LDPC(174,91) MN/NM/NRW tables, platanh(), bp_decode_174_91()
│       │   ├── osd.rs              # Stage 3 stub (OsdInput/OsdResult types, returns None in Stage 2)
│       │   └── decode174_91.rs     # Hybrid decode174_91() dispatcher (max_osd < 0 => BP-only in Stage 2)
│       └── ft8/
│           ├── mod.rs
│           ├── params.rs           # All FT8 constants, ICOS7, GRAYMAP
│           ├── sync8.rs            # Candidate, sync8() spectrogram + Costas search + priority sort
│           ├── sync8d.rs           # Sync8dTables (csync), sync8d()
│           ├── ft8b.rs             # Ft8bScratch, ft8b_decode_candidate(), compute_1sym_bmeta(), normalize_bmet()
│           ├── ap.rs               # Stage 3 stub (ApContext/ApCandidateResult placeholders)
│           ├── subtract.rs         # Ft8Subtractor (precomputed cw_fft + endcorr), subtract_ft8(), peakup()
│           └── decoder.rs          # Ft8Decoder (owns plans/tables/scratch), decode_slot_i16/f32()
└── tools/ftxr-oracle/
    ├── fortran_dump/
    │   └── ft8_stage_dump.f90      # Dev-only Fortran driver linked against wsjtx build's libwsjt_fort.a
    └── src/
        ├── stage2_verify.rs        # Intermediate binary array comparator (<= 1e-4 rel err)
        └── ft8_mvp_bench.rs        # jt9 -8 -d 1 diff scorer, AWGN sweep, partial/late/noise gates
```

### 3.3 Concrete Rust Types, Traits & Function Signatures

#### `crates/ftxr-core/src/dsp/fft.rs`
```rust
use std::sync::Arc;
use num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::{Fft, FftPlanner};

/// Immutable, thread-safe collection of pre-planned FT8 (and later FT4) FFTs.
/// Held inside `DecoderEngine` so plans stay warm across slots with zero replanning.
pub struct Ft8FftPlans {
    pub r2c_3840: Arc<dyn RealToComplex<f32>>,   // sync8 + get_spectrum_baseline
    pub r2c_192000: Arc<dyn RealToComplex<f32>>, // ft8_downsample long FFT
    pub c2c_inv_3200: Arc<dyn Fft<f32>>,         // ft8_downsample baseband IFFT
    pub c2c_fwd_32: Arc<dyn Fft<f32>>,           // ft8b symbol FFT
    pub c2c_fwd_180000: Arc<dyn Fft<f32>>,       // subtractft8 filter FFT
    pub c2c_inv_180000: Arc<dyn Fft<f32>>,       // subtractft8 filter IFFT
    pub r2c_180000: Arc<dyn RealToComplex<f32>>, // subtractft8 lrefinedt energy FFT
}

impl Ft8FftPlans {
    pub fn new() -> Self { /* plans all 7 sizes once via RealFftPlanner & FftPlanner */ }
}
```

#### `crates/ftxr-core/src/dsp/window.rs` & `baseline.rs` & `twkfreq.rs`
```rust
// window.rs
/// Periodic Nuttall window matching `lib/nuttal_window.f90` (denominator N, not N-1).
pub fn nuttall_window(n: usize) -> Vec<f32>;

/// Normalized `cos(pi*j/nfilt)^2` filter taps for `j = -nfilt/2..=+nfilt/2` (length `nfilt + 1`).
pub fn cos2_lpf_window(nfilt: usize) -> (Vec<f32>, f32);

// baseline.rs
/// Exact equivalent of `lib/pctile.f90` using `sort_by(f32::total_cmp)`.
pub fn pctile(slice: &[f32], npct: u32) -> f32;

/// Unweighted (mode=0) polynomial least-squares fit in `f64` (`nterms = 5`).
/// Solves 5x5 normal equations via partial-pivoted Gaussian elimination; returns `[0.0; 5]` if singular.
pub fn polyfit_5(x: &[f64], y: &[f64]) -> [f64; 5];

/// Port of `lib/ft8/baseline.f90`: fits quartic dB baseline (`+ 0.65 dB`) over bins `[ia..=ib]`.
/// `s_linear` and `sbase_db` are 1-based-bin indexed (`len == NH1 + 1`, index 0 unused).
pub fn fit_ft8_baseline_db(s_linear: &[f32], f_low_hz: f32, f_high_hz: f32, sbase_db: &mut [f32]);

/// Port of `lib/ft8/get_spectrum_baseline.f90`.
/// Returns clamped `(f_low_hz, f_high_hz)` matching Fortran's in-place `nfa`/`nfb` clamping.
pub fn compute_ft8_spectrum_baseline(
    dd: &[f32; 180_000],
    f_low_hz: f32,
    f_high_hz: f32,
    plans: &Ft8FftPlans,
    nuttall_scaled: &[f32],
    sbase_db: &mut [f32; 1921],
) -> (f32, f32);

// twkfreq.rs
/// Port of `lib/ft8/twkfreq1.f90`: Legendre polynomial phase mixer (reusable in FT4/telemetry).
pub fn twkfreq1(input: &[Complex32], fsample: f32, a: &[f32; 5], output: &mut [Complex32]);
```

#### `crates/ftxr-core/src/dsp/downsample.rs`
```rust
/// Immutable 192,000-pt r2c spectrum snapshot (`cx[0..=96000]`) captured once per subtraction pass.
/// Shared read-only across all candidates in a pass (enables zero-refactor `rayon` in Stage 5).
pub struct Ft8SpectrumSnapshot {
    pub cx: Vec<Complex32>, // len == 96_001
}

pub struct Ft8Downsampler {
    taper: [f32; 101], // 0.5 * (1.0 + cos(i * PI / 100.0)) for i in 0..=100
    scale: f32,        // 1.0 / (192000.0_f32 * 3200.0_f32).sqrt()
}

impl Ft8Downsampler {
    pub fn new() -> Self;

    /// Computes zero-padded 192,000-pt r2c FFT of `dd[0..180000]` into `snapshot.cx`.
    pub fn compute_snapshot(
        &self,
        dd: &[f32; 180_000],
        plans: &Ft8FftPlans,
        r2c_in: &mut [f32],
        r2c_scratch: &mut [Complex32],
        snapshot: &mut Ft8SpectrumSnapshot,
    );

    /// Downconverts `f0_hz` to 200 Hz baseband `cd0[0..3200]` from an immutable `snapshot`.
    pub fn downsample_from_snapshot(
        &self,
        snapshot: &Ft8SpectrumSnapshot,
        f0_hz: f32,
        plans: &Ft8FftPlans,
        ifft_scratch: &mut [Complex32],
        cd0: &mut [Complex32; 3200],
    );
}
```

#### `crates/ftxr-core/src/fec/bp.rs`, `osd.rs`, `decode174_91.rs`
```rust
// bp.rs
pub const LDPC_N: usize = 174;
pub const LDPC_K: usize = 91;
pub const LDPC_M: usize = 83;
pub const LDPC_COL_WEIGHT: usize = 3;
pub const BP_MAX_ITERS: usize = 30;

pub static MN: [[usize; 3]; LDPC_N] = [ /* 0-based check indices from ldpc_174_91_c_parity.f90 */ ];
pub static NM: [[usize; 7]; LDPC_M] = [ /* 0-based bit indices from ldpc_174_91_c_parity.f90 */ ];
pub static NRW: [usize; LDPC_M] = [ /* 6 or 7 per check */ ];

/// Exact piecewise-linear approximation from `lib/platanh.f90`.
#[inline]
pub fn platanh(x: f32) -> f32;

pub struct BpDecodeOutput {
    pub decoded: Option<LdpcDecoded>,
    /// Cumulative posterior LLRs saved at iterations 1..=max_osd (or channel LLR if max_osd == 0).
    /// Consumed by Stage 3 `osd174_91`.
    pub zsave: [[f32; LDPC_N]; 3],
    pub num_zsave: usize,
    pub iterations_run: usize,
    pub final_unsatisfied_checks: usize,
}

pub struct LdpcDecoded {
    pub message91: [u8; LDPC_K],
    pub codeword: [u8; LDPC_N],
    pub nhard_errors: i32,
    pub dmin: f32,
    pub fec_type: FecType, // FecType::Bp (ntype=1) | FecType::Osd (ntype=2, Stage 3)
}

pub fn bp_decode_174_91(
    llr: &[f32; LDPC_N],
    apmask: &[u8; LDPC_N],
    max_osd: i32,
) -> BpDecodeOutput;

// decode174_91.rs
/// Hybrid BP + OSD entry point matching `lib/ft8/decode174_91.f90`.
/// In Stage 2, `max_osd < 0` executes BP only; Stage 3 enables `osd174_91` over `bp.zsave`.
pub fn decode174_91(
    llr: &[f32; LDPC_N],
    keff: usize,
    max_osd: i32,
    norder: usize,
    apmask: &[u8; LDPC_N],
) -> Option<LdpcDecoded>;
```

#### `crates/ftxr-core/src/ft8/{sync8,sync8d,ft8b,subtract,decoder}.rs`
```rust
// sync8.rs
#[derive(Debug, Clone, Copy)]
pub struct Ft8Candidate {
    pub freq_hz: f32,
    pub dt_sec: f32, // relative to nominal 0.5 s: (jpeak - 0.5) * 0.04
    pub sync: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitMetricMode {
    Amplitude = 1, // imetric = 1 (pass 1)
    Power = 2,     // imetric = 2 (passes 2-3)
}

pub fn sync8(
    dd: &[f32; 180_000],
    f_low_hz: f32,
    f_high_hz: f32,
    sync_min: f32,
    nfqso_hz: f32,
    max_cand: usize,
    plans: &Ft8FftPlans,
    scratch: &mut Sync8Scratch,
) -> (Vec<Ft8Candidate>, [f32; 1921]); // (prioritized candidates, sbase_db)

// sync8d.rs
pub struct Sync8dTables {
    csync: [[Complex32; 32]; 7],
}
impl Sync8dTables {
    pub fn new() -> Self;
    pub fn sync8d(&self, cd0: &[Complex32; 3200], i0: isize, ctwk: Option<&[Complex32; 32]>) -> f32;
}

// ft8b.rs
pub struct Ft8bOutcome {
    pub freq_hz: f32,
    pub dt_raw_sec: f32, // before `- 0.5` (passed directly to subtract_ft8)
    pub sync_fine: f32,
    pub nsync_hard: u8,
    pub snr_db: f32,     // clamped >= -25.0
    pub nhard_errors: i32,
    pub dmin: f32,
    pub qual: f32,
    pub message: String,
    pub i3: u8,
    pub n3: u8,
    pub itone: [u8; 79],
}

/// Decodes a single candidate from immutable `snapshot` (does NOT mutate `dd`).
/// Caller performs deterministic subtraction on `dd` when `Some(outcome)` is returned.
pub fn ft8b_decode_candidate(
    snapshot: &Ft8SpectrumSnapshot,
    cand: &Ft8Candidate,
    xbase: f32,
    depth: u8,
    metric_mode: BitMetricMode,
    nagain: bool,
    ncontest: u8,
    plans: &Ft8FftPlans,
    downsampler: &Ft8Downsampler,
    sync8d_tables: &Sync8dTables,
    hash_table: &CallsignHashTable,
    scratch: &mut Ft8bScratch,
) -> Option<Ft8bOutcome>;

/// Exposed publicly inside crate for Stage 2 intermediate dump tests and Stage 3 reuse.
pub fn compute_1sym_bmeta(cs: &[[Complex32; 8]; 79], metric_mode: BitMetricMode) -> [f32; 174];
pub fn normalize_bmet(bmet: &mut [f32; 174]);

// subtract.rs
pub struct Ft8Subtractor {
    cw_fft: Vec<Complex32>,    // len 180_000, precomputed FFT(cshift(window/sumw, 2001)) / 180000
    endcorr: [f32; 2001],      // endcorrection[0..=2000]
}
impl Ft8Subtractor {
    pub fn new(plans: &Ft8FftPlans) -> Self;
    pub fn subtract(
        &self,
        dd: &mut [f32; 180_000],
        itone: &[u8; 79],
        f0_hz: f32,
        dt_raw_sec: f32,
        refine_dt: bool,
        plans: &Ft8FftPlans,
        scratch: &mut SubtractScratch,
    );
}
pub fn peakup(ym: f32, y0: f32, yp: f32) -> f32;

// decoder.rs
pub struct Ft8Decoder {
    plans: Ft8FftPlans,
    downsampler: Ft8Downsampler,
    sync8d: Sync8dTables,
    subtractor: Ft8Subtractor,
    sync8_scratch: Sync8Scratch,
    snapshot: Ft8SpectrumSnapshot,
    ft8b_scratch: Ft8bScratch,
    sub_scratch: SubtractScratch,
}

impl Ft8Decoder {
    pub fn new() -> Self;

    /// Primary entry point for `i16` 12 kHz PCM (exact `jt9` scaling: `s as f32`).
    pub fn decode_i16(
        &mut self,
        samples_12k: &[i16],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        mut on_decode: Option<&mut dyn FnMut(&DecodedMessage)>,
    ) -> Result<Vec<DecodedMessage>, FtxrError>;

    /// Convenience entry point for normalized `f32` `[-1.0, 1.0]` 12 kHz PCM (scales by `32768.0`).
    pub fn decode_f32(
        &mut self,
        samples_12k: &[f32],
        config: &DecodeConfig,
        hash_table: &CallsignHashTable,
        on_decode: Option<&mut dyn FnMut(&DecodedMessage)>,
    ) -> Result<Vec<DecodedMessage>, FtxrError>;
}
```

### 3.4 Memory & Concurrency Design (Stage 5 Readiness)
- **Zero per-slot heap allocations on hot path:** `Ft8Decoder` pre-allocates `dd` (720 KB), `snapshot.cx` (768 KB), `cw_fft` (1.44 MB), `cfilt`/`camp` (2.88 MB), `s` spectrogram (2.86 MB), `sync2d` (0.96 MB), and FFT scratch buffers once at `Ft8Decoder::new()` (~10 MB total warm working set).
- **Decoupled read vs. mutate for Stage 5 `rayon`:** `ft8b_decode_candidate` takes `&Ft8SpectrumSnapshot` (immutable) and per-worker `&mut Ft8bScratch`, returning `Option<Ft8bOutcome>` without mutating `dd`. In Stage 2, `Ft8Decoder::decode_i16` loops sequentially and calls `subtractor.subtract(&mut dd, ...)` immediately after each non-duplicate decode (matching classic `jt9` order); in Stage 5, candidates in a pass decode in parallel over `&Ft8SpectrumSnapshot`, then subtract sequentially in deterministic candidate order.
- **Public API alignment (`src/lib.rs`):** `DecoderEngine` embeds `Ft8Decoder` (plus `CallsignHashTable` and Stage 4's `Ft4Decoder`) and exposes `decode_i16` / `decode_f32` dispatching on `DecodeConfig::mode == Mode::Ft8`.

---

## 4. Ordered Implementation Tasks

### Task 1 — DSP Foundation (`src/dsp/{mod,fft,window,baseline,twkfreq}.rs`)
- **Inputs:** `lib/four2a.f90`, `lib/nuttal_window.f90`, `lib/pctile.f90`, `lib/polyfit.f90`, `lib/determ.f90`, `lib/ft8/get_spectrum_baseline.f90`, `lib/ft8/baseline.f90`, `lib/ft8/twkfreq1.f90`.
- **Work:**
  1. Add `rustfft = "6"`, `realfft = "3"`, `num-complex = "0.4"` to `crates/ftxr-core/Cargo.toml`.
  2. Implement `Ft8FftPlans` (`r2c` 3840/180000/192000; `c2c` 32/3200/180000).
  3. Implement `nuttall_window`, `cos2_lpf_window`, `pctile`, `polyfit_5`, `fit_ft8_baseline_db`, `compute_ft8_spectrum_baseline` (including in-place `(f_low, f_high)` `[100, 4910]` clamping and silence/zero-bin NaN guards).
  4. Implement `twkfreq1`.
- **Acceptance Criteria:** Unit tests verify FFT round-trip + sign convention ($e^{-j2\pi k/N}$ tone peaks at bin $+k$), `nuttall_window` periodicity, `pctile` edge bounds, exact quartic recovery in `polyfit_5`, and `twkfreq1` frequency shift accuracy `< 1e-6`.

### Task 2 — Baseband Downsampler & Fine Sync (`src/dsp/downsample.rs`, `src/ft8/{params,sync8d}.rs`)
- **Inputs:** `lib/ft8/ft8_params.f90`, `lib/ft8/ft8_downsample.f90`, `lib/ft8/sync8d.f90`.
- **Work:**
  1. Populate `src/ft8/params.rs` with every verified constant, `ICOS7`, and `GRAYMAP`.
  2. Implement `Ft8Downsampler::compute_snapshot` (zero-padding `180000..192000`) and `downsample_from_snapshot` (cosine taper `0..=100`, `rotate_left((i0 - ib).rem_euclid(3200))`, IFFT 3200, `1/sqrt(192000*3200)`). Guard `k <= 100` for out-of-band frequencies.
  3. Implement `Sync8dTables::new` (`csync[0..7][0..32]`) and `sync8d` with `NP2 = 2812` bounds and optional `ctwk` multiplication.
- **Acceptance Criteria:** Synthetic pure tone at `f0 = 1500.0 Hz` downconverts to DC in `cd0`; tone at `f0 + 6.25*m` appears at exact bin `m` of a 32-pt forward FFT; synthetic Costas signal peaks sharply at `ibest = round((dt + 0.5)*200)` with `sync8d`.

### Task 3 — Spectrogram & Candidate Search (`src/ft8/sync8.rs`)
- **Inputs:** `lib/ft8/sync8.f90`.
- **Work:**
  1. Implement `Sync8Scratch` and `sync8()` computing `NHSYM = 372` zero-padded 3840-pt STFT frames (`fac = 1/300.0`), keeping 1-based frequency bin alignment (`len = NH1 + 1 = 1921`, index `0` unused).
  2. Call `compute_ft8_spectrum_baseline` first and adopt its clamped `(f_low_hz, f_high_hz)`.
  3. Implement `sync2d` Costas correlation (`j in -62..=62`, `jstrt = 12`, `sync_abc` vs `sync_bc`, 7-tone sum `0..=6`, div-by-zero guard). Clamp `ib <= NH1 - 12` to prevent OOB if `f_high_hz` is ever raised.
  4. Implement `mlag = 13` (`red`) and `mlag2 = 62` (`red2`) peak extraction, 40th-percentile baseline normalization (`npctile = (0.40 * iz as f32).round() as usize`), candidate extraction up to `MAXPRECAND = 1000`, near-dupe suppression (`|df| < 4.0 && |dt| < 0.04`), and `|f - nfqso| <= 10.0` promotion + descending sync sort.
- **Acceptance Criteria:** Synthetic nominal, `+2.0 s` DT, `-2.0 s` DT, `11.8 s` partial, and `5.0 s` late-start FT8 signals all surface as top-ranked candidates with AF within `±3.125 Hz` and DT within `±0.04 s`; silent/zero buffer returns 0 candidates with zero NaNs.

### Task 4 — LDPC(174,91) Belief Propagation (`src/fec/{mod,bp,osd,decode174_91}.rs`)
- **Inputs:** `lib/ft8/decode174_91.f90`, `lib/ft8/ldpc_174_91_c_parity.f90`, `lib/platanh.f90`, Stage 1 `crc14` and `encode174_91`.
- **Work:**
  1. Transcribe `MN` (`174 × 3`), `NM` (`83 × 7`), and `NRW` (`83`) converted to 0-based indices into `src/fec/bp.rs`.
  2. Implement `platanh`, `bp_decode_174_91` (`iter = 0..=30`, `zsum`/`zsave`, CRC-14 check matching 96-bit zero-padded layout, `nhard_errors`, `dmin`, early stopping `ncnt >= 5 && iter >= 10 && ncheck > 15`).
  3. Implement `decode174_91` dispatcher calling `bp_decode_174_91` and leaving a clean `if max_osd >= 0 { osd::osd174_91(...) }` stub in `src/fec/osd.rs` for Stage 3.
- **Acceptance Criteria:**
  - Parity-matrix self-test asserts `MN` and `NM` are exact graph transposes (`522` edges) and `H * cw == 0 (mod 2)` across 500 random messages encoded with Stage 1 `encode174_91`.
  - CRC-14 cross-check asserts `crc14` on 77+14 bits matches `get_crc14` 96-bit zero-padded layout on 1,000 random vectors.
  - BP recovers noisy BPSK codewords with `0..25` bit flips and rejects invalid/all-zero codewords.

### Task 5 — Candidate Demodulator (`src/ft8/ft8b.rs`, `src/ft8/ap.rs`)
- **Inputs:** `lib/ft8/ft8b.f90` (explicitly excluding `bessi0` lines 509–530).
- **Work:**
  1. Implement coarse DT search (`i0 - 10 ..= i0 + 10`), ±2.5 Hz frequency search (`ifr = -5..=5`, step `0.5 Hz`), refined `downsample_from_snapshot` at `f1 + delfbest`, fine DT search (`ibest - 4 ..= ibest + 4`), and `dt_raw_sec = (ibest - 1) as f32 * 0.005`.
  2. Implement 79 × 32-pt symbol FFTs (`cs = csymb[0..8]/1000.0`, `s8 = |csymb[0..8]|`), hard sync `nsync` (lowest-index tie-breaking) and `nsync <= syncmin` gate (`8` for `depth <= 2`, `6`/`7` for `depth == 3`).
  3. Implement 1-symbol `compute_1sym_bmeta` (`Amplitude` and `Power`), `normalize_bmet`, `llra = 2.83 * bmeta`, and `decode174_91(&llra, 91, -1, 2, &[0; 174])`.
  4. Implement post-decode filters: `nhard_errors in 0..=36`, non-all-zero `cw`, `(i3, n3)` validity (`i3 <= 5`, `!(i3 == 0 && (n3 > 6 || n3 == 2))`), `unpack77(..., rx_mode=true)` + `/R`/`TU; ` contest filter, `get_ft8_tones_from_77bits`, SNR `xsnr`/`xsnr2`, `nsync <= 10 && xsnr < -25.0` bail-out, and `-25.0` floor clamp.
- **Acceptance Criteria:** Clean Stage 1 loopback FT8 waveform decodes with exact message, `nhard_errors == 0`, `nsync == 21`, AF error `< 1 Hz`, DT error `< 0.02 s`.

### Task 6 — Signal Subtraction (`src/ft8/subtract.rs`)
- **Inputs:** `lib/ft8/subtractft8.f90`, `lib/peakup.f90`, Stage 1 `gen_ft8_complex_wave`.
- **Work:**
  1. Implement `peakup` with denominator guard (`c.abs() < 1e-30 => f32::INFINITY`).
  2. Implement `Ft8Subtractor::new` precomputing `cw_fft` (`cos^2` window `4001` taps / `sumw`, `rotate_left(2001)`, forward 180k FFT, `* (1.0 / 180000.0)`) and `endcorr[0..=2000]`.
  3. Implement `subtract()` supporting both `refine_dt = false` (used in `ft8b` passes) and `refine_dt = true` (`sqf(-90, 0, +90)` + `peakup` + `abs(dx) <= 1.0`, ready for Stage 3/5).
- **Acceptance Criteria:** Subtracting a 0 dB synthetic FT8 signal from a two-signal mixture (`0 dB` + `-14 dB` separated by `10 Hz` in frequency) attenuates the strong signal by `> 25 dB` and allows Pass 2 to cleanly decode the masked `-14 dB` signal.

### Task 7 — Multi-Pass Decoder Orchestrator & `DecoderEngine` Wiring (`src/ft8/decoder.rs`, `src/lib.rs`, `src/telemetry.rs`)
- **Inputs:** `lib/ft8_decode.f90`.
- **Work:**
  1. Implement `Ft8Decoder::decode_i16` and `decode_f32` (zero-padding `< 180,000` samples, truncating `> 180,000`).
  2. Implement pass loop:
     - `depth == 1`: `npass = 2`, `syncmin = 2.1`, `ipass=1 => Amplitude`, `ipass=2 => Power`, `lsubtract = true`.
     - `depth == 2`: `npass = 3`, `syncmin = 2.1` (still BP-only until Stage 3 enables OSD).
     - `depth == 3`: `npass = 3`, `syncmin = 1.3`, skip pass 3 if `ndecodes == 0` (still BP-only until Stage 3).
  3. Per pass: run `sync8`, refresh `Ft8SpectrumSnapshot`, iterate prioritized candidates, compute `xbase = 10_f32.powf(0.1 * (sbase[bin] - 40.0))`, invoke `ft8b_decode_candidate`, deduplicate on exact trimmed/37-char `message`, immediately subtract valid non-dupe decode from `dd` (`refine_dt = false`), emit `DecodedMessage` (`dt_sec = dt_raw_sec - 0.5 + if eme_delay { 2.0 } else { 0.0 }`, `snr_db = snr.round() as i32`, `ap_tag = ApTag::None`, `telemetry` populated with pass/sync/hard_errors/dmin/`FecPath::Bp`), and fire optional streaming callback.
  4. Wire `DecoderEngine` in `src/lib.rs` to delegate `Mode::Ft8` to `Ft8Decoder`.
- **Acceptance Criteria:** End-to-end decode of `samples/FT8/210703_133430.wav` and `191111_110200.wav` produces clean deduplicated decodes with zero panics across partial, full, and empty slots.

### Task 8 — Fortran Intermediate Dump Harness & Stage 2 Oracle Suite (`tools/ftxr-oracle/`)
- **Inputs:** Compiled WSJT-X `libwsjt_fort.a` + `libwsjt_cxx.a` + `jt9` from Stage 0.
- **Work:**
  1. Author `tools/ftxr-oracle/fortran_dump/ft8_stage_dump.f90`: standalone dev-only Fortran program calling WSJT-X's `get_spectrum_baseline`, `sync8`, `ft8_downsample`, `sync8d`, `four2a` (32-pt), `normalizebmet`, `decode174_91`, `subtractft8`, and `twkfreq1` directly on any `.wav` file (with optional `172,800`-sample truncation flag) and writing raw little-endian binary dumps + manifest JSON. Zero modifications to WSJT-X sources!
  2. Author `tools/ftxr-oracle/src/stage2_verify.rs` loading those dumps and asserting elementwise / L2 relative error `<= 1e-4` at every stage.
  3. Author `tools/ftxr-oracle/src/ft8_mvp_bench.rs` running the full Stage 2 gate battery against `jt9 -8 -d 1 -c b -x b` and emitting a machine-readable JSON scorecard.
- **Acceptance Criteria:** All numerical, differential, partial-slot, late-start, hot-swap, and pure-noise gates pass (Section 5).

---

## 5. Verification, Oracle Testing & Numerical Gates

### 5.1 Stage-by-Stage Fortran Intermediate Array Verification (`≤ 1e-4` Gate)

Run `ft8_stage_dump` and Rust `stage2_verify` on **both** a clean synthetic signal (`-10 dB`, `f0 = 1500 Hz`, `DT = 0.0 s`) **and** real-world `samples/FT8/210703_133430.wav` (using identical `i16` input samples):

| Pipeline Stage | Dumped Arrays / Scalars | Comparison Metric | Pass Gate |
| :--- | :--- | :--- | :--- |
| 1. Baseline | `sbase[1..=1920]`, clamped `(nfa, nfb)` | Max abs diff (dB) & L2 rel err over `[ia..=ib]` | Rel err `≤ 1e-4`, exact `(nfa, nfb)` |
| 2. Spectrogram & Sync8 | `candidate[0..ncand]` (`freq_hz`, `dt_sec`, `sync`) | Top-50 candidate match; sync L2 rel err | Sync rel err `≤ 1e-4`, exact AF/DT on top peaks |
| 3. Downsample | `cx[0..=96000]`, `cd0[0..3200]` for top 5 candidates | L2 relative error `‖a - b‖₂ / ‖b‖₂` | `≤ 1e-4` (expected `~1e-6`) |
| 4. Fine Sync & Peakup | `sync8d` curve (`i0-10..=i0+10`), `delfbest`, `ibest`, `f1`, `xdt` | Exact `ibest` & `delfbest`; `sync8d` L2 rel err | Exact `ibest`, exact `delfbest`, `sync` rel err `≤ 1e-4` |
| 5. Symbol FFT & LLRs | `cs[0..79][0..8]`, `s8[0..79][0..8]`, `nsync`, `bmeta[0..174]`, `llra[0..174]` | Exact `nsync`; L2 rel err on `cs`, `s8`, `llra` | Exact `nsync`, L2 rel err `≤ 1e-4` |
| 6. LDPC BP | `cw[0..174]`, `message91[0..91]`, `nhard_errors`, `dmin`, `zsave` (when `maxosd=2`) | Bit-exact `cw`/`message91`/`nhard_errors`; L2 rel err on `dmin`, `zsave` | 100% bit-exact `cw`; `dmin`/`zsave` rel err `≤ 1e-4` |
| 7. Subtraction | Post-subtraction `dd[0..180000]` (both `refine_dt = false` and `true`) | L2 relative error `‖dd_rs - dd_f90‖₂ / ‖dd_f90‖₂` | `≤ 1e-4` |
| 8. `twkfreq1` | Complex output on test vector (`npts = 2812`) | L2 relative error | `≤ 1e-4` |

### 5.2 End-to-End Oracle & Behavioral Gates

Standardized classic `jt9` oracle command for all Stage 2 differential runs:
```bash
jt9 -8 -d 1 -w 0 -c b -x b -L 200 -H 4000 -f 1500 <file.wav>
```
*(Uses `-c b -x b` so `jt9.f90`'s hardcoded `K1ABC`/`W9XYZ`/`EN37` defaults do not bias AP; omits `-M` to select classic `lib/ft8/`. Rust `DecodeConfig` uses matching `f_low_hz = 200.0`, `f_high_hz = 4000.0`, `nfqso_hz = 1500.0`, `depth = 1`.)*

| Test Suite | Corpus / Parameters | Metric | Pass Gate |
| :--- | :--- | :--- | :--- |
| **Synthetic AWGN Sweep** | Deterministic seeded PRNG in `ftxr-oracle`: `0 dB` to `-18 dB` (1 dB steps, ≥ 50 trials/point, randomized AF `300..2800 Hz`, DT `-1.0..+1.0 s`) | Decode rate across `0..-15 dB` + sensitivity curve vs `jt9 -8 -d 1` across `-16..-18 dB` | **≥ 99% decode rate** from `0 dB` to `-15 dB` |
| **Real-World Corpus Diff** | `samples/FT8/210703_133430.wav`, `191111_110200.wav`, plus Stage 0 curated quiet/crowded/fading FT8 fixtures | Matched decode ratio vs `jt9 -8 -d 1 -c b -x b` | **≥ 90% of `jt9 -8 -d 1` decode count**; zero bogus messages |
| **Measurement Parity** | All matched decodes across synthetic + real corpora | Per-decode deltas (`rust - jt9`) | AF within **±3 Hz**, DT within **±0.15 s**, SNR within **±2 dB** |
| **Crowded Multi-Pass Subtraction** | Synthetic overlapping signals: strong (`0 dB`) + weak (`-14 dB`) separated by `10 Hz` and `25 Hz` | Both signals decoded after Pass 1 → Pass 2 subtraction | **100%** decode of both strong and masked weak signals |
| **Partial-Slot Truncation** | `191111_110200.wav` and synthetic slots truncated to **`11.8 s`** (`141,600` samples) and **`13.5 s`** (`162,000` samples), zero-padded to 15 s | Expected messages decoded (`CQ TA6CQ KN70`, `CQ LZ1JZ KN22`, `CQ R7IW LN35`, `CQ DX R6WA LN32`) | **100%** of expected `11.8 s` / `13.5 s` fixture decodes recovered |
| **5.0 s Late-Start TX** | Stage 1 UTC-aligned FT8 waveform starting at `+5.0 s` (`0..60000` zeroed, matching `digi-dx/tests/modem_integration.rs:376-403`) | Decode recovered via `sync_bc` + BP erasure recovery | **100%** decode, AF within `±3 Hz`, DT aligned within `±0.25 s` |
| **Mid-Slot Hot-Swap TX** | First `1.0 s` message A (`K1ABC N1MPM -08`), swapped at `1.0 s` to UTC-aligned message B (`K1ABC N1MPM RR73`, matching `digi-dx/tests/modem_integration.rs:407-440`) | Replacement decoded; cancelled stale prefix NOT decoded | Decodes B, **0** decodes of A |
| **Pure-Noise False Positives** | ≥ 100 deterministic pure-AWGN slots (various RMS levels from `50` to `5000` `i16` counts) + 1 all-zero silent slot | Total decoded messages | **0 false decodes (0.0%)**, zero panics/NaNs |

---

## 6. Risks, Gotchas & Cross-Stage Handoff

### Key Risks & Mitigations
1. **Silent 1-bin (`3.125 Hz`) or 1-sample (`5 ms` / `83 µs`) indexing drift:** Mitigated by keeping 1-based bin layout (`len = NH1 + 1`) for `s`/`sbase`, preserving `jstrt = 12`, `xdt = (ibest - 1) * 0.005`, and `cw.rotate_left(2001)`, AND enforcing the Stage-by-Stage Fortran dump gate (`≤ 1e-4`) before debugging end-to-end metrics.
2. **`gen_ft8wave` phase LUT vs direct `sin/cos` drift in `subtractft8`:** WSJT-X's `gen_ft8wave.f90:58` uses a 65,536-entry complex LUT (`max phase err ~9.6e-5 rad`). On high-SNR signals (`+20 dB`), a `1e-4` phase error in `cref` could leave slightly larger residual than `1e-4` when compared directly against Fortran `dd`. During Task 8 verification, measure `cref` diff first; if Stage 1 uses direct `f32`/`f64` trig and `dd` diff on a strong signal exceeds `1e-4`, verify residual subtraction power is equal or better than Fortran (and document the LUT quantization cause).
3. **`jt9 -8 -d 1` baseline gap from `llrb..llre` and `a1` CQ:** If any specific real-world fixture shows `< 90%` of `jt9 -8 -d 1` decodes, inspect whether the missed decodes in `jt9` carried `a1` tags or came from `ipass = 2..5` (`llrb`/`llrc`/`llrd`/`llre`). Do **not** hack thresholds to chase Stage 3 features; verify the Stage-by-Stage dump parity first, and note that `bmetd` (`nsym = 1` bit-normalized) is trivial to audit if needed while keeping Stage 3's scope clean.
4. **Input scale differences (`i16` vs normalized `f32`):** Always expose `decode_i16` (exact `jt9` parity without rescaling) alongside `decode_f32` (`* 32768.0`), and confirm scale-invariance of `xsnr2` in a dedicated unit test (`0.1×`, `1.0×`, `10.0×` input amplitudes produce identical SNR within `0.1 dB`).

### Cross-Stage Handoff Contracts

| Direction | Interface / Artifact | Contract |
| :--- | :--- | :--- |
| **Consumed from Stage 1** | `protocol::{unpack77, CallsignHashTable, crc14, get_ft8_tones_from_77bits}` | RX-mode `unpack77` updates hash table; `crc14` matches 96-bit zero-padded check; `get_ft8_tones_from_77bits` outputs `[u8; 79]` in `0..8`. |
| **Consumed from Stage 1** | `modulate::ft8::gen_ft8_complex_wave` | Generates `151,680`-sample complex analytic `exp(+j*phi)` with `BT = 2.0`, `NSPS = 1920`, `nramp = 240` cosine edge ramps (`icmplx = 1` parity). |
| **Exposed to Stage 3 (FT8 Weak-Signal)** | `fec::bp::bp_decode_174_91` + `fec::decode174_91` | Already accumulates `zsave: [[f32; 174]; 3]` at BP iterations `1..=max_osd` (and channel LLR at `max_osd == 0`) and dispatches `max_osd >= 0` to `fec::osd::osd174_91`. |
| **Exposed to Stage 3 (FT8 Weak-Signal)** | `ft8::ft8b` + `ft8::ap` + `ft8::subtract` | `cs`/`s8` extraction, `normalize_bmet`, `apmask` plumbing in `bp_decode_174_91`, and `Ft8Subtractor::subtract(..., refine_dt = true)` are already wired; Stage 3 adds `bmetb..e`, `osd174_91`, `ap` (`a1`–`a6`), and full 3-pass `ndepth = 2/3` behavior without restructuring Stage 2. |
| **Exposed to Stage 4 (FT4 Decoder)** | `dsp::{window, baseline, twkfreq}` & `fec::decode174_91` | FT4 reuses `pctile`, `polyfit_5`, `twkfreq1`, and the identical LDPC(174,91) BP+OSD engine (`decode174_91`). |
| **Exposed to Stage 5 (`ftxr-modem` & Parallelism)** | `Ft8FftPlans` (`Send + Sync`), immutable `Ft8SpectrumSnapshot` (`cx`), per-worker `Ft8bScratch`, `on_decode` callback | Enables zero-refactor `rayon` candidate parallelism per pass with deterministic sequential subtraction, warm plan reuse across slots, and immediate streaming decode emission over stdio IPC. |
