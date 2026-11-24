# Stage 4 Plan — FT4 Decoder Parity (`ftxr-core` + `ftxr-oracle`)

> **Stage:** 4 of 7  
> **Title:** FT4 Decoder Parity  
> **Workspace:** `/home/antigravity/mpmbq2/wsjtx/rust-ftx/`  
> **Primary Oracle:** Classic WSJT-X `jt9 -5 -d 1/2/3` (`lib/ft4/` + `lib/ft4_decode.f90`) and Fortran intermediate dumps  
> **License:** GPLv3 (direct port of WSJT-X Fortran FT4 receive pipeline with attribution in `NOTICE`; zero non-GPL or `wsjtx`/FFTW/Qt/Fortran runtime dependencies in `crates/ftxr-core`)

---

## 1. Header & Scope

### Prerequisites (Consumed from Stages 0–3)
- **Stage 0:** Headless WSJT-X oracle binaries (`jt9`, `ft4code`, `ft4sim`, `ft4sim_mult`) built and callable by `tools/ftxr-oracle`; `samples/FT4/000000_000002.wav` and baseline `jt9 -5 -d 1/2/3` scorecards recorded; GPLv3 workspace scaffolded.
- **Stage 1:**
  - `crates/ftxr-core/src/protocol/`: 77-bit `pack77` / `unpack77`, thread-safe `CallsignHashTable` (with `mycall`/`hiscall` resolution matching `packjt77.f90` `nrx=1`), `crc14` (`0x2757`/`0x6757`), `ldpc::encode174_91`, and `rvec::RVEC` (77-bit FT4 scrambling vector).
  - `crates/ftxr-core/src/modulate/ft4.rs`: `ft4_tones_from_77bits` (`[u8; 77] -> [u8; 103]`) and FT4 GFSK (`BT=1.0`, `h=1.0`, 105-symbol including cosine ramp-up/ramp-down) modulator, extended in Stage 4 (if not already exposed) with a complex analytic waveform generator `gen_ft4_complex_wave` matching `gen_ft4wave(..., icmplx=1)`.
- **Stage 2:**
  - `crates/ftxr-core/src/dsp/`: `fft.rs` (`rustfft`/`realfft` cached plan infrastructure), `window.rs` (`nuttall_window`), `baseline.rs` (`pctile`, `polyfit`/`determ`), `twkfreq.rs` (`twkfreq1`), and `normalizebmet`.
  - `crates/ftxr-core/src/fec/bp.rs`: LDPC(174,91) Belief Propagation solver with `zsave` capture on iterations 1..3.
- **Stage 3:**
  - `crates/ftxr-core/src/fec/osd.rs` & `decode174_91.rs`: Hybrid BP + GF(2) OSD decoder (`osd174_91` supporting `maxosd ∈ {-1, 0, 1, 2, 3}`, `ndeep = 2`, `Keff = 91`, `apmask: &[u8; 174]`) validated 100% against Fortran `osd174_91`.
  - `DecoderEngine`, `Mode::{Ft8, Ft4}`, `DecodeConfig`, `QsoContext`, `ApTag`, `DecodedMessage`, `DecodeTelemetry`, and `FtxrError` public API contracts in `src/lib.rs`.

### Deliverables
1. Complete pure-Rust port of WSJT-X's FT4 receive pipeline in `crates/ftxr-core/src/ft4/` + shared DSP extensions in `crates/ftxr-core/src/dsp/`:
   - `src/ft4/params.rs`: FT4 protocol, spectral, downsampling, sync, Costas, and Gray-map constants.
   - `src/ft4/candidates.rs`: Nuttall-windowed 4-symbol spectrogram, 15-bin smoothed spectrum, `ft4_baseline` polynomial baseline (linear power output), parabolic peak interpolation, and `nfqso ± 20 Hz` stable partition.
   - `src/dsp/downsample.rs`: `Ft4Downsampler` (`NMAX=72,576` R2C FFT, raised-cosine passband filter with `rotate_left(126)`, `NFFT2=4,032` inverse C2C FFT, immutable per-pass spectrum cache `cx`).
   - `src/ft4/sync4d.rs`: 4-Costas-array 2x-decimated coherent correlation with precomputed `ctwk2` frequency-tweak table (`-16..=+16 Hz`) and exact boundary slicing for `i1 < 0` and `i4 + 127 >= 4032`.
   - `src/ft4/bitmetrics.rs`: 103×32-point symbol FFTs, hard sync quality check (`nsync < 8`), 1-/2-/4-symbol coherent bit metrics, tail symbol fallbacks (`200..206`), and `normalizebmet` over all 206 bits.
   - `src/ft4/ap.rs`: FT4-specific `rvec`-scrambled A Priori bit sets (`apbits`, `apmy_ru`, `aphis_fd`, scrambled `mcq*` tables) and `a1`–`a6` hypothesis builder for `ncontest = 0..5` and `nQSOProgress = 0..5`.
   - `src/ft4/subtract.rs`: 72,576-point FFT-convolved Hann² (`NFILT=1400`) complex envelope estimation and FT4 signal subtraction (with `-NSPS` ramp alignment and no `endcorrection`).
   - `src/ft4/decoder.rs` & `src/ft4/mod.rs`: Full 3-pass orchestrator (`ndepth = 1/2/3`), 3-segment DT search (`iseg = 1..3`), `nsync_qual < 20` check, `maxosd = 2` vs `maxosd = 3` near `nfqso`, `rvec` descrambling, SNR/DT/qual calculation, and `DecoderEngine` `Mode::Ft4` wiring.
2. Stage-4 FT4 oracle harness additions in `tools/ftxr-oracle`:
   - Stage-by-stage intermediate numerical comparison against WSJT-X FT4 Fortran.
   - Differential scoring vs `jt9 -5 -d 1/2/3` across deterministic AWGN sweeps (`0` to `-21 dB`), crowded multi-signal fixtures (mirroring `ft4sim_mult` + `messages.txt`), real-world FT4 recordings (`samples/FT4/000000_000002.wav`), pure-noise FP sweeps, and FT4 TX→RX loopback (nominal + `0.5 s–1.5 s` late-start + 90,000-sample truncation).

### Exit Gate Checklist
- [ ] All 6 FT4 intermediate stages (`savg`/`sbase`/`candidates`, `cd2`, `sync4d` `(smax, ibest, idfbest)`, `cb`/`cd`, `bitmetrics`/`llr`, post-subtraction residual `dd`) match WSJT-X Fortran within `≤1e-4` relative error on reference fixtures.
- [ ] **≥95% of `jt9 -5 -d 3` decode count** across crowded synthetic (`ft4sim_mult`-style) and real-world FT4 corpora (`samples/FT4/000000_000002.wav` + curated fixtures), and ≥95% parity at `-d 1` and `-d 2`.
- [ ] Synthetic AWGN 50% decode threshold within **≤0.5–1.0 dB** of `jt9 -5 -d 3` across `0` to `-21 dB`.
- [ ] Measurement agreement vs `jt9 -5`: AF within ±2 Hz, DT within ±0.1 s, SNR within ±2 dB.
- [ ] False-decode rate on pure AWGN noise slots **≤ `jt9 -5 -d 3`** (both blind and with AP `a1`–`a6` enabled; uncertain AP decodes tagged `? a*` when `qual < 0.17`).
- [ ] **100% loopback decode rate** on Rust-modulated FT4 signals: nominal start (`300 ms` delay), `ft4sim`-aligned (`dt = 0.0 s`), UTC-aligned late-start (`0.5 s`, `1.0 s`, `1.5 s`), clock-shifted (`dt ∈ [-0.8 s, +0.9 s]`), and full 7.5 s (`90,000`-sample) input buffers.
- [ ] Zero regressions on Stage 1–3 FT8 tests and `ftxr-core` builds cleanly with zero `wsjtx`/FFTW/Fortran/Qt dependencies.

---

## 2. Source Audit & Technical Reference

Every file, constant, array bound, and formula below was verified directly from `/home/antigravity/mpmbq2/wsjtx/`.

### 2.1 Audited WSJT-X Files & Roles

| WSJT-X File | Subroutines / Entries | Role in FT4 Receive Pipeline |
| :--- | :--- | :--- |
| `lib/ft4/ft4_params.f90` | Parameters only | Core dimensions (`KK`, `ND`, `NS`, `NN`, `NN2`, `NSPS`, `NZ`, `NZ2`, `NMAX`, `NFFT1`, `NH1`, `NSTEP`, `NHSYM`, `NDOWN`) |
| `lib/ft4_decode.f90` (468 lines) | `ft4_decoder%decode` | Top-level 3-pass orchestrator, `ctwk2`, `rvec` AP tables & `apbits`, 3-segment DT search, `cb`→`cd` slicing, `nsync_qual`, LLR extraction, `decode174_91` (`maxosd=2/3`), `rvec` descrambling, SNR/DT/`qual` |
| `lib/ft4/getcandidates4.f90` (84 lines) | `getcandidates4` | Nuttall window, 122-frame 2304-pt spectrogram, 15-bin boxcar smoothing, `ft4_baseline`, parabolic peak interpolation, `f_offset`, `nfqso ± 20 Hz` stable partition |
| `lib/ft4/ft4_baseline.f90` (50 lines) | `ft4_baseline` | 10-segment 10th-percentile lower-envelope 5-term polynomial fit in dB + `0.65 dB` offset, converted back to **linear** power |
| `lib/ft4/ft4_downsample.f90` (49 lines) | `ft4_downsample` | 72,576-pt R2C FFT (cached per pass), raised-cosine passband window with `cshift(..., 126)`, 4,032-pt inverse C2C FFT scaled by `/4032` |
| `lib/ft4/sync4d.f90` (97 lines) | `sync4d` | 4-Costas-array 2x-decimated (`2*NSS=64`) coherent correlation + amplitude sum `∑|z_k|/64`, boundary clamping for `i1 < 0` and `i4+127 > 4031` |
| `lib/ft4/get_ft4_bitmetrics.f90` (117 lines) | `get_ft4_bitmetrics` | 103×32-pt symbol FFTs, hard sync check (`nsync < 8`), 1-/2-/4-symbol coherent bit metrics over Gray map `[0,1,3,2]`, tail fallbacks, `normalizebmet(..., 206)` |
| `lib/ft4/subtractft4.f90` (67 lines) | `subtractft4` | Complex `gen_ft4wave` reference (`NFRAME=60,480`), `nstart = dt*12000 + 1 - 576`, 72,576-pt FFT convolution with `NFILT=1400` `cos²` kernel, subtract `2*Re(cfilt*cref)` |
| `lib/ft4/genft4.f90` (91 lines) | `genft4`, `get_ft4_tones_from_77bits` | `rvec` scrambling, `encode174_91`, 4-FSK Gray map, interleaving 4 Costas arrays + 87 data symbols (`103` tones) |
| `lib/ft4/gen_ft4wave.f90` (67 lines) | `gen_ft4wave` | Continuous-phase 4-GFSK (`BT=1.0`, `h=1.0`, 3-symbol pulse) + 1-symbol cosine ramp-up and ramp-down (`60,480` samples) |
| `lib/nuttal_window.f90` (16 lines) | `nuttal_window` | Periodic 4-term Nuttall window (`a0=0.3635819, a1=-0.4891775, a2=0.1365995, a3=-0.0106411`, denominator `n`) |
| `lib/ft8/twkfreq1.f90` (27 lines) | `twkfreq1` | Complex frequency-mixing routine (`twopi = 6.283185307`, `cb(i) = exp(j*i*dphi) * ca(i)`) |
| `lib/ft8/ft8b.f90` (lines 493–506) | `normalizebmet` | Divides slice by `sqrt(E[x²] - E[x]²)` (fallback `sqrt(E[x²])`), without subtracting mean |
| `lib/pctile.f90`, `lib/shell.f90`, `lib/polyfit.f90`, `lib/determ.f90` | `pctile`, `polyfit` | Shell-sorted `nint(0.01*npct*npts)` percentile and Cramer's-rule `real*8` polynomial least-squares fit |
| `lib/ft8/decode174_91.f90` & `osd174_91.f90` | `decode174_91`, `osd174_91` | Shared LDPC(174,91) hybrid BP (30 iter) + GF(2) OSD (`maxosd` up to 3, `ndeep = 2`, `Keff = 91`) |
| `lib/jt9.f90` & `lib/decoder.f90` | `jt9`, `multimode_decoder`, `ft4_decoded` | CLI `-5` (`mode=5`), reads `npts = 21*3456 = 72,576` samples (single pass, no early `41/47/50` passes), formats `' + '` marker and `?`/`aN` annotation (`qual < 0.17`) |
| `lib/ft4/ft4sim.f90`, `ft4sim_mult.f90`, `messages.txt` | Reference simulators | Signal alignment `k = nint((xdt+0.5)*12000) - NSPS`, SNR2500 formula `sqrt(2*2500/6000)*10^(snrdb/20)`, multi-signal crowded test files |
| `Modulator/Modulator.cpp` (line 79) & `widgets/mainwindow.cpp` (lines 2226, 2507) | Live TX/RX timing | FT4 TX nominal delay `300 ms`; live RX captures `m_hsymStop = 21` chunks (`21*3456 = 72,576` samples = `6.048 s`) |

### 2.2 Exact FT4 Constants & Derived Values

| Constant | Value | Derivation / Notes |
| :--- | :--- | :--- |
| `FS_HZ` | `12_000.0` | Input audio sample rate (Hz) |
| `KK` | `91` | 77 payload bits + 14 CRC bits |
| `ND` | `87` | Data symbols (`2 * ND = 174` LDPC codeword bits) |
| `NS` | `16` | Sync symbols (4 Costas arrays × 4 symbols) |
| `NN` | `103` | Sync + data symbols (`NS + ND`), offsets: `s4[0..4] d29[4..33] s4[33..37] d29[37..66] s4[66..70] d29[70..99] s4[99..103]` |
| `NN2` | `105` | Shaped TX symbols (`1 ramp-up + 103 + 1 ramp-down`) |
| `NSPS` | `576` | Samples per symbol at 12 kHz (`0.048 s/sym`, `baud = 12000/576 = 20.833333 Hz`) |
| `NZ` | `59_328` | `NSPS * NN` (`4.944 s` sync + data) |
| `NZ2` / `NFRAME` | `60_480` | `NSPS * NN2` (`5.040 s` full shaped waveform with ramps) |
| `NMAX` | `72_576` | `21 * 3456` (`6.048 s` capture buffer; prime factorization **`2^7 × 3^4 × 7`**, 7-smooth!) |
| `NFFT1` / `NH1` | `2304` / `1152` | 4-symbol candidate FFT (`2^8 × 3^2`, 3-smooth; `df1 = 12000/2304 = 5.208333 Hz = baud/4`) |
| `NSTEP` | `576` | `NSPS` (1-symbol step! Comment in `ft4_params.f90:15` saying "1/4-sym steps" is stale) |
| `NHSYM` | `122` | `(NMAX - NFFT1) / NSTEP = 70272 / 576 = 122` (exact; stops at sample `72000`, omitting final 576 samples) |
| `NDOWN` | `18` | Downsampling factor (`fs_down = 12000/18 = 666.66667 Hz`) |
| `NSS` | `32` | `NSPS / NDOWN = 576 / 18 = 32` (`2^5` samples/symbol at baseband) |
| `NDMAX` / `NFFT2` | `4032` | `NMAX / NDOWN = 72576 / 18 = 4032` (**`2^6 × 3^2 × 7`**, 7-smooth!) |
| `MAXCAND` | `200` | Maximum candidates per subtraction pass |
| `SYNCMIN` | `1.18` | Peak threshold on `savsm / sbase` in `getcandidates4` |
| `F_OFFSET` | `-31.25` | `-1.5 * 12000.0 / 576.0` (shifts spectral centroid of 4-FSK down to tone 0) |
| `IWT`, `IWF`, `IWS` | `63`, `504`, `126` | `ft4_downsample` filter transition/flat/shift bins (`df = 12000/72576 = 0.1653439 Hz`) |
| `NFILT` | `1400` | `subtractft4` Hann² lowpass filter width (1401 taps `-700..=+700`, `cshift` left by `701`) |
| `SCALEFAC` | `2.83` | Bit-metric to LLR multiplier |
| `NAPWID` | `50.0` | Hardcoded AP & `maxosd=3` half-width around `nfqso` (Hz) |
| `DT_RATE_LITERAL` | `666.67` | Literal denominator in `ft4_decode.f90:436,454` (`dt = ibest/666.67`, `xdt = ibest/666.67 - 0.5`) |
| `ICOS4A..D` | `[0,1,3,2]`, `[1,0,2,3]`, `[2,3,1,0]`, `[3,2,0,1]` | Four 4×4 Costas arrays at 0-based symbol offsets `0, 33, 66, 99` |
| `GRAYMAP4` | `[0, 1, 3, 2]` | 2-bit integer `0..=3` → tone index `0..=3` (`00→0, 01→1, 10→3, 11→2`) |
| `SYNC_BITS4` | `a:[0,0,0,1,1,0,1,1]`, `b:[0,1,0,0,1,1,1,0]`, `c:[1,1,1,0,0,1,0,0]`, `d:[1,0,1,1,0,0,0,1]` | 32 expected hard bits at 0-based bit indices `0..8`, `66..74`, `132..140`, `198..206` |

### 2.3 Non-Obvious Algorithm Details & Verified Fortran Gotchas

1. **All FT4 FFT Lengths Are 7-Smooth (`2^a × 3^b × 7^c`), Zero Bluestein Penalty:**
   - `NMAX = 72,576 = 2^7 × 3^4 × 7` (`NMAX/2 = 36,288 = 2^6 × 3^4 × 7` for `realfft`).
   - `NFFT2 = 4,032 = 2^6 × 3^2 × 7`.
   - `NFFT1 = 2,304 = 2^8 × 3^2` (`NFFT1/2 = 1,152 = 2^7 × 3^2`).
   - `NSS = 32 = 2^5`.
   - `rustfft` v6 has native butterflies for radices 2, 3, 4, 5, 7 and mixed-radix Cooley-Tukey composition; none of these lengths fall back to Bluestein's algorithm. Plans MUST still be precomputed once and reused across slots via `Ft4Plans`.

2. **Immutable Pass-Start Spectrum `cx` Inside Each Subtraction Pass (`ft4_decode.f90:223–230, 438`):**
   - `dobigfft` is set `.true.` once per `isp` pass, consumed on `icand = 1`, and **never** reset to `.true.` when `subtractft4` mutates `dd` during that pass.
   - All candidates within pass `isp` downsample from the **same** `cx = R2C_FFT(dd_pass_start)`. Subtracted residuals take effect on pass `isp + 1` when `getcandidates4` re-runs and `cx` is recomputed.
   - This makes per-pass `cx` naturally immutable in Rust (zero `save` state, and enables deterministic Stage 5 `rayon` candidate parallelism over `&cx` with zero sensitivity drift!).

3. **`ibest < 0` Slicing Quirk in `ft4_decode.f90:294`:**
   ```fortran
   if(ibest.ge.0) then
      it=min(NDMAX-1,ibest+NN*NSS-1)
      np=it-ibest+1
      cd(0:np-1)=cb(ibest:it)
   else
      cd(-ibest:ibest+NN*NSS-1)=cb(0:NN*NSS+2*ibest-1)
   endif
   ```
   - With `L = NN*NSS = 3296`, when `ibest < 0` (segment 3, `xdt < -0.5 s`, worst case `ibest = -349`), the upper bound `ibest + L - 1` makes BOTH slices length `L + 2*ibest` instead of `L + ibest`, **accidentally zeroing the trailing `|ibest|` samples of `cd`**!
   - **Design decision:** Match Fortran's exact `else` slice in Stage 4 (behind a well-documented helper `slice_frame_cd` with a unit test covering both `ibest >= 0` and `ibest < 0`) so that stage-by-stage oracle diffs against WSJT-X Fortran agree to `<1e-4` even on segment-3 candidates. Because `ibest < 0` only affects extreme early clock offsets (`xdt < -0.5 s`), revisit only after Stage 4 parity is proven (if ever desired as a deliberate post-parity enhancement).

4. **`sync4d` 2× Decimation, Amplitude Metric & Boundary Math (`sync4d.f90`):**
   - Unlike FT8's `sync8d` (which sums non-coherent per-symbol power `|z|²` at full 32 sym/s), FT4's `sync4d` decimates by 2 (stride `2`, `64` samples per 4-symbol Costas array), correlates **coherently** across each 4-symbol Costas array, and sums **amplitudes** `|z_k| / 64.0`.
   - `ctwk2` is generated with `twkfreq1(ones_64, 64, fs / 2.0, [idf, 0, 0, 0, 0])` for `idf = -16..=+16` (note `fs / 2.0 = 333.3333 Hz`!).
   - For `i1 < 0`: `npts = (i1 + 127) / 2` (truncating toward zero; if `npts <= 16` → `z1 = 0`). Otherwise sum `m = 0..=npts` (`npts + 1` elements!) of `cd0[2*m] * csync2[63 - npts + m].conj()`.
   - For `i4 + 127 > 4031`: `npts = (4032 - i4) / 2` (if `npts <= 16` → `z4 = 0`). Otherwise sum `m = 0..npts` (`npts` elements!) of `cd0[i4 + 2*m] * csync2[m].conj()`.

5. **Two Different Normalizations (`cd2` vs `cb`) and 3-Segment `smax1` Gate (`ft4_decode.f90:231, 246, 278–280, 286`):**
   - `cd2` divides energy by `NDMAX = 4032.0`; `cb` divides energy by `NN * NSS = 3296.0`.
   - `iseg = 1` searches `ib ∈ [108, 560]` (`xdt ∈ [-0.34, +0.34] s`) and stores `smax1 = smax`.
   - `iseg = 2` searches `ib ∈ [560, 1012]` (`[+0.34, +1.02] s`), `iseg = 3` searches `ib ∈ [-344, 108]` (`[-1.02, -0.34] s`).
   - Segments 2 and 3 require BOTH `smax >= 1.2` AND `smax >= smax1` (compared specifically against segment 1's `smax1`, NOT `max(smax1, smax2)`!).
   - Once any segment decodes (`nharderror >= 0`, including duplicate or failed `unpack77`, excluding all-zero `message77` unless on the final `ipass`), exit the `iseg` loop immediately.

6. **`rvec` Scrambling Inside FT4 AP Construction vs FT8 (`ft4_decode.f90:111–118, 151–183, 323–410`):**
   - FT4 XORs the 77-bit message with `rvec` **before** CRC-14 and LDPC(174,91) encoding. Therefore ALL LDPC LLRs and AP priors live in the **scrambled** domain!
   - `mcq`, `mcqru`, `mcqfd`, `mcqtest`, `mcqww` are XORed with `rvec[0..29]` and mapped `{0,1} -> {-1,+1}`.
   - `apbits` encodes `"{mycall} {hiscall0} RR73"` (with `hiscall0 = mycall` if `hiscall` is empty/short, `nohiscall = true`), validates `i3 == 1 && message == msgsent && unpk77_success`, builds shifted contest vectors BEFORE scrambling (`apmy_ru` uses `rvec[1..29]`, `aphis_fd` uses `rvec[28..56]`), then scrambles `msg77 ^ rvec`, calls `encode174_91`, maps `2*cw - 1`, and sets `apbits[29] = 99` sentinel if `nohiscall`.
   - **Unlike FT8:**
     - `naptypes` for QSO states 3 & 4 is `[3, 6]` (`nappasses = 2`), **NOT** `[3, 4, 5, 6]` (FT4 never runs `a4`/`a5`, only `a1, a2, a3, a6`; `mrrr`/`m73`/`mrr73` are unused).
     - `a1`, `a2`, `a3` do **NOT** mask/set bits `75..=77` (`i3`).
     - `apmag = max(|llra|) * 1.1` (from 1-sym `llra`), while AP base LLRs always copy `llrc` (4-sym coherent) for a single pass per AP hypothesis (not alternating `llra`/`llrc`).
     - `napwid = 50` Hz is hardcoded and checks only `|f1 - nfqso| <= 50` (not `nftx`).
   - All-zero codeword rejection `sum(message77) == 0` runs **BEFORE** XORing `rvec` back out (since `cw = 0` is in the scrambled domain).

7. **Depth Semantics & `maxosd = 3` Near `nfqso` (`ft4_decode.f90:190–206, 414–420`):**
   - `ndepth = 1`: 1 pass (`nsp = 1`), no subtraction, BP-only (`maxosd = -1`), no AP (`npasses = 3`).
   - `ndepth = 2`: up to 3 passes (`nsp = 3`), subtraction ON, **BP-only** (`maxosd = -1` — unlike FT8 where `-d 2` already runs OSD!), AP ON.
   - `ndepth = 3`: up to 3 passes (`nsp = 3`), subtraction ON, BP + OSD (`ndeep = 2`, `maxosd = 2` normally, **`maxosd = 3` when `|nfqso - f1| <= 50 Hz`**), AP ON.
   - Between passes: exit before pass 2 if `ndecodes == 0`; exit before pass 3 if pass 2 found `0` new decodes (`nd2 == 0`).

8. **`subtractft4` Offsets, Filter `cshift` & Subtraction-Before-Dedupe (`subtractft4.f90` & `ft4_decode.f90:434–445`):**
   - `nstart0 = (dt * 12000.0_f32) as i32 - 576` (`- NSPS` aligns `gen_ft4wave`'s 1-symbol ramp-up ahead of symbol 1; FT8 has no ramp symbol so FT8 has no `- NSPS`).
   - Filter kernel `cw` uses `rotate_left(NFILT / 2 + 1)` (`701`, matching Fortran `cshift(cw, 701)` — 1-sample lag parity with WSJT-X).
   - **No `endcorrection`** and **no `lrefinedt`** (unlike `subtractft8`).
   - `subtractft4` runs **before** duplicate suppression (`idupe`), so re-decoded duplicates still get subtracted from `dd` before returning.
   - `get_ft4_tones_from_77bits` receives **unscrambled** `message77` and re-scrambles internally before `encode174_91`.

9. **Input Audio Length Handling (`NMAX = 72,576` vs 7.5 s = `90,000` Samples):**
   - WSJT-X captures `21 * 3456 = 72,576` samples (`6.048 s`) for FT4 (`jt9.f90:306`, `mainwindow.cpp:2507`).
   - Callers (e.g., `digi-dx` or CLI users recording full 7.5 s slots) may pass **90,000 samples**, or early/partial buffers `< 72,576` samples (e.g., 5.5 s = 66,000 samples).
   - **Contract in `ftxr-core`:** `prepare_ft4_buffer(samples: &[f32], dd: &mut [f32; NMAX])` copies `samples[..samples.len().min(NMAX)]` into `dd` and zero-fills any tail `samples.len()..NMAX`. Samples beyond `72,576` are deterministically truncated (matching `jt9 -5` exact behavior), and an empty or all-zero buffer cleanly returns `Ok(Vec::new())` without NaN/division-by-zero in `ft4_baseline`.

10. **Exact Output Formatting & Tie-Breaking Details:**
    - `getcandidates4` uses 1-based FFT bin indexing `1..=1152` (excluding DC bin `0`), `ft4_baseline` outputs **linear** power (unlike FT8's dB `sbase`), and `getcandidates4` **does NOT sort** by SNR—it stably partitions `|fpeak - nfqso| <= 20.0` ahead of all other peaks in ascending frequency order.
    - `get_ft4_bitmetrics`: `argmax` on `s4[:, k]` MUST break ties by choosing the **first** (lowest) tone index (matching Fortran `maxloc`), and tail fallbacks (`bitmetrics[204..206][1]`, `bitmetrics[200..204][2]`, `bitmetrics[204..206][2]`) MUST be copied **before** calling `normalizebmet(..., 206)`.
    - `SNR = ((10.0 * (speak - 1.0).log10() - 14.8).max(-21.0)).round()` (or `-21` if `speak <= 1.0`), `xdt = (ibest as f32) / 666.67 - 0.5`, `qual = 1.0 - (nharderror as f32 + dmin) / 60.0`, uncertain flag `?` when `nap != 0 && qual < 0.17`.
    - `jt9 -5` prints `' + '` as the mode marker (`decoder.f90:1828`), unlike FT8's `' ~ '`.

---

## 3. Architecture & Concrete Rust Design

### 3.1 Files to Create / Edit

```text
rust-ftx/
├── crates/ftxr-core/src/
│   ├── lib.rs                    # [EDIT] Wire Mode::Ft4 into DecoderEngine + Ft4Plans cache
│   ├── telemetry.rs              # [EDIT] Ensure DecodeTelemetry supports FT4 fields (dt_segment, nsync_bits, etc.)
│   ├── modulate/ft4.rs           # [EDIT] Expose gen_ft4_complex_wave(&[u8; 103], f32, &mut [Complex32]) if not yet public
│   ├── dsp/
│   │   ├── fft.rs                # [EDIT] Add cached FT4 FFT plans (2304 R2C, 72576 R2C, 4032 C2C inv, 72576 C2C fwd/inv)
│   │   ├── baseline.rs           # [EDIT] Add ft4_baseline(&[f32], usize, usize, &mut [f32]) -> bool (linear power output)
│   │   └── downsample.rs         # [EDIT] Add Ft4Downsampler (window, rotate_left(126), immutable cx snapshot)
│   └── ft4/
│       ├── mod.rs                # [CREATE] Public module re-exports & decode_ft4 entry point
│       ├── params.rs             # [CREATE] All FT4 constants, Costas arrays, Gray map, sync bit patterns
│       ├── candidates.rs         # [CREATE] getcandidates4 (Nuttall spectrogram, 15-bin smooth, baseline, parabolic peaks, partition)
│       ├── sync4d.rs             # [CREATE] Precomputed Costas templates + ctwk2(-16..=16) + sync4d correlation
│       ├── bitmetrics.rs         # [CREATE] 103×32 symbol FFT, nsync check, 1/2/4-sym coherent metrics, tail fallback, normalizebmet
│       ├── ap.rs                 # [CREATE] Ft4ApTables (scrambled mcq*), Ft4ApSet (apbits/apmy_ru/aphis_fd), apply_ft4_ap
│       ├── subtract.rs           # [CREATE] Ft4Subtractor (precomputed cw kernel + subtractft4)
│       └── decoder.rs            # [CREATE] Ft4Plans, Ft4Scratch, 3-pass decode orchestrator & single-candidate evaluator
└── tools/ftxr-oracle/src/
    ├── jt9_runner.rs             # [EDIT] Support Mode::Ft4 (`jt9 -5`, parse `' + '` lines)
    ├── sweep.rs                  # [EDIT] Add deterministic FT4 AWGN sweep (0..-21 dB) + crowded multi-signal FT4 generator
    ├── ft4_dump_diff.rs          # [CREATE] Stage-by-stage Fortran vs Rust intermediate numerical verifier
    └── main.rs                   # [EDIT] Add `ft4-parity`, `ft4-dump-diff`, `ft4-sweep`, `ft4-loopback` subcommands
```

### 3.2 Concrete Rust Structs, Enums & Function Signatures

#### `crates/ftxr-core/src/ft4/params.rs`
```rust
pub const FS_HZ: f32 = 12_000.0;
pub const KK: usize = 91;
pub const ND: usize = 87;
pub const NS: usize = 16;
pub const NN: usize = NS + ND;             // 103
pub const NN2: usize = NS + ND + 2;        // 105
pub const NSPS: usize = 576;
pub const NZ: usize = NSPS * NN;           // 59_328
pub const NZ2: usize = NSPS * NN2;         // 60_480
pub const NMAX: usize = 21 * 3456;         // 72_576 (2^7 * 3^4 * 7)
pub const NFFT1: usize = 2304;             // 2^8 * 3^2
pub const NH1: usize = NFFT1 / 2;          // 1152
pub const NSTEP: usize = NSPS;             // 576
pub const NHSYM: usize = (NMAX - NFFT1) / NSTEP; // 122
pub const NDOWN: usize = 18;
pub const NSS: usize = NSPS / NDOWN;       // 32
pub const NDMAX: usize = NMAX / NDOWN;     // 4032 (2^6 * 3^2 * 7)
pub const NFRAME_DOWN: usize = NN * NSS;   // 3296
pub const NBITS: usize = 2 * NN;           // 206
pub const NCW: usize = 2 * ND;             // 174

pub const MAXCAND: usize = 200;
pub const SYNCMIN: f32 = 1.18;
pub const SYNC4D_MIN: f32 = 1.2;
pub const SCALEFAC: f32 = 2.83;
pub const NAPWID_HZ: f32 = 50.0;
pub const NFQSO_PRIOR_HZ: f32 = 20.0;
pub const DT_RATE_LITERAL: f32 = 666.67;
pub const SUBTRACT_NFILT: usize = 1400;

pub const ICOS4A: [usize; 4] = [0, 1, 3, 2];
pub const ICOS4B: [usize; 4] = [1, 0, 2, 3];
pub const ICOS4C: [usize; 4] = [2, 3, 1, 0];
pub const ICOS4D: [usize; 4] = [3, 2, 0, 1];
pub const GRAYMAP4: [usize; 4] = [0, 1, 3, 2];

pub const SYNC_BITS_A: [u8; 8] = [0, 0, 0, 1, 1, 0, 1, 1];
pub const SYNC_BITS_B: [u8; 8] = [0, 1, 0, 0, 1, 1, 1, 0];
pub const SYNC_BITS_C: [u8; 8] = [1, 1, 1, 0, 0, 1, 0, 0];
pub const SYNC_BITS_D: [u8; 8] = [1, 0, 1, 1, 0, 0, 0, 1];

/// 3 DT search segments `(ibmin, ibmax)` in downsampled samples (step 4).
pub const DT_SEGMENTS: [(i32, i32); 3] = [
    (108, 560),    // iseg 1: nominal [-0.34, +0.34] s
    (560, 1012),   // iseg 2: late    [+0.34, +1.02] s
    (-344, 108),   // iseg 3: early   [-1.02, -0.34] s
];
```

#### `crates/ftxr-core/src/dsp/downsample.rs` (FT4 extension)
```rust
pub struct Ft4DownsamplePlan {
    r2c_nmax: Arc<dyn RealToComplex<f32>>,       // len 72_576
    c2c_inv_nfft2: Arc<dyn Fft<f32>>,            // len 4_032 (Inverse)
    window: [f32; NDMAX],                        // raised-cosine rotated left by 126, pre-scaled by 1/4032
}

/// Immutable frequency-domain snapshot computed once per subtraction pass.
pub struct Ft4PassSpectrum {
    pub cx: Vec<Complex32>,                      // len NMAX/2 + 1 = 36_289
}

impl Ft4DownsamplePlan {
    pub fn new(cache: &mut FftPlanCache) -> Self;

    /// Compute `cx = R2C_FFT(dd)` once at pass start (`dobigfft = true`).
    pub fn compute_pass_spectrum(
        &self,
        dd: &[f32; NMAX],
        scratch: &mut [Complex32],
        out: &mut Ft4PassSpectrum,
    );

    /// Bandpass-filter and downsample around `f0_hz` into `out_cd` (len 4032).
    /// Pure read of `&Ft4PassSpectrum` — safe for concurrent calls in Stage 5!
    pub fn downsample_at_freq(
        &self,
        spec: &Ft4PassSpectrum,
        f0_hz: f32,
        c1_buf: &mut [Complex32; NDMAX],
        fft_scratch: &mut [Complex32],
        out_cd: &mut [Complex32; NDMAX],
    );
}
```
*Note on pre-scaling `window` by `1.0 / (NDMAX as f32)`: multiplying `c1[k] *= window[k]` where `window[k]` is already divided by `4032.0` matches Fortran `c1 = c1 * window / NFFT2` within 1 ULP while saving 4032 divisions per candidate.*

#### `crates/ftxr-core/src/ft4/candidates.rs`
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ft4Candidate {
    pub freq_hz: f32,   // fpeak (base tone 0 frequency)
    pub peak_snr: f32,  // speak (savsm / sbase >= 1.18); linear SNR estimate = peak_snr - 1.0
}

pub struct Ft4CandidatePlan {
    r2c_nfft1: Arc<dyn RealToComplex<f32>>,      // len 2304
    window: [f32; NFFT1],                        // periodic Nuttall window
}

impl Ft4CandidatePlan {
    pub fn new(cache: &mut FftPlanCache) -> Self;

    /// Runs `getcandidates4`: 122-frame Nuttall spectrogram -> 15-bin smooth -> `ft4_baseline` ->
    /// parabolic peak interpolation -> `nfqso ± 20 Hz` stable partition (max 200 candidates).
    /// Uses 1-based bin indexing `[0..=NH1]` internally to match Fortran bin indices 1-to-1.
    pub fn find_candidates(
        &self,
        dd: &[f32; NMAX],
        fa_hz: f32,
        fb_hz: f32,
        syncmin: f32,
        nfqso_hz: f32,
        scratch: &mut Ft4CandidateScratch,
        out: &mut Vec<Ft4Candidate>,
    );
}
```

#### `crates/ftxr-core/src/ft4/sync4d.rs`
```rust
/// Precomputed Costas templates (len 64) and frequency-tweak vectors for `idf ∈ -16..=+16`.
pub struct Ft4SyncTables {
    /// Pre-multiplied tweaked templates `[4 Costas arrays][33 idf slots][64 samples]`!
    /// Storing `tweaked[costas_idx][idf + 16][k] = (ctwk2[idf][k] * csync[costas_idx][k]).conj()`
    /// eliminates 256 complex multiplies + conjugates on every single `sync4d` call!
    tweaked_conj: Box<[[[Complex32; 2 * NSS]; 33]; 4]>,
}

impl Ft4SyncTables {
    pub fn new() -> Self;

    /// Computes FT4 coherent 4-Costas sync amplitude `p(z1)+p(z2)+p(z3)+p(z4)`.
    /// `idf` must be in `-16..=16`.
    #[inline]
    pub fn sync4d(&self, cd0: &[Complex32; NDMAX], i0: i32, idf: i32) -> f32;
}
```
*Performance insight:* Precomputing `tweaked_conj[c][idf + 16][k] = (ctwk2[idf][k] * csync_c[k]).conj()` once in `Ft4SyncTables::new()` uses only `4 × 33 × 64 × 8 = 67.5 KB` (fits cleanly in L2 cache!) and turns each `sync4d` call into four tight 64-element strided dot products (`cd0[idx + 2*m] * tweaked_conj[c][slot][m]`), cutting `sync4d` math in half with **bit-identical** results!

#### `crates/ftxr-core/src/ft4/bitmetrics.rs`
```rust
pub struct Ft4BitMetricsPlan {
    c2c_fwd_32: Arc<dyn Fft<f32>>,               // len 32 (Forward)
}

pub struct Ft4BitMetricsResult {
    pub bitmetrics: [[f32; 3]; NBITS],           // normalized 1-, 2-, 4-symbol metrics (len 206)
    pub nsync_hard: u8,                          // 0..=16 matching Costas hard tones
    pub s4_peak_tones: [f32; NN],                // optional telemetry envelope hook
}

impl Ft4BitMetricsPlan {
    pub fn new(cache: &mut FftPlanCache) -> Self;

    /// Returns `None` if `badsync` (`nsync_hard < 8`).
    pub fn compute(
        &self,
        cd: &[Complex32; NFRAME_DOWN],
        scratch: &mut Ft4BitMetricsScratch,
    ) -> Option<Ft4BitMetricsResult>;
}

/// Counts matching hard sync bits (`bitmetrics[i][0] >= 0.0`) across the 4 Costas blocks (0..=32).
pub fn count_sync_bit_quality(bitmetrics: &[[f32; 3]; NBITS]) -> u8;

/// Extracts the 3 data-bit blocks `[8..66]`, `[74..132]`, `[140..198]` scaled by `SCALEFAC = 2.83`.
pub fn extract_llrs(bitmetrics: &[[f32; 3]; NBITS]) -> ([f32; NCW], [f32; NCW], [f32; NCW]);
```

#### `crates/ftxr-core/src/ft4/ap.rs`
```rust
/// Static rvec-scrambled CQ hypothesis tables (`{-1, +1}`, len 29).
pub struct Ft4ApTables {
    pub mcq: [i8; 29],
    pub mcqru: [i8; 29],
    pub mcqfd: [i8; 29],
    pub mcqtest: [i8; 29],
    pub mcqww: [i8; 29],
}

/// Per-decode-call QSO-context AP priors (pure, stateless, thread-safe).
#[derive(Debug, Clone)]
pub struct Ft4ApSet {
    pub has_mycall: bool,        // corresponds to apbits(1) <= 1
    pub has_hiscall: bool,       // corresponds to apbits(30) <= 1
    pub apbits: [i8; 77],        // scrambled 77-bit message of "mycall hiscall0 RR73" in {-1,+1}
    pub apmy_ru: [i8; 28],       // message77[0..28] ^ rvec[1..29] in {-1,+1}
    pub aphis_fd: [i8; 28],      // message77[29..57] ^ rvec[28..56] in {-1,+1}
}

impl Ft4ApSet {
    /// Builds FT4 AP priors from `QsoContext` using `pack77`, `unpack77(nrx=1)`, `RVEC`.
    pub fn from_context(qso: &QsoContext, hash_table: &CallsignHashTable) -> Self;
}

/// Returns the ordered AP hypotheses (`ApTag::A1/A2/A3/A6`) for `(qso_progress, ap_cq_only, depth, contest)`.
pub fn ft4_ap_schedule(
    qso_progress: u8,
    ap_cq_only: bool,
    ap_enabled: bool,
    depth: u8,
    contest: u8,
) -> &'static [u8];

/// Attempts to build `(llr, apmask)` for a given `iaptype ∈ {1,2,3,6}` starting from `llrc`.
/// Returns `None` if bailout conditions fail (`|f1 - nfqso| > 50`, missing/nonstandard calls, etc.).
pub fn build_ft4_ap_pass(
    tables: &Ft4ApTables,
    ap_set: &Ft4ApSet,
    iaptype: u8,
    contest: u8,
    f1_hz: f32,
    nfqso_hz: f32,
    llrc: &[f32; NCW],
    apmag: f32,
    out_llr: &mut [f32; NCW],
    out_mask: &mut [u8; NCW],
) -> bool;
```

#### `crates/ftxr-core/src/ft4/subtract.rs`
```rust
pub struct Ft4Subtractor {
    c2c_fwd_nmax: Arc<dyn Fft<f32>>,             // len 72_576 (Forward)
    c2c_inv_nmax: Arc<dyn Fft<f32>>,             // len 72_576 (Inverse)
    cw: Vec<Complex32>,                          // len 72_576, precomputed FFT(rotate_left(hann2/sum, 701)) / 72576
}

impl Ft4Subtractor {
    pub fn new(cache: &mut FftPlanCache) -> Self;

    /// Subtracts an FT4 signal (`tones: [0..=3; 103]`, `f0_hz`, `dt_sec = ibest / 666.67`)
    /// from `dd` in place. Matches `subtractft4.f90` (no `endcorrection`, `-NSPS` alignment).
    pub fn subtract(
        &self,
        dd: &mut [f32; NMAX],
        tones: &[u8; NN],
        f0_hz: f32,
        dt_sec: f32,
        scratch: &mut Ft4SubtractScratch,
    );
}
```

#### `crates/ftxr-core/src/ft4/decoder.rs`
```rust
/// Reusable immutable plans held warm inside `DecoderEngine` across slots (Send + Sync).
pub struct Ft4Plans {
    pub candidates: Ft4CandidatePlan,
    pub downsample: Ft4DownsamplePlan,
    pub sync: Ft4SyncTables,
    pub bitmetrics: Ft4BitMetricsPlan,
    pub ap_tables: Ft4ApTables,
    pub subtractor: Ft4Subtractor,
}

/// Reusable per-worker scratch buffers (zero heap allocations inside candidate loop).
pub struct Ft4Scratch {
    pub dd: Box<[f32; NMAX]>,
    pub pass_spec: Ft4PassSpectrum,
    pub candidates_buf: Vec<Ft4Candidate>,
    pub cand_scratch: Ft4CandidateScratch,
    pub down_c1: Box<[Complex32; NDMAX]>,
    pub down_fft_scratch: Vec<Complex32>,
    pub cd2: Box<[Complex32; NDMAX]>,
    pub cb: Box<[Complex32; NDMAX]>,
    pub cd: Box<[Complex32; NFRAME_DOWN]>,
    pub bm_scratch: Ft4BitMetricsScratch,
    pub sub_scratch: Ft4SubtractScratch,
}

/// Intermediate dump hook (optional, zero overhead when `None`, used by `tools/ftxr-oracle`).
pub trait Ft4Probe {
    fn on_candidates(&mut self, pass: u8, savg: &[f32], sbase: &[f32], cands: &[Ft4Candidate]) {}
    fn on_sync(&mut self, pass: u8, icand: usize, iseg: u8, f0: f32, smax: f32, ibest: i32, idfbest: i32) {}
    fn on_bitmetrics(&mut self, pass: u8, icand: usize, iseg: u8, bm: &[[f32; 3]; NBITS], llra: &[f32; NCW], llrb: &[f32; NCW], llrc: &[f32; NCW]) {}
    fn on_post_subtract(&mut self, pass: u8, msg: &str, dd: &[f32; NMAX]) {}
}

pub fn decode_ft4<F>(
    plans: &Ft4Plans,
    scratch: &mut Ft4Scratch,
    samples_12k: &[f32],
    config: &DecodeConfig,
    hash_table: &CallsignHashTable,
    cancel: Option<&AtomicBool>,
    mut probe: Option<&mut dyn Ft4Probe>,
    mut on_decode: F,
) -> Result<Vec<DecodedMessage>, FtxrError>
where
    F: FnMut(&DecodedMessage);
```

### 3.3 Dependencies & License Audit
- **No new crates needed in Stage 4.** Stage 4 reuses the exact GPLv3-compatible crates already selected in Stages 1–3 (`rustfft` MIT/Apache-2.0, `realfft` MIT/Apache-2.0, `num-complex` MIT/Apache-2.0).
- Zero `unsafe` code required anywhere in `src/ft4/`.

### 3.4 Memory & Concurrency Design
- **`Ft4Plans` is `Send + Sync` and immutable after construction:** All `rustfft`/`realfft` plans, Nuttall window, raised-cosine filter, 67.5 KB `Ft4SyncTables`, scrambled `Ft4ApTables`, and 580 KB precomputed `Ft4Subtractor::cw` live inside `DecoderEngine` and are initialized once.
- **Per-decode memory footprint (`Ft4Scratch`):**
  - `dd`: `72,576 × 4 B ≈ 290 KB`
  - `pass_spec.cx`: `36,289 × 8 B ≈ 290 KB`
  - `sub_scratch` (`cref` 484 KB + `cfilt` 580 KB + FFT scratch ~580 KB): `~1.65 MB`
  - `cd2`/`cb`/`cd`/`down_c1`: `~125 KB`
  - Total reusable scratch per engine instance: **~2.4 MB** (zero per-candidate heap allocation!).
- **Stage 5 Concurrency Readiness:** Because `Ft4PassSpectrum` (`cx`) is strictly immutable within each pass `isp`, we structure the candidate evaluator as a pure function `evaluate_ft4_candidate(plans, &pass_spec, &ap_set, cand, config, hash_table, &mut cand_scratch) -> Option<Ft4CandidateHit>` returning the decoded message, tones, `f1`, `dt`, telemetry, etc. In Stage 4 it runs serially (`subtract` immediately into `dd`); in Stage 5 `rayon` maps `evaluate_ft4_candidate` across `candidates` in parallel and then applies `subtract` + dedupe in exact candidate index order!

---

## 4. Ordered Implementation Tasks

### Task 4.1 — FT4 Constants & Shared DSP/Modulator Prerequisites
- **Inputs:** `lib/ft4/ft4_params.f90`, `lib/ft4/gen_ft4wave.f90`, `lib/ft4/ft4_baseline.f90`, Stage 1–3 `dsp/` and `modulate/ft4.rs`.
- **Work:**
  1. Create `crates/ftxr-core/src/ft4/params.rs` with all verified constants, Costas arrays, Gray map, sync bit patterns, and `DT_SEGMENTS`.
  2. Extend `crates/ftxr-core/src/dsp/fft.rs` (`FftPlanCache`) to construct/cache FT4 FFT lengths (`2304` R2C, `72_576` R2C, `4032` C2C inverse, `72_576` C2C forward/inverse; `32` C2C forward already present). Add a unit test verifying round-trip FFT accuracy (`<1e-6`) on `NMAX = 72_576` and `NFFT2 = 4032`.
  3. Add `ft4_baseline(savg: &[f32], nfa: usize, nfb: usize, sbase: &mut [f32]) -> bool` in `crates/ftxr-core/src/dsp/baseline.rs` (sharing `pctile` + `polyfit` with FT8, outputting **linear** power `10^(db/10)` with `+0.65 dB` offset, guarding non-positive `savg` / `nlen == 0` / non-finite outputs).
  4. Verify `crates/ftxr-core/src/modulate/ft4.rs` exposes `gen_ft4_complex_wave(tones: &[u8; 103], f0_hz: f32, out: &mut [Complex32])` (len `60_480`, `icmplx = 1`, with cosine ramp-up/ramp-down) and pure `ft4_tones_from_77bits(&[u8; 77]) -> [u8; 103]`.
  5. Verify `crates/ftxr-core/src/fec/decode174_91.rs` supports `maxosd = 3` (`zsave` captures iterations `1, 2, 3` and runs up to 3 OSD passes) and `maxosd = -1` (BP-only).
- **Acceptance Criteria:** Unit tests pass for `72_576`/`4032` FFT round-trips, `ft4_baseline` on flat/sloped synthetic spectra, `gen_ft4_complex_wave` matching `Re(cref) == cos(phi)` / `Im(cref) == sin(phi)` (`|cref| == ramp_env`), and `decode174_91(..., maxosd=3)` unit test.

### Task 4.2 — FT4 Candidate Detection (`src/ft4/candidates.rs`)
- **Inputs:** `lib/ft4/getcandidates4.f90`, `lib/nuttal_window.f90`, Task 4.1.
- **Work:**
  1. Implement `Ft4CandidatePlan` using 1-based bin arrays `[0..=NH1]` (bins `1..=1152` populated from `|cx[1..=1152]|²`, DC bin `0` unused).
  2. Compute exact `NHSYM = 122` frames (`ia = j * 576`, `fac = 1.0 / 300.0`, Nuttall window len `2304`), average by `122.0`, 15-bin boxcar smooth `savsm[i] = mean(savg[i-7..=i+7])` for `i = 8..=1145`.
  3. Clamp `nfa = max((fa/df) as usize, (200.0/df).round() as usize)`, `nfb = min((fb/df) as usize, (4910.0/df).round() as usize)`.
  4. Call `ft4_baseline`; bail with 0 candidates if `!ok` or `any(sbase[nfa..=nfb] <= 0.0)`.
  5. Divide `savsm[nfa..=nfb] /= sbase[nfa..=nfb]`, find local maxima `>= 1.18`, parabolic interpolate `(del, fpeak = (i + del)*df - 31.25, speak)`, filter `200.0 <= fpeak <= 4910.0`, cap at `200`, and stably partition `|fpeak - nfqso| <= 20.0` first.
- **Acceptance Criteria:** Unit test on clean synthetic FT4 tones recovers `fpeak` within `±3 Hz` and partitions `nfqso ± 20 Hz` candidates first; silent/all-zero buffer returns `0` candidates with zero NaNs; Fortran dump diff on `000000_000002.wav` matches `savg`, `sbase`, and `candidate` within `≤1e-4`.

### Task 4.3 — FT4 Baseband Downsampler (`src/dsp/downsample.rs`)
- **Inputs:** `lib/ft4/ft4_downsample.f90`, Task 4.1.
- **Work:**
  1. Implement `Ft4DownsamplePlan::new` constructing the raised-cosine filter (`IWT=63`, `IWF=504`, `IWS=126`, `window.rotate_left(126)`, divided by `4032.0`).
  2. Implement `compute_pass_spectrum` (72,576-pt R2C FFT into `Ft4PassSpectrum.cx` len `36,289`) and `downsample_at_freq` (`i0 = (f0 / df).round() as isize`, bin extraction `0..=2016` with bounds checks `[0..=36288]`, multiply by `window`, 4,032-pt inverse C2C FFT).
- **Acceptance Criteria:** Unit test verifies single pure complex sinusoid at `f0 + k*baud` (`k ∈ 0..4`) passes with unity flat gain (`|H| = 1.0`), out-of-band tone at `f0 + 6*baud` attenuates to `0.0`, and Fortran dump diff of `cd2` on `000000_000002.wav` matches within `≤1e-4`.

### Task 4.4 — 4-Costas Coherent Sync (`src/ft4/sync4d.rs`)
- **Inputs:** `lib/ft4/sync4d.f90`, `lib/ft4_decode.f90:104–109`, `lib/ft8/twkfreq1.f90`.
- **Work:**
  1. Build the four 64-sample Costas templates (`csynca..d`) with `dphi = 2 * 2π * icos[i] / 32.0` (`16` samples/symbol) and `ctwk2` for `idf = -16..=+16` via `twkfreq1(..., fs / 2.0, [idf, 0, 0, 0, 0])`.
  2. Precompute `tweaked_conj[4][33][64]` = `(ctwk2 * csync).conj()`.
  3. Implement `sync4d(cd0, i0, idf)` with exact boundary handling for `i1 < 0` (`npts = (i1 + 127) / 2`, `npts > 16` → `m = 0..=npts`) and `i4 + 127 > 4031` (`npts = (4032 - i4) / 2`, `npts > 16` → `m = 0..npts`), returning `(|z1| + |z2| + |z3| + |z4|) / 64.0`.
- **Acceptance Criteria:** Unit tests verify peak sync at exact `(i0, idf)` on clean `ft4sim` waveform (including `i0 = -100`, `i0 = -1`, `i0 = 0`, `i0 = 333`, `i0 = 800`, `i0 = 1000` boundary cases); Fortran dump diff matches `sync4d` outputs to `≤1e-5` across all 3 segments.

### Task 4.5 — Multi-Symbol Coherent Bit Metrics (`src/ft4/bitmetrics.rs`)
- **Inputs:** `lib/ft4/get_ft4_bitmetrics.f90`, `lib/ft4_decode.f90:300–321`.
- **Work:**
  1. Implement `Ft4BitMetricsPlan::compute`: 103 forward 32-pt C2C FFTs extracting `cs[0..4][k]` and `s4[0..4][k] = |cs|`.
  2. Implement hard sync check (`nsync < 8` → `None`) using first-maximum tie-breaking matching Fortran `maxloc`.
  3. Implement `nsym = 1, 2, 4` (`nt = 4, 16, 256`) coherent sums using `GRAYMAP4 = [0, 1, 3, 2]` and MSB-first bit-polarity max-difference (`bit_pos = ibmax - ib`).
  4. Apply tail fallbacks (`bitmetrics[204..206][1] = [0]`, `bitmetrics[200..204][2] = [1]`, `bitmetrics[204..206][2] = [0]`) **before** `normalizebmet` on each of the 3 length-206 columns.
  5. Implement `count_sync_bit_quality` (`0..=32`, gate `< 20`) and `extract_llrs` (blocks `8..66`, `74..132`, `140..198` × `2.83`).
- **Acceptance Criteria:** Unit tests verify 16/16 hard sync tones and 32/32 hard sync bits on clean encoded FT4 frames, zero-bit-error hard LLR decisions across all 174 codeword bits, and Fortran dump diff of `bitmetrics` & `llra/b/c` within `≤1e-4`.

### Task 4.6 — FT4 `rvec`-Scrambled AP Engine (`src/ft4/ap.rs`)
- **Inputs:** `lib/ft4_decode.f90:74–84, 111–140, 151–183, 323–410`, Stage 1 `protocol/`.
- **Work:**
  1. Implement `Ft4ApTables::new()` with `mcq`, `mcqru`, `mcqfd`, `mcqtest`, `mcqww` XORed with `RVEC[0..29]` in `{-1, +1}`.
  2. Implement `Ft4ApSet::from_context`: pack `"{mycall} {hiscall0} RR73"`, validate `i3 == 1 && msg == msgsent && unpk77_success`, derive `apmy_ru` (`msg77[0..28] ^ RVEC[1..29]`), `aphis_fd` (`msg77[29..57] ^ RVEC[28..56]`), and `apbits` (`msg77 ^ RVEC` in `{-1, +1}`), tracking `has_mycall`/`has_hiscall` sentinels identically to `ft4_decode.f90:151–179`.
  3. Implement `ft4_ap_schedule` (`nQSOProgress 0..=5`: `[[1,2], [2,3], [2,3], [3,6], [3,6], [3,1,2]]`; override `[1]` when `lapcqonly`; empty when `ndepth == 1`, `!ap_enabled`, or `ncontest >= 6`).
  4. Implement `build_ft4_ap_pass` for `iaptype ∈ {1, 2, 3, 6}` across `ncontest = 0..=5`, enforcing `|f1 - nfqso| <= 50.0` for `iaptype >= 3`, `has_mycall` for `iaptype >= 2`, `has_hiscall` for `iaptype >= 3`, copying `llrc` as base, and scaling masked bits by `apmag = max(|llra|) * 1.1`.
- **Acceptance Criteria:** Unit tests verify bit-exact AP bit vectors against hand-verified `ft4code` scrambled payloads for standard QSOs, blank `hiscall`, nonstandard calls (proper bailout), and all 6 contest modes (`ncontest = 0..=5`).

### Task 4.7 — FT4 Signal Subtraction (`src/ft4/subtract.rs`)
- **Inputs:** `lib/ft4/subtractft4.f90`, `lib/ft4/gen_ft4wave.f90`, Task 4.1.
- **Work:**
  1. Precompute `cw` once in `Ft4Subtractor::new`: 1401-tap `cos²(π*j/1400)` for `j = -700..=700`, normalize by sum, zero-pad to `72,576`, `rotate_left(701)`, forward C2C FFT, scale by `1.0 / 72576.0`.
  2. Implement `subtract`: generate `cref[0..60480]` via `gen_ft4_complex_wave(tones, f0_hz)`, compute `nstart0 = (dt_sec * 12000.0_f32) as i32 - 576`, demodulate `camp`, FFT-convolve with `cw` via 72,576-pt forward + inverse C2C FFTs, and subtract `2.0 * (cfilt[k] * cref[k]).re` from `dd[nstart0 + k]` where in bounds.
- **Acceptance Criteria:** Subtracting a clean synthetic FT4 signal reduces residual signal energy in the passband by **>35 dB** (and cancels a +10 dB interferer cleanly enough to decode a co-channel -14 dB signal on pass 2); Fortran dump diff of post-subtraction `dd` matches within `≤1e-4`.

### Task 4.8 — FT4 Orchestrator, `DecoderEngine` Integration & Input Buffer Handling (`src/ft4/decoder.rs`, `src/ft4/mod.rs`, `src/lib.rs`)
- **Inputs:** `lib/ft4_decode.f90`, Tasks 4.1–4.7.
- **Work:**
  1. Implement `prepare_ft4_buffer(samples_12k: &[f32], dd: &mut [f32; NMAX])` (copy up to `72,576`, zero-pad if shorter, truncate if longer e.g. `90,000` samples).
  2. Implement 3-pass loop (`isp = 1..=nsp` with early exit `ndecodes == 0` before pass 2 and `nd2 == 0` before pass 3), `cand` loop over immutable `Ft4PassSpectrum`, RMS normalizations of `cd2` (`/4032`) and `cb` (`/3296`), 3-segment coarse+fine `sync4d` loop with `smax >= 1.2` & `smax >= smax1` & `10.0 < f1 < 4990.0` gates, `slice_frame_cd` (`ibest >= 0` and `ibest < 0`), `bitmetrics` + `nsync_qual >= 20`, `ipass = 1..=npasses` (`llra`, `llrb`, `llrc`, then AP passes), `decode174_91` (`ndeep = 2`, `maxosd = -1 / 2 / 3`), all-zero scrambled `message77` rejection, `rvec` descrambling, `unpack77(nrx=1)` with `CallsignHashTable`, `subtractft4` **before** dedupe, SNR/DT/`qual` calculation, `ApTag` + `uncertain = (nap != 0 && qual < 0.17)` tagging, `DecodeTelemetry` population, streaming `on_decode` callback, and cooperative `cancel` check per candidate.
  3. Wire `Mode::Ft4` into `DecoderEngine::decode` in `src/lib.rs` (initializing `Ft4Plans` lazily or at engine construction).
- **Acceptance Criteria:** End-to-end `DecoderEngine` decodes `samples/FT4/000000_000002.wav` and synthetic multi-signal slots cleanly with zero panics across all `ndepth ∈ {1, 2, 3}` and all input lengths (`0`, `1000`, `66_000`, `72_576`, `90_000`).

### Task 4.9 — Oracle Harness & Differential Parity Verification (`tools/ftxr-oracle`)
- **Inputs:** Stage 0 `ftxr-oracle`, WSJT-X `jt9`, `ft4sim`, `ft4sim_mult`, `samples/FT4/000000_000002.wav`, `lib/ft4/messages.txt`, Task 4.8.
- **Work:**
  1. Extend `jt9_runner.rs` to invoke `jt9 -5 -d {1,2,3} -c ... -x ... -Q ... -L ... -H ... -f ...` and parse `' + '` FT4 decode lines (including `?` and `a1`–`a6` metadata).
  2. Implement `ft4_dump_diff.rs` comparing `Ft4Probe` arrays against temporary Fortran dump hooks (on non-merged local scratch or standalone Fortran test driver) for all 6 intermediate pipeline checkpoints on `000000_000002.wav` and clean `ft4sim` WAVs.
  3. Implement deterministic FT4 AWGN sensitivity sweep (`0` to `-21 dB`, 1 dB steps, seeded Rust PRNG, `≥100` trials/point around `-13` to `-21 dB`), crowded multi-signal generator (reproducing `lib/ft4/messages.txt` 8 files × ~20 signals with deterministic PRNG), pure-noise FP sweep (`≥300` slots blind + AP), and loopback suite (nominal `300 ms`, `ft4sim` `dt=0`, late-start `0.5 s`/`1.0 s`/`1.5 s`, shifted `dt ∈ [-0.8, +0.9] s`, 90,000-sample buffers).
  4. Run full oracle comparison and record JSON scorecard for Stage 4 exit sign-off.
- **Acceptance Criteria:** Every numerical gate in Section 5 passes; no temporary Fortran hooks remain in tracked WSJT-X files.

---

## 5. Verification, Oracle Testing & Numerical Gates

### 5.1 Standardized `jt9 -5` Oracle CLI Flags
Always pass explicit flags so `jt9.f90`'s hardcoded defaults (`mycall='K1ABC'`, `hiscall='W9XYZ'`, `ndepth=1`) never skew benchmarks:
- **Blind / non-QSO runs:** `jt9 -5 -d {1|2|3} -L 200 -H 4000 -f 1500 -c b -x b -Q 0 <file.wav>`
  *(Note: even with `-c b -x b`, `ft4_decode.f90` still runs `a1` CQ AP at `Q=0` when `ndepth >= 2` because FT4 has no `lft8apon` toggle! Match `QsoContext { qso_progress: 0, mycall: None, hiscall: None }` and `ap_enabled: true` in `ftxr-core` for apples-to-apples parity, and additionally test `ap_enabled: false` for pure blind FP validation.)*
- **AP QSO-context runs (`a1`–`a6`):** `jt9 -5 -d 3 -L 200 -H 4000 -f <nfqso> -c <MYCALL> -x <HISCALL> -Q <0..5> -X <ncontest> <file.wav>`

### 5.2 Stage-by-Stage Intermediate Numerical Gates (vs. WSJT-X Fortran)
Verified on `samples/FT4/000000_000002.wav` and deterministic single-signal `ft4sim` fixtures (`-10 dB`, `0 dB`, `dt = 0.0`, `dt = +0.7`, `dt = -0.7`):

| Pipeline Stage | Compared Arrays / Scalars | Pass Tolerance |
| :--- | :--- | :--- |
| 1. Candidate search (`getcandidates4` + `ft4_baseline`) | `savg[1..=1152]`, `sbase[nfa..=nfb]`, candidate list `(fpeak, speak)` & order | Relative error `≤ 1e-4`; exact candidate count & partition order (barring boundary ties at `1.1800 ± 1e-5`) |
| 2. Baseband downsample (`ft4_downsample`) | `cd2[0..4032]` and `cb[0..4032]` (complex) | Relative L2 error `≤ 1e-4` |
| 3. 4-Costas fine sync (`sync4d` 3-seg loop) | `(smax, ibest, idfbest)` per `(icand, iseg)` | `ibest` exact (`0` diff), `idfbest` exact (`0` diff), `smax` rel err `≤ 1e-4` |
| 4. Frame slice & bit metrics (`get_ft4_bitmetrics`) | `cd[0..3296]`, `nsync`, `nsync_qual`, `bitmetrics[0..206][0..3]`, `llra/b/c[0..174]` | `nsync` & `nsync_qual` exact; `llra/b/c` rel L2 error `≤ 1e-4` |
| 5. AP & FEC (`ft4::ap` + `decode174_91`) | `apbits`, `apmask`, `llrd`, decoded `cw`, `nharderror`, `dmin`, `iaptype`, `qual` | **100% bit-exact** `apbits`/`apmask`/`cw`, exact `nharderror`/`iaptype`, `dmin`/`qual` rel err `≤ 1e-4` |
| 6. Signal subtraction (`subtractft4`) | Post-subtraction `dd[0..72576]` residual | Relative L2 error `≤ 1e-4` |

### 5.3 End-to-End Differential & Statistical Gates

| Test Suite | Corpus / Parameters | Pass Gate |
| :--- | :--- | :--- |
| **Real-World Corpus (`-d 1/2/3`)** | `samples/FT4/000000_000002.wav` + curated real FT4 WAVs | **≥95%** of `jt9 -5` decode count at each depth (`-d 1`, `-d 2`, `-d 3`); target ≥98% at `-d 3` |
| **Crowded Multi-Signal Corpus** | 8 deterministic `ft4sim_mult`-equivalent files (`lib/ft4/messages.txt`, ~150+ overlapping signals, 20+ dB dynamic range) + high-density 25-signal synthetic stress slots | **≥95%** of `jt9 -5 -d 3` decode count; verifies 3-pass subtraction recovery of masked weak signals |
| **Synthetic AWGN Cliff Sweep** | `0 dB` to `-21 dB` in 1 dB steps (seeded PRNG; 100 trials/step in `-13..-21 dB`) | **≥99%** decode rate `0` to `-14 dB`; 50% decode threshold within **≤0.5–1.0 dB** of `jt9 -5 -d 3` (classic FT4 50% threshold ≈ `-17.5 dB` blind / `-19 to -20 dB` AP) |
| **AP Hypothesis Matrix (`a1`–`a6`)** | Weak signals (`-18` to `-21 dB`) across `Q = 0..5`, `ncontest = 0..5`, inside/outside `nfqso ± 50 Hz`, standard & nonstandard calls | Matches `jt9 -5 -d 3` AP recovery rate within ≥95%; `a3`/`a6` never trigger outside `±50 Hz`; `maxosd = 3` activates inside `±50 Hz`; `qual < 0.17` sets `uncertain = true` (`? a*`) |
| **Measurement Agreement** | All matched decodes across real + synthetic suites | `|Δaf| ≤ 2 Hz`, `|Δdt| ≤ 0.1 s`, `|Δsnr| ≤ 2 dB` |
| **Pure-Noise False Positives** | ≥300 pure-AWGN slots at `-d 3` (blind `Q=0`, AP-disabled, and AP-enabled `Q=3` with callsigns) | FP rate **≤ `jt9 -5 -d 3`** (target `<0.1%` without AP; any AP FPs properly carry `ApTag` / `uncertain` metadata) |
| **FT4 TX→RX Loopback** | Rust `modulate::ft4` → Rust `decode_ft4` AND `jt9 -5`: nominal `300 ms` delay, `ft4sim` `dt=0`, UTC-aligned late-start (`0.5 s`, `1.0 s`, `1.5 s`), clock shifts (`dt = -0.8, -0.4, +0.4, +0.9 s`), truncated (`66,000` samples) & full 7.5 s (`90,000` samples) | **100%** decode rate in both `ftxr-core` and `jt9 -5` across all cases; zero panics on edge-length (`0`, `100`, `72_575`, `72_576`, `90_000`) and silent buffers |

---

## 6. Risks, Gotchas & Cross-Stage Handoff

### 6.1 Key Risks & Mitigations
1. **Non-power-of-two `72,576`-point FFT latency in `subtractft4`:**
   - *Risk:* Two 72,576-point C2C FFTs per decoded signal can dominate wall time on a 25-signal crowded FT4 slot if FFT plans are recreated or fall back to Bluestein.
   - *Mitigation:* `72,576 = 2^7 × 3^4 × 7` is 7-smooth (native `rustfft` mixed-radix, zero Bluestein). Precompute and cache `c2c_fwd_nmax`, `c2c_inv_nmax`, and `cw` once inside `Ft4Plans`, and reuse `Ft4SubtractScratch` across signals. Benchmark 25-signal subtraction latency in Stage 4 so Stage 5 starts with a fast baseline.
2. **Silent off-by-one sensitivity regressions (1-based Fortran → 0-based Rust):**
   - *Risk:* Subtle indexing bugs in `getcandidates4` bin `1..=1152`, `ft4_downsample` `rotate_left(126)`, `sync4d` `i1 < 0` / `i4` boundary slicing, `get_ft4_bitmetrics` tail fallbacks, or `subtractft4` `rotate_left(701)` / `-NSPS` can silently degrade sensitivity by 1–3 dB while high-SNR tests still pass.
   - *Mitigation:* Mandatory Task 4.9 stage-by-stage intermediate numerical diff (`≤1e-4`) against WSJT-X Fortran before trusting end-to-end statistics.
3. **`rvec` domain confusion in AP / `decode174_91` / `get_ft4_tones_from_77bits`:**
   - *Risk:* Forgetting to scramble `apmy_ru` (`rvec[1..29]`) or `aphis_fd` (`rvec[28..56]`) with their 1-bit shifted `rvec` slices, checking `sum(message77) == 0` after descrambling instead of before, or passing scrambled bits to `ft4_tones_from_77bits` (double-scrambling!).
   - *Mitigation:* Dedicated unit tests in `ft4::ap` and `ft4::decoder` exercising every contest mode (`0..=5`), all-zero LDPC codeword rejection, and verifying subtraction tones match `ft4code` output bit-for-bit.
4. **Float32 `log10(0.0)` / NaN propagation in `ft4_baseline` on silent or band-limited audio:**
   - *Risk:* Real-world audio with a steep hardware low-pass filter above 3 kHz or digital silence produces `savg[i] == 0.0`, yielding `-inf` → `NaN` in `polyfit`.
   - *Mitigation:* Guard `savg[i] <= 0.0` (either floor or bail identically to Fortran's `sbase <= 0` guard) and verify `!sbase[i].is_finite() || sbase[i] <= 0.0` returns `0` candidates cleanly.

### 6.2 Cross-Stage Handoff Contracts

#### Consumed from Stages 1–3
- `protocol::{pack77, unpack77, CallsignHashTable, rvec::RVEC, ldpc::encode174_91}`
- `modulate::ft4::{ft4_tones_from_77bits, gen_ft4_complex_wave}`
- `dsp::{fft::FftPlanCache, window::nuttall_window, baseline::{pctile, polyfit}, twkfreq::twkfreq1}`
- `fec::{decode174_91::decode174_91, normalizebmet}` (specifically verifying `maxosd = 3` and `maxosd = -1` paths!)

#### Exposed to Stage 5 (`ftxr-modem` CLI + Warm IPC Worker, Parallelism & Telemetry)
- **`DecoderEngine::decode` (`Mode::Ft4`):**
  - Accepts arbitrary-length 12 kHz `&[f32]` PCM (handles `<72,576` partial/early passes via zero-padding and `>72,576` full 7.5 s `90,000`-sample slots via deterministic truncation).
  - Keeps `Ft4Plans` warm in RAM across slots with zero cold-start stall.
  - Exposes streaming `on_decode: impl FnMut(&DecodedMessage)` callback and cooperative `AtomicBool` cancellation check between candidates (enabling `<50 ms` cancel in Stage 5).
  - Separates per-pass immutable `Ft4PassSpectrum` (`cx`) from `evaluate_ft4_candidate(...)` and deterministic in-order `subtractft4` + dedupe, allowing Stage 5 to drop in `rayon::par_iter()` over candidates within each pass with **zero** behavioral divergence from Stage 4.
  - Emits `DecodedMessage` with reconciler-compatible `ApTag` + `uncertain` (`None`, `a1`–`a6`, `? a*`), `mode: Mode::Ft4` (formatted with `' + '` in CLI text output), and populated `DecodeTelemetry` (`fec_method`, `sync_score`, `subtraction_pass`, `nharderror`, `dmin`, `qual`, `dt_segment`, `nsync_hard`, `nsync_bits`).

#### Exposed to Stage 6 & Stage 7 (Independent Repo & `digi-dx` Blueprint)
- Zero `wsjtx`/Fortran/FFTW/Qt dependencies inside `crates/ftxr-core`; compact FT4 regression fixtures (`000000_000002.wav` + self-contained synthetic loopback vectors) travel cleanly into `mpmbq2/ftxr-modem` in Stage 6.
- Explicitly documents for Stage 7 (`digi-dx` FT4 live-loop prerequisites) that FT4's effective RX capture window is `6.048 s` (`72,576` samples, `21 × 3456`) within a `7.5 s` slot (with `90,000`-sample buffers safely accepted and truncated), TX delay is `300 ms`, active TX is `5.04 s`, and `jt9` FT4 output lines use `' + '` instead of `' ~ '`.
