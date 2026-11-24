# Stage 3 Plan — FT8 Weak-Signal Parity (OSD, Multi-Symbol Coherent LLRs, AP `a1`–`a6` & Deep Subtraction)

## 1. Header & Scope

- **Stage:** 3 of 7 (developed & verified inside `/home/antigravity/mpmbq2/wsjtx/rust-ftx/`)
- **Primary Objective:** Bring pure-Rust `ftxr-core` FT8 decoding from Stage 2's BP-only MVP (`jt9 -8 -d 1` baseline) to full weak-signal parity with classic single-threaded WSJT-X `jt9 -8 -d 3` (`lib/ft8/`, excluding `-M`/`lib/ft8var/` and deferring cross-slot `a7` and RX-freq list decoder `a8`).
- **Prerequisites (from Stages 0–2):**
  - **Stage 0:** Built WSJT-X oracle binaries (`jt9`, `ft8code`, `ft8sim`), deterministic AWGN/crowded-band corpus generator in `tools/ftxr-oracle`, golden real-world fixtures (`samples/FT8/210703_133430.wav`, `191111_110200.wav`, etc.), and recorded baseline `jt9 -8 -d 1/2/3` scorecards.
  - **Stage 1 (`crates/ftxr-core/src/protocol/`, `src/modulate/`):** 100% bit-exact `pack77` / `unpack77`, `is_standard_callsign` (`stdcall`), thread-safe `CallsignHashTable` (`save_hash_call` returning `(n10, n12, n22)`), `crc14` / `crc14_check` (`0x2757`), `encode174_91` / `encode174_91_nocrc`, `LDPC_174_91_GENERATOR` (83×91 parity matrix), `ft8_tones_from_77bits`, and `gen_ft8_wave`.
  - **Stage 2 (`crates/ftxr-core/src/dsp/`, `src/fec/`, `src/ft8/`):** Numerically verified `sync8`, `get_spectrum_baseline`, `ft8_downsample`, `sync8d`, `twkfreq1`, 1-symbol `llra` path in `ft8b`, BP decoder `decode174_91` (with `zsave: [[f32; 174]; 3]` hooks preserved), and base `subtractft8` (`lrefinedt = false`).
- **Deliverables:**
  1. Bit-packed GF(2) Ordered Statistics Decoder (`crates/ftxr-core/src/fec/osd.rs`) directly porting `lib/ft8/osd174_91.f90` (`Keff = 91` fast path + general `77..=91`, `ndeep = 0..=6`, zero global mutable state).
  2. Hybrid BP + OSD integration in `crates/ftxr-core/src/fec/decode174_91.rs` (`maxosd = -1..=3`, `norder = 2`, exact channel-LLR `dmin` recomputation and `zsave`-based `nharderror` preservation).
  3. Multi-symbol coherent bit metrics (`bmeta`, `bmetb`, `bmetc`, `bmetd`, `bmete` for `nsym = 1, 2, 3`; `imetric = 1` vs `2`), `normalizebmet`, hard Costas sync quality gates, AP pass loop (`ipass = 1..=npasses`, `llra`/`llrc` alternation), and all post-decode false-positive gates in `crates/ftxr-core/src/ft8/ft8b.rs`.
  4. Single-slot A priori (`AP`) engine (`crates/ftxr-core/src/ft8/ap.rs`) porting `lib/ft8/ft8apset.f90` and `lib/ft8/ft8b.f90` (`a1`–`a6` across `nQSOProgress = 0..=5` and `ncontest = 0..=8`), plus `ApTag` formatting (`None`, `a1`–`a6`, `? a1`–`? a6`) matching `digi-dx`'s `SlotDecodeReconciler`.
  5. DT-refined signal subtraction (`lrefinedt = true`) in `crates/ftxr-core/src/ft8/subtract.rs` (`±90` sample residual band-power search + `peakup` interpolation) and full 3-pass / WSJT-X 3-stage (`nzhsym = 41/47/50`) scheduling in `crates/ftxr-core/src/ft8/decoder.rs`.
  6. Oracle verification suite in `tools/ftxr-oracle`: standalone Fortran-vs-Rust `osd174_91` differential driver, deterministic Monte Carlo AWGN cliff sweep (`-15` to `-24 dB`), crowded-band overlapping-signal test, AP `a1`–`a6` test matrix, real-world corpus scorecard, and pure-noise false-positive sweep.
- **Exit Gate Checklist:**
  - [ ] **100% bit-exact codeword & distance parity** between Rust `osd174_91` and Fortran `osd174_91` on identical LLR vectors (including `Keff = 91`, `ndeep = 0..=3`, AP and non-AP masks).
  - [ ] Intermediate multi-symbol LLR arrays (`llra`–`llre`) and post-`lrefinedt` residual `dd` match WSJT-X Fortran within `≤ 1e-4` relative error on golden fixtures.
  - [ ] **≥ 95–98% of classic `jt9 -8 -d 3` total decode count** across the real-world FT8 `.wav` corpus (excluding deferred `a7`/`a8` decodes).
  - [ ] Deterministic AWGN 50% decode threshold within **≤ 0.5–1.0 dB** of `jt9 -8 -d 3` (both blind and AP `a1`–`a6` scenarios).
  - [ ] Crowded-band overlapping-signal decode count **≥ 95%** of `jt9 -8 -d 3`.
  - [ ] Pure-noise false-decode rate **≤ `jt9 -8 -d 3`** (target `< 0.1%` without AP; uncertain AP decodes tagged `? a1`–`? a6` identically to `decoder.f90` `qual < 0.17`).
  - [ ] Measurement agreement vs. `jt9 -8 -d 3`: AF within **±2 Hz**, DT within **±0.1 s**, SNR within **±2 dB**.
  - [ ] Zero unsafe global state, zero non-GPLv3-compatible crates, zero Numerical Recipes code (`bessi0` and NR `indexx` excluded), zero modifications to WSJT-X or `digi-dx`.

---

## 2. Source Audit & Technical Reference

All constants, indices, and edge cases below were verified directly against the WSJT-X Fortran and `digi-dx` Rust sources on disk.

### 2.1 `lib/ft8/osd174_91.f90` (410 lines)

| Element | Fortran Source Reference | Exact Semantics & Verified Bounds |
| :--- | :--- | :--- |
| Signature & sizes | L1, L16, L25 | `osd174_91(llr, k, apmask, ndeep, message91, cw, nhardmin, dmin)`; `N = 174`, `k ∈ [77, 91]` (`Keff = 91` in FT8 and FT4). |
| Generator matrix `gen(k, N)` | L37–L65 | For `i = 1..k`: `message91 = 0; message91(i) = 1`. If `i ≤ 77`: compute `get_crc14(m96, 96, ncrc14)` where `m96(1:91) = message91` (`m96(78:96) = 0`), store 14-bit CRC in `message91(78:91)`, then zero `message91(78:k)`. Call `encode174_91_nocrc(message91, cw)`, set `gen(i, :) = cw`. **Note:** When `k = 91`, `message91(78:91)` is completely zeroed by L58, so `gen` is simply `[I_91 \| P_{91×83}]` where `P` is the transpose of `ldpc_174_91_c_generator.f90`'s `83×91` matrix. |
| Hard decisions | L71–L72 | `hdec = 0; where (rx .ge. 0) hdec = 1` (**inclusive `>= 0.0`** — differs from BP's strict `> 0.0`). |
| Reliability sort | L75–L82 | `absrx = abs(rx)`; `indexx(absrx, 174, indx)` (ascending); `indices(i) = indx(175 - i)` (descending reliability). Reorders columns of `gen` into `genmrb(1:k, 1:174)`. |
| GF(2) Gauss-Jordan elimination | L86–L107 | For `id = 1..k`: scans `icol = id..k+20` (for `k = 91`, `icol <= 111`) for first `genmrb(id, icol) == 1`. Swaps column `id` and `icol` in `genmrb(1:k, :)` and `indices`; XORs row `id` into every row `ii ≠ id` where `genmrb(ii, id) == 1`, then exits inner loop. **Gotcha:** No row pivoting; `k+20` is ad-hoc (`! The 20 is ad hoc - beware`). If no `1` exists in `id..k+20`, Fortran silently skips elimination for `id`, leaving `genmrb(:, 1:k) ≠ I_k`. Searching `id..N` finds the exact same pivot whenever one exists in `id..k+20` and eliminates the latent silent failure (since full row rank `k` guarantees a pivot in `id..N`). |
| Transpose `g2` | L109, L295–L305 | `g2(N, k) = transpose(genmrb(k, N))` solely so column `g2(1:N, i)` is contiguous in column-major Fortran. In row-major Rust, `genmrb[i]` *already* equals `g2(:, i)` — zero transpose needed. |
| Order-0 codeword | L117–L128 | Permute `hdec`, `absrx`, `rx`, `apmaskr` by `indices`. `m0 = hdec(1:k)`, `c0 = mrbencode91(m0, g2)`, `nxor = c0 ^ hdec`, `nhardmin = sum(nxor)`, `dmin = sum(nxor * absrx)`. |
| `ndeep` parameter table | L134–L177 | `0`: order-0 only; `1`: `nord=1, npre1=0, npre2=0, nt=40, ntheta=12`; **`2` (FT8 & FT4)**: **`nord=1, npre1=1, npre2=0, nt=40, ntheta=10`**; `3`: `nord=1, npre1=1, npre2=1, nt=40, ntheta=12, ntau=14`; `4`: `nord=2, npre1=1, npre2=1, nt=40, ntheta=12, ntau=17`; `5`: `nord=3, npre1=1, npre2=1, nt=40, ntheta=12, ntau=15`; `≥6`: `nord=4, npre1=1, npre2=1, nt=95, ntheta=12, ntau=15` (clamp `nt` to `N - k` in Rust to avoid OOB when `k = 91` and `N - k = 83 < 95`). |
| Order `1..nord` + Pre-1 loop | L179–L228 | Walks weight-`iorder` patterns `misub` via `nextpat91` (from highest bits `k-iorder+1..k` downwards; `iflag` = lowest set bit in `misub`). Inner loop `n1 = iflag` down to `iend` (`iflag` if `iorder == nord && npre1 == 0`, else `1`): sets `mi = misub; mi(n1) = 1`; skips if `any(apmaskr(1:k) & mi == 1)`. For `n1 == iflag`: computes `e2sub = ce(k+1:N) ^ hdec(k+1:N)`, `nd1kpt = sum(e2sub(1:nt)) + 1`, `d1 = sum((me(1:k) ^ hdec(1:k)) * absrx(1:k))`. For `n1 < iflag`: `e2 = e2sub ^ g2(k+1:N, n1)`, `nd1kpt = sum(e2(1:nt)) + 2`. If `nd1kpt <= ntheta`: computes `dd` (`d1 + sum(e2sub * absrx(k+1:N))` or `d1 + (ce(n1)^hdec(n1))*absrx(n1) + sum(e2 * absrx(k+1:N))`), updates `(dmin, cw, nhardmin)` if `dd < dmin`. |
| Pre-2 bucket loop (`npre2 = 1`) | L230–L279, L336–L408 | Indexes all `k(k-1)/2 = 4095` pairs `(i1, i2)` by their `ntau`-bit LRB prefix XOR (`g2(k+1:k+ntau, i1) ^ g2(k+1:k+ntau, i2)`), then probes `e2sub ^ ui` (0 or 1 error in first `ntau` LRB bits). **Thread-safety bug in Fortran:** `boxit91`/`fetchit91` use global `COMMON /boxes/` (`525,001`-entry head array) and `SAVE lastpat, inext`; Rust replaces this with per-call/per-thread scratch storage. Only active for `ndeep >= 3` (`npre2 = 0` for FT8/FT4). |
| Un-permute & CRC-14 check | L283–L290 | `cw(indices) = cw` (scatter `cw_orig[indices[pos]] = cw_perm[pos]`). Sets `m96 = 0; m96(1:77) = cw(1:77); m96(83:96) = cw(78:91)` (5 zero pad bits at `78:82`), calls `get_crc14(m96, 96, nbadcrc)`. If `nbadcrc != 0`, **`nhardmin = -nhardmin`**. |

### 2.2 `lib/ft8/decode174_91.f90` (156 lines)

- **Parameters:** `N = 174`, `K = 91`, `M = 83`, `maxiterations = 30`.
- **`maxosd` semantics (L28–L37, L61–L64, L137–L148):**
  - Clamped to `<= 3`.
  - `maxosd < 0`: BP only (`nosd = 0`).
  - `maxosd == 0`: BP followed by 1 OSD call using raw channel `llr` (`zsave(:, 1) = llr`).
  - `maxosd ∈ {1, 2, 3}`: During BP (`iter = 0..=30`), `zsum += zn` every iteration (so `iter = 0` sets `zsum = llr`). For `iter = 1..=maxosd`, saves `zsave(:, iter) = zsum` (i.e. `zsave(:, 1) = zn_0 + zn_1`, `zsave(:, 2) = zn_0 + zn_1 + zn_2`, `zsave(:, 3) = zn_0 + zn_1 + zn_2 + zn_3`). Early stopping requires `iter >= 10` (`ncnt >= 5 && iter >= 10 && ncheck > 15`), so `zsave(:, 1..=maxosd]` is always populated before early exit.
- **BP AP clamping (L54–L60, L108–L118):**
  - If `apmask(i) == 1`: `zn(i) = llr(i)` (clamped, ignoring incoming `tov`).
  - Check-node message `toc(i, j) = zn(ibj) - tov(kk, ibj)` still subtracts `tov` even for AP bits (must be preserved for numerical parity with Fortran).
- **BP success (`ntype = 1`, L67–L89):**
  - Hard decision `cw = (zn > 0.0)` (strict `>`).
  - If syndrome `ncheck == 0` and `get_crc14 == 0`: `nharderror = count((2*cw - 1) * llr < 0.0)` (strict `< 0.0`), `dmin = sum((hdec_llr ^ cw) * abs(llr))` where `hdec_llr = (llr >= 0.0)`, `ntype = 1`, return.
- **OSD invocation & critical metric asymmetry (`ntype = 2`, L137–L148):**
  - Loops `i = 1..=nosd`, calling `osd174_91(zsave(:, i), Keff, apmask, norder, message91, cw, nharderror, dminosd)`.
  - **CRITICAL GOTCHA #1:** Acceptance test is **`if (nharderror .gt. 0)`** (strictly `> 0`, NOT `>= 0`!). This simultaneously guards against `nbadcrc != 0 && nhardmin == 0` (where `-0 == 0`) and rejects 0-hard-error OSD candidates on `zsave`.
  - **CRITICAL GOTCHA #2:** Upon OSD success, `decode174_91` **recomputes `dmin`** against the raw channel `llr` (`sum(ieor(hdec_llr, cw) * abs(llr))`), discarding `dminosd`, **but keeps `nharderror` from `osd174_91` (computed against `zsave(:, i)`) without recomputing it against channel `llr`**! This exact `nharderror` feeds both `ft8b.f90` L444 (`nharderrors > 36`) and `ft8_decode.f90` L234 (`qual = 1.0 - (nharderrors + dmin)/60.0`).

### 2.3 `lib/ft8/ft8b.f90` (531 lines)

- **Hard Costas sync quality check (L164–L184):**
  - After 32-pt symbol FFTs (`cs(0:7, 1:79) = csymb(1:8) / 1e3`, `s8(0:7, 1:79) = abs(csymb(1:8))`), counts exact `argmax(s8(:, k)) == icos7` matches across the 3×7 = 21 Costas symbols (`1..7`, `37..43`, `73..79`).
  - Bailout (`nbadcrc = 1`, return) if `nsync <= syncmin`, where:
    - `ndepth <= 2`: `syncmin = 8` (requires `nsync >= 9`)
    - `ndepth == 3, imetric == 1`: `syncmin = 6` (requires `nsync >= 7`)
    - `ndepth == 3, imetric == 2`: `syncmin = 7` (requires `nsync >= 8`)
- **Multi-symbol coherent bit metrics (L186–L254, L493–L506):**
  - `graymap = [0, 1, 3, 2, 5, 6, 4, 7]` (3-bit binary → tone index).
  - `one(0:511, 0:8)`: `one[i][j] = ((i >> j) & 1) != 0`.
  - For `nsym ∈ {1, 2, 3}`, `nt = 1 << (3 * nsym)` (`8, 64, 512`), `ibmax = 3 * nsym - 1` (`2, 5, 8`):
    - For `ihalf ∈ {1, 2}` and `k = 1, 1 + nsym, ... <= 29`:
      - `ks = if ihalf == 1 { k + 7 } else { k + 43 }` (1-indexed into 79-symbol `cs`).
      - **CRITICAL GOTCHA #3 (Boundary overlap into Costas symbols 37 and 73):** Because `29` is not divisible by `2` or `3`:
        - `nsym = 2, k = 29`: sums `cs(:, 36) + cs(:, 37)` (`ihalf = 1`) and `cs(:, 72) + cs(:, 73)` (`ihalf = 2`). Bits `88..90` written on `ihalf = 1` are overwritten by `ihalf = 2, k = 1`; bits `175..177` are skipped by `i32 + ib > 174`.
        - `nsym = 3, k = 28`: sums `cs(:, 35) + cs(:, 36) + cs(:, 37)` (`ihalf = 1`) and `cs(:, 71) + cs(:, 72) + cs(:, 73)` (`ihalf = 2`).
        - Because Costas symbols `37` and `73` are real phase-coherent tones in `cs`, Rust **must** include them identically rather than truncating `nsym` at the half boundary.
      - For `i ∈ 0..nt`: `i1 = i >> 6`, `i2 = (i >> 3) & 7`, `i3 = i & 7`:
        - `nsym = 1`: `z = cs[graymap[i3]][ks]`
        - `nsym = 2`: `z = cs[graymap[i2]][ks] + cs[graymap[i3]][ks + 1]`
        - `nsym = 3`: `z = cs[graymap[i1]][ks] + cs[graymap[i2]][ks + 1] + cs[graymap[i3]][ks + 2]`
        - `s2[i] = if imetric == 2 { z.norm_sqr() } else { z.norm() }` (note L207 `if (imetric.eq.2) s2 = s2**2`).
      - For `ib ∈ 0..=ibmax`: bit mask checks bit `ibmax - ib` of `i` (MSB first):
        - `m1 = max_{bit==1} s2[i]`, `m0 = max_{bit==0} s2[i]`, `bm = m1 - m0`.
        - `nsym = 1`: `bmeta[idx] = bm`; `den = m1.max(m0)`; `bmetd[idx] = if den > 0.0 { bm / den } else { 0.0 }`.
        - `nsym = 2`: `bmetb[idx] = bm`.
        - `nsym = 3`: `bmetc[idx] = bm`.
  - `bmete[i]` (L235–L241): computed **before** normalization as the signed element of `[bmeta[i], bmetb[i], bmetc[i]]` having the maximum `abs(...)` (first wins on tie).
  - `normalizebmet(bmet, 174)` (L493–L506): divides by `bmetsig = sqrt(var)` if `var = E[x^2] - E[x]^2 > 0`, else `sqrt(E[x^2])` (guard `bmetsig > 0.0` in Rust to avoid `0/0 = NaN`).
  - `scalefac = 2.83` → `llra..llre = 2.83 * (bmeta..bmete)`.
- **Pass schedule & AP LLR alternation (L256–L302):**
  - `npasses = 5` if `!lapon && ncontest != 7`, or if `nzhsym < 50`.
  - Else if `lapcqonly`: `npasses = 7` (AP pass 1 only, `iaptype = 1`).
  - Else: `npasses = 5 + 2 * nappasses[nQSOProgress]`.
  - Passes `1..=5` (`apmask = 0, iaptype = 0`): `llra`, `llrb`, `llrc`, `llrd`, `llre`.
  - Passes `6..=npasses` (`iaptype = naptypes[nQSOProgress][(ipass - 4)/2]` or `1` if `lapcqonly`):
    - **CRITICAL GOTCHA #4 (Comment vs. Code mismatch at L263–L270 vs L294–L296):** The comment claims AP passes alternate `nsym=1` and `nsym=2`, **but the actual code alternates `llra` (`nsym=1`) on odd `(ipass - 5)` (`6, 8, 10, 12`) and `llrc` (`nsym=3`) on even `(ipass - 5)` (`7, 9, 11, 13`)!**
    - `apmag = maxval(abs(llrz)) * 1.1` (recomputed from that pass's `llra` or `llrc`).
- **QSO progress & AP hypothesis table (L61–L82, L304–L422):**
  - `nQSOProgress`:
    - `0` (`Calling` / Tx6): `nappasses = 2`, `[a1, a2]`
    - `1` (`Replying` / Tx1): `nappasses = 2`, `[a2, a3]`
    - `2` (`Report` / Tx2): `nappasses = 2`, `[a2, a3]`
    - `3` (`RogerReport` / Tx3): `nappasses = 4`, `[a3, a4, a5, a6]`
    - `4` (`Rogers` / Tx4): `nappasses = 4`, `[a3, a4, a5, a6]`
    - `5` (`Signoff` / Tx5): `nappasses = 3`, `[a3, a1, a2]`
  - `ncontest`: `0=None, 1=NaVhf, 2=EuVhf, 3=FieldDay, 4=Rtty, 5=WwDigi, 6=Fox, 7=Hound, 8=ArrlDigi` (note `decoder.f90` L137 masks `iand(params%nexp_decode, 7)` for `0..=7`, while `ft8b.f90` also explicitly branches on `8`).
  - **Bailout conditions (L315–L320):**
    1. `ncontest <= 5 && iaptype >= 3 && |f1 - nfqso| > napwid && |f1 - nftx| > napwid` → skip
    2. `ncontest == 6` (Fox) → skip all AP
    3. `ncontest == 7 && f1 > 950.0` (Hound) → skip
    4. `iaptype >= 2 && apsym[0] > 1` (missing/nonstandard `mycall`) → skip
    5. `ncontest == 7 && iaptype >= 2 && aph10[0] > 1` (missing `hiscall` hash10) → skip
    6. `iaptype >= 3 && apsym[29] > 1` (missing/nonstandard `hiscall`) → skip
    7. `iaptype == 5 && ncontest == 7` (Hound skips `73`) → skip
  - **Static bipolar (`±1`) AP bit patterns (L40–L60):**
    - Note Fortran bug at L52–L60: `if (first .or. ncontest /= ncontest0) mcq = 2*mcq - 1` corrupts `SAVE`d arrays if `ncontest` ever changes. Rust stores them as `const [i8; 29]` / `const [i8; 19]` already in `{-1, +1}`.
  - **Exact bit masks per `(iaptype, ncontest)` (0-indexed slices matching Fortran 1-indexed):**
    - `a1` (`CQ`): `[0..29]` = `mcq(ncontest)` (`0/7: MCQ`, `1/2/8: MCQ_TEST`, `3: MCQ_FD`, `4: MCQ_RU`, `5: MCQ_WW`); `[74..77]` = `(-1, -1, +1)` (`i3=1`).
    - `a2` (`MyCall ??? ???`):
      - `0, 1, 5, 8`: `[0..29] = apsym[0..29]`, `[74..77] = (-1, -1, +1)`.
      - `2` (EU VHF): `[0..28] = apsym[0..28]`, `[71..74] = (-1, +1, -1)`, `[74..77] = (-1, -1, -1)`.
      - `3` (Field Day): `[0..28] = apsym[0..28]`, `[74..77] = (-1, -1, -1)`.
      - `4` (RTTY RU): `[1..29] = apsym[0..28]`, `[74..77] = (-1, +1, +1)`.
      - `7` (Hound): `[28..56] = apsym[0..28]`, `[56..66] = aph10[0..10]`, `[71..77] = (-1, -1, +1, -1, -1, -1)`.
    - `a3` (`MyCall DxCall ???`):
      - `0, 1, 2, 5, 7, 8`: `[0..58] = apsym[0..58]`, `[74..77] = (-1, -1, +1)`.
      - `3` (Field Day): `[0..28] = apsym[0..28]`, `[28..56] = apsym[29..57]`, `apmask[71..74] = 1` (**note L390: masks `72:74` without overwriting `llrz(72:74)`**), `[74..77] = (-1, -1, -1)`.
      - `4` (RTTY RU): `[1..29] = apsym[0..28]`, `[29..57] = apsym[29..57]`, `[74..77] = (-1, +1, +1)`.
    - `a4` / `a5` / `a6` (`MyCall DxCall RRR|73|RR73`):
      - `0..=5, 8` or `(7 && a6)`: `[0..58] = apsym[0..58]`, `[58..77] = MRRR | M73 | MRR73` (`apmask[0..77] = 1`, all 77 message bits locked).
      - `7 && a4` (Hound `MyCall RR73; ...`): `[0..28] = apsym[0..28]`, `[56..66] = aph10[0..10]`, `[71..77] = (-1, -1, +1, -1, -1, -1)`.
- **OSD call config (L427–L438):** `Keff = 91`, `norder = 2`, `maxosd = if ndepth == 1 { -1 } else { 2 }`. (L431–L434 is a no-op since `maxosd` is already `2`.)
- **Post-decode false-positive gates (L444–L488):**
  1. `nharderrors < 0 || nharderrors > 36` → `cycle`
  2. `cw` is all-zero (`count(cw == 0) == 174`) → `cycle`
  3. `i3 > 5 || (i3 == 0 && n3 > 6) || (i3 == 0 && n3 == 2)` → `cycle`
  4. `unpack77` fails → `cycle`
  5. `ncontest == 0 && i3 ∈ 1..=3 && (msg.contains("/R") || msg.starts_with("TU; "))` → `cycle`
  6. After SNR calculation (`xbase = 10^(0.1*(sbase(nint(f1/3.125)) - 40))`, `xsnr2 = 10*log10(max(0.001, xsig/xbase/3e6 - 1.0)) - 27.0`): if `nsync <= 10 && xsnr < -25.0`, reject (`nbadcrc = 1`, return). Else clamp `xsnr = xsnr.max(-25.0)`.
- **Licensing exclusion:** `bessi0` (L509–L530, Numerical Recipes) is unused in `ft8b.f90` and is **strictly excluded** from `ftxr-core`. Similarly, NR `indexx` (`lib/indexx.f90`) is replaced by Rust's stdlib sort.

### 2.4 `lib/ft8/ft8apset.f90` (63 lines)

- Initializes `apsym = [0; 58]` with sentinels `apsym[0] = 99`, `apsym[29] = 99`, and `aph10 = [0; 10]` with `aph10[0] = 99`.
- Returns immediately if `mycall.trim().len() < 3`.
- If `hiscall.trim().len() < 3`: substitutes dummy `"KA1ABC"` (`nohiscall = true`); else calls `save_hash_call(hiscall)` → `n10 = hash10 & 0x3FF` → bipolar `aph10[0..10] = 2 * bit - 1`.
- Builds `msg = "{mycall} {hiscall} RRR"` (or `"<{mycall}> {hiscall} RRR"` if `!stdcall(mycall)`), calls `pack77` + `unpack77`:
  - Rejects (keeps `99` sentinels) if `(ncontest == 7 && (i3 != 1 || !ok))` or `(ncontest <= 5 && (i3 != 1 || msg != msgchk || !ok))`.
  - **Gotcha:** A nonstandard `hiscall` produces `i3 = 4`, causing `ft8apset` to return early and leave `apsym[0] = 99` as well (disabling `a2` too — exact WSJT-X behavior).
- Unpacks `c77[0..58]` into bipolar `{-1, +1}`; if `nohiscall`, restores `apsym[29] = 99` and `aph10[0] = 99`.

### 2.5 `lib/ft8_decode.f90`, `lib/ft8/subtractft8.f90`, `lib/decoder.f90`, `lib/jt9.f90`

- **Pass schedule (`ft8_decode.f90` L176–L244):**
  - `ndepth == 1`: `npass = 2` (comment says 1, code executes 2!), `syncmin = 2.1`, `imetric = [1, 2]`, `lsubtract = true`.
  - `ndepth == 2`: `npass = 3`, `syncmin = 2.1`, `imetric = [1, 2, 2]`, `lsubtract = true`, pass 3 skipped if `ndecodes == 0`, `lrefinedt = false` at `nzhsym = 47`.
  - `ndepth == 3`: `npass = 3`, `syncmin = 1.3`, `imetric = [1, 2, 2]`, `lsubtract = true`, pass 3 skipped if `ndecodes == 0`, `lrefinedt = true` at `nzhsym = 47` and `50`.
  - **Immutable pass spectrum `cx` (`ft8_decode.f90` L180, `ft8b.f90` L106, `ft8_downsample.f90` L23–L29):** `newdat` is set `.true.` once per `ipass` and flipped to `.false.` on the first `ft8_downsample` call. In-pass `subtractft8` mutates `dd` without resetting `newdat`, so all candidates within a pass decode from the pass-start FFT snapshot `cx`, and subtractions take effect on the next `ipass`. (This directly validates Stage 5's lock-free `rayon` candidate parallelism!)
  - Candidate prioritization (`sync8.f90` L154–L174): candidates within `|f - nfqso| <= 10.0 Hz` and `sync >= syncmin` are placed first, followed by remaining candidates in descending `sync` order, capped at `MAXCAND = 1000`. Total unique decodes per slot capped at `MAX_EARLY = 200`.
- **DT-refined subtraction (`subtractft8.f90` L47–L104 & `lib/peakup.f90`):**
  - When `lrefinedt == true`: evaluates `sqf(-90)`, `sqf(+90)`, `sqf(0)` (`±7.5 ms` at 12 kHz). Each `sqf(idt)` subtracts the reconstructed GFSK waveform at `nstart = trunc(dt * 12000.0) + 1 + idt`, computes a `180,000`-pt r2c FFT of the residual over `[0..NFRAME]` (zero-padded to `180,000`), and sums spectral power `|cx[i]|^2` across `ia = trunc((f0 - 1.5 * 6.25) / df)` .. `ib = trunc((f0 + 8.5 * 6.25) / df)` (`df = 12000 / 180000 = 1/15 Hz`).
  - Parabolic minimum (`peakup.f90`): `b = sqb - sqa`, `c = sqb + sqa - 2*sq0`, `dx = -b / (2*c)`. If `!(dx.abs() <= 1.0)` (handles `NaN`/`Inf` when `c == 0`), aborts without subtracting; else performs final subtraction at `i2 = (90.0 * dx).round() as i32` and commits `dd0 = dd`.
- **WSJT-X 3-stage disk WAV execution (`jt9.f90` L424–L443 & `ft8_decode.f90` L111–L162):**
  - Classic `jt9 -8 -d 3 file.wav` calls `multimode_decoder` three times per WAV:
    1. `nzhsym = 41` (`141,696` samples = `11.808 s`, tail zeroed): runs 3 passes without AP (`npasses = 5`), saves `ndec_early` signals.
    2. `nzhsym = 47` (`162,432` samples = `13.536 s`): re-subtracts early decodes with `xdt_save - 0.5 < 0.396` from clean `dd` using `lrefinedt = (ndepth >= 3)`, saves `dd1`, skips `sync8`/`ft8b`.
    3. `nzhsym = 50` (`172,800` samples = `14.400 s`, tail `172,800..180,000` zeroed): merges `dd1(1..162432)` + `iwave(162433..180000)`, subtracts remaining early decodes with `lrefinedt = true`, runs 3 passes **with AP enabled** (`a1`–`a6`), then attempts `a7` and `a8`.
- **Quality & AP metadata (`ft8_decode.f90` L234, `decoder.f90` L1741–L1755, `digi-dx` `reconciler.rs` & `domain/ft8.rs`):**
  - `qual = 1.0 - (nharderrors as f32 + dmin) / 60.0`.
  - `decoder.f90` L1742–L1745: when `nap == 0`, emits no annotation (never `?`); when `nap ∈ 1..=6`, emits `a<nap>`, and prefixes `?` in column 37 iff `qual < 0.17` (`? a1`..`? a6`).
  - `digi-dx/src/domain/ft8.rs` `strip_decoder_metadata` extracts `None`, `Some("a1")`..`Some("a6")`, or `Some("? a1")`..`Some("? a6")`.
  - `digi-dx/src/qso/reconciler.rs` `confidence_rank` ranks `None => 2`, `Some("a*") => 1`, `Some("? a*") => 0` — strictly preventing confidence downgrades across passes.

### 2.6 Isolating `a1`–`a6` Parity vs. Deferred `a7`/`a8` in `ftxr-oracle`

Verified directly from `ft8_decode.f90` L251–L308 and `jt9.f90` L111–L112, L257–L261, L350:

1. **Disabling `a7` (cross-slot memory):** `ft8_decode.f90` L251 requires `ndec(jseq, 0) >= 1`, which is only populated when `jt9` processes multiple `.wav` files in a single process invocation. **Rule:** `ftxr-oracle` ALWAYS invokes `jt9` with **one `.wav` file per subprocess**; `ndec(jseq, 0)` is guaranteed `0`, so `a7` never runs.
2. **Disabling `a8` (RX-freq list decoder):** `ft8_decode.f90` L289 requires `len(trim(hiscall12)) >= 3 .and. len(trim(hisgrid)) >= 4`. Because `jt9.f90` L111–L112 hardcodes defaults `mycall='K1ABC'`, `hiscall='W9XYZ'`, `hisgrid='EN37'`:
   - For **blind / non-QSO runs** (`a1` CQ AP only, matching `jt9`'s hardcoded `lft8apon = .true.`): invoke `jt9 -8 -d 3 -c b -x b -Q 0` (`mycall`, `hiscall`, `hisgrid` all cleared → `a2`–`a8` provably disabled).
   - For **QSO AP runs (`a1`–`a6`)**: invoke `jt9 -8 -d 3 -c <MYCALL> -x <DXCALL> -g X -f <NFQSO> -Q <0..5>` (passing 1-char `-g X` sets `len(trim(hisgrid)) = 1 < 4`, so **`a8` is provably disabled** without modifying `jt9`!).
   - **Defense-in-depth:** `ftxr-oracle`'s `jt9` parser also flags and excludes any line carrying `a7`/`a8` metadata and fails an assertion if any leaks during an isolated parity run.
3. **Comparing pure no-AP runs:** Because `jt9.f90` L350 hardcodes `lft8apon = .true.`, `ftxr-oracle` evaluates no-AP behavior by (a) scoring `ftxr-core (ap_enabled = false)` against `jt9 -8 -d 3 -c b -x b -Q 0` with `a1`/`? a1` outputs partitioned separately, and (b) scoring `ftxr-core (ap_enabled = true, my_call = None, dx_call = None, qso_progress = Calling)` directly against unfiltered `jt9 -8 -d 3 -c b -x b -Q 0`.

---

## 3. Architecture & Concrete Rust Design

### 3.1 Workspace Files to Create / Update

All files stay strictly inside `/home/antigravity/mpmbq2/wsjtx/rust-ftx/`:

| File | Action | Responsibility |
| :--- | :--- | :--- |
| `crates/ftxr-core/src/fec/osd.rs` | **Create** | Bit-packed GF(2) OSD (`BitVec174`, `OsdConfig`, `OsdScratch`, `OsdResult`, `osd174_91`, `nextpat91`, `npre2` bucket table). |
| `crates/ftxr-core/src/fec/decode174_91.rs` | **Edit** | Wire `zsave[0..maxosd]` into `osd174_91`, return `FecMethod`, `nharderrors`, `dmin`, `cw`, `message91`. |
| `crates/ftxr-core/src/fec/mod.rs` | **Edit** | Export `osd` module and `FecMethod`, `OsdConfig`, `OsdScratch`, `OsdResult`. |
| `crates/ftxr-core/src/ft8/ap.rs` | **Create** | `QsoProgress`, `ContestMode`, `QsoContext`, `ApSeed`, `ApTag`, `compute_ap_seed` (`ft8apset`), `apply_ap_hypothesis` (`a1`–`a6`), `ap_pass_schedule`. |
| `crates/ftxr-core/src/ft8/ft8b.rs` | **Edit** | Add 1/2/3-symbol coherent `bmeta`–`bmete`, `imetric = 1/2`, `normalizebmet`, hard `nsync` gate, `ipass = 1..=npasses` loop with `llra`/`llrc` AP alternation, all post-decode FP gates. |
| `crates/ftxr-core/src/ft8/subtract.rs` | **Edit** | Add `lrefinedt: bool` DT-refinement (`±90` sample residual band-power FFT + `peakup`). |
| `crates/ftxr-core/src/ft8/decoder.rs` | **Edit** | Full 3-pass schedule (`ndepth = 1/2/3`, `imetric = [1, 2, 2]`, `syncmin = 2.1/1.3`), `qual` & `ApTag`, plus `Ft8SlotSession` supporting WSJT-X 3-stage `nzhsym = 41/47/50` with `lrefinedt` re-subtraction. |
| `crates/ftxr-core/src/ft8/mod.rs` | **Edit** | Export `ap` module and updated decoder APIs. |
| `crates/ftxr-core/src/telemetry.rs` | **Edit** | Populate Stage 3 `DecodeTelemetry` fields (`fec_method`, `ap_tag`, `qual`, `nharderrors`, `dmin`, `nsync_hard`, `subtraction_pass`). |
| `crates/ftxr-core/src/lib.rs` | **Edit** | Re-export `QsoContext`, `QsoProgress`, `ContestMode`, `ApTag`, `FecMethod`, updated `DecodeConfig` and `DecodedMessage`. |
| `tools/ftxr-oracle/src/osd_oracle.rs` | **Create** | Builds/invokes standalone Fortran `osd174_91` oracle driver (from unmodified WSJT-X `.f90` files) and diffs 1,000+ LLR vectors against Rust `osd174_91`. |
| `tools/ftxr-oracle/src/ft8_stage3.rs` | **Create** | Stage 3 sweeps: AWGN cliff (`-15` to `-24 dB`), AP `a1`–`a6` matrix, crowded-band, real-world corpus, and pure-noise FP suite against `jt9 -8 -d 3`. |

**Dependencies & Licensing:** Zero new crates added to `ftxr-core` (uses `std` + Stage 1–2 crates `rustfft`, `realfft`, `num-complex`, `serde`, `thiserror` — all MIT/Apache-2.0, GPLv3-compatible).

### 3.2 Bit-Packed GF(2) OSD Design (`crates/ftxr-core/src/fec/osd.rs`)

```rust
pub const N_BITS: usize = 174;
pub const K_MAX: usize = 91;
pub const K_MIN: usize = 77;
const WORD2_MASK: u64 = (1u64 << (N_BITS - 128)) - 1; // 46 bits

/// 174-bit GF(2) row vector packed into 3×u64 (LSB = bit 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct BitVec174(pub [u64; 3]);

impl BitVec174 {
    #[inline(always)]
    pub fn get(&self, bit: usize) -> bool {
        debug_assert!(bit < N_BITS);
        ((self.0[bit >> 6] >> (bit & 63)) & 1) != 0
    }
    #[inline(always)]
    pub fn set(&mut self, bit: usize) {
        self.0[bit >> 6] |= 1u64 << (bit & 63);
    }
    #[inline(always)]
    pub fn xor_assign(&mut self, rhs: &Self) {
        self.0[0] ^= rhs.0[0];
        self.0[1] ^= rhs.0[1];
        self.0[2] ^= rhs.0[2];
    }
    #[inline(always)]
    pub fn swap_bits(&mut self, a: usize, b: usize) {
        let ba = (self.0[a >> 6] >> (a & 63)) & 1;
        let bb = (self.0[b >> 6] >> (b & 63)) & 1;
        let diff = ba ^ bb;
        self.0[a >> 6] ^= diff << (a & 63);
        self.0[b >> 6] ^= diff << (b & 63);
    }
    /// Extract bits `k..174` (at most 97 bits) into the low `174 - k` bits of a `u128`.
    #[inline(always)]
    pub fn extract_lrb(&self, k: usize) -> u128 {
        debug_assert!(k >= K_MIN && k <= K_MAX);
        // k ∈ [77, 91] lies strictly in word 1 (bits 64..128):
        let shift1 = k - 64;             // 13..=27
        let lo = (self.0[1] >> shift1) as u128;
        let hi = ((self.0[2] & WORD2_MASK) as u128) << (64 - shift1);
        lo | hi
    }
}
```

**Precomputed Immutable Generator Cache:**
- `struct GeneratorCache { rows: [BitVec174; K_MAX], cols: [u128; N_BITS] }`
- Cached in `static GEN_91: OnceLock<GeneratorCache>` for the hot `k == 91` path (built once from Stage 1's `LDPC_174_91_GENERATOR`; `cols[c]` stores the 91-bit column `c` in a single `u128`).
- General `k ∈ 77..91` uses a small `[OnceLock<GeneratorCache>; 15]` table indexed by `k - 77`, building partial-CRC-cascaded rows per `osd174_91.f90` L49–L62 using Stage 1's `crc14` and `encode174_91_nocrc`.

**Per-Thread / Per-Call Scratch (Zero Heap Churn, `rayon`-Safe):**
```rust
#[derive(Debug, Clone)]
pub struct OsdScratch {
    genmrb: [BitVec174; K_MAX],
    parity_rows: [u128; K_MAX],
    indices: [usize; N_BITS],
    absrx_perm: [f32; N_BITS],
    // Lazily allocated only if ndeep >= 3 (npre2 == 1):
    boxes: Option<Box<Pre2Boxes>>,
}

#[derive(Debug, Clone, Copy)]
pub struct OsdResult {
    pub message91: [u8; K_MAX],
    pub cw: [u8; N_BITS],
    /// Positive if CRC-14 verified; negative (`-nhardmin`) or 0 if CRC failed.
    pub nhardmin: i32,
    pub crc_ok: bool,
    pub dmin: f32,
    /// Order that produced the winning codeword (0, 1, 2, ...).
    pub winning_order: u8,
    /// Diagnostic: true if Gauss-Jordan needed a pivot beyond `k + 20`.
    pub used_extended_pivot: bool,
}

pub fn osd174_91(
    llr: &[f32; N_BITS],
    k: usize,
    apmask: &[u8; N_BITS],
    ndeep: u8,
    scratch: &mut OsdScratch,
) -> OsdResult
```

**Key Fast-Path Mechanics:**
1. **Column Permutation via `cols: [u128; 174]`:** Clears `scratch.genmrb[0..k]`; for each `c in 0..174`, iterates set bits of `gen.cols[indices[c]]` (~3,867 set bits total vs. 15,834 dense checks) to populate `genmrb[r].set(c)`.
2. **Gauss-Jordan Elimination:** For `id in 0..k`, searches `icol in id..N_BITS` for first `genmrb[id].get(icol)` (identical pivot to Fortran `id..k+20` whenever a pivot exists in `id..k+20`; sets `used_extended_pivot = true` if `icol >= k + 20` rather than silently breaking `I_k`). Swaps column `id`/`icol` across `0..k` and `indices`, then XORs `genmrb[id]` into all `ii != id` with `genmrb[ii].get(id)` (3 `u64` XORs per row).
3. **128-Bit Parity Projection (`parity_rows[r] = genmrb[r].extract_lrb(k)`):**
   - Because `genmrb[0..k]` now has `I_k` in `0..k`, **all encoding and distance checks reduce to `u128` XOR + `POPCNT`**:
   - Order-0: `c0_lrb` = XOR of `parity_rows[i]` for set bits `i` of `hdec_mrb`; `e0_lrb = c0_lrb ^ hdec_lrb`.
   - For `ndeep == 2` (`nord = 1, npre1 = 1, nt = 40, ntheta = 10`):
     - `nt_mask = (1u128 << nt.min(N_BITS - k)) - 1`.
     - Outer `iflag` from `k - 1` down to `0`: skip if `(apmask_mrb >> iflag) & 1 != 0`.
     - `e2sub = e0_lrb ^ parity_rows[iflag]`, `d1 = absrx_perm[iflag]`.
     - Order-1 (`n1 == iflag`): if `(e2sub & nt_mask).count_ones() + 1 <= 10`, compute `dd = d1 + lrb_weighted_dist(e2sub, &absrx_perm[k..])`.
     - Order-2 pre-1 (`n1` from `iflag - 1` down to `0`): skip if `(apmask_mrb >> n1) & 1 != 0`; `e2 = e2sub ^ parity_rows[n1]`; if `(e2 & nt_mask).count_ones() + 2 <= 10`, compute `dd = d1 + absrx_perm[n1] + lrb_weighted_dist(e2, &absrx_perm[k..])`.
   - `lrb_weighted_dist(mut mask: u128, weights: &[f32]) -> f32` iterates set bits via `trailing_zeros()` and `mask &= mask - 1` (only ~10–30 iterations when `nd1kpt <= 10`, never touching zero bits).
4. **General `nord = 2..4` & `npre2 = 1` (`ndeep = 3..=6`):** Uses a `u128` `misub` bitmask stepped by exact `nextpat91` logic, and `Pre2Boxes` (`fp: Vec<i32>` of len `1 << ntau`, `indexes: Vec<(u8, u8)>`, `np: Vec<i32>` of len `4096`) allocated only when `npre2 == 1`.

### 3.3 Hybrid BP + OSD API (`crates/ftxr-core/src/fec/decode174_91.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FecMethod {
    Bp { iterations: u8 },
    Osd { bp_save_iter: u8, osd_order: u8 },
}

#[derive(Debug, Clone)]
pub struct Decode174Result {
    pub message91: [u8; 91],
    pub cw: [u8; 174],
    pub method: FecMethod,
    /// BP: hard errors vs channel `llr`; OSD: hard errors vs `zsave[i]` (exact WSJT-X parity).
    pub nharderrors: i32,
    /// Euclidean-weighted hard-error distance vs channel `llr`.
    pub dmin: f32,
}

pub fn decode174_91(
    llr: &[f32; 174],
    keff: usize,
    maxosd: i8,
    norder: u8,
    apmask: &[u8; 174],
    osd_scratch: &mut OsdScratch,
) -> Option<Decode174Result>
```

- Exact acceptance contract:
  - BP (`iter = 0..=30`): returns `Some` with `FecMethod::Bp { iterations: iter as u8 }` on `ncheck == 0 && crc14_check(&cw[0..91])`.
  - OSD (`i = 0..nosd`): accepts **iff `res.crc_ok && res.nhardmin > 0`** (matching Fortran L140 `nharderror .gt. 0`), recomputes `dmin` against channel `llr` (`hdec_llr = llr >= 0.0`), preserves `nharderrors = res.nhardmin`, and returns `FecMethod::Osd { bp_save_iter: (i + 1) as u8, osd_order: res.winning_order }`.

### 3.4 `QsoContext`, `ApSeed`, `ApTag` & `digi-dx` Compatibility (`crates/ftxr-core/src/ft8/ap.rs` & `src/lib.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum QsoProgress {
    #[default]
    Calling = 0,     // Tx6 (CQ)      -> [a1, a2]
    Replying = 1,    // Tx1           -> [a2, a3]
    Report = 2,      // Tx2           -> [a2, a3]
    RogerReport = 3, // Tx3           -> [a3, a4, a5, a6]
    Rogers = 4,      // Tx4           -> [a3, a4, a5, a6]
    Signoff = 5,     // Tx5           -> [a3, a1, a2]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum ContestMode {
    #[default]
    None = 0,
    NaVhf = 1,
    EuVhf = 2,
    FieldDay = 3,
    Rtty = 4,
    WwDigi = 5,
    Fox = 6,
    Hound = 7,
    ArrlDigi = 8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QsoContext {
    pub my_call: Option<String>,
    pub dx_call: Option<String>,
    pub qso_progress: QsoProgress,
    pub contest: ContestMode,
    /// RX cursor frequency (`nfqso`, default 1500.0 Hz).
    pub rx_freq_hz: f32,
    /// TX cursor frequency (`nftx`, default 0.0 Hz when unspecified to match `jt9` CLI).
    pub tx_freq_hz: Option<f32>,
    /// AP frequency window half-width (`napwid`, default 75.0 Hz).
    pub ap_wid_hz: f32,
    /// Enable AP decoding (`lft8apon`, default true).
    pub ap_enabled: bool,
    /// Restrict AP to CQ (`a1`) only (`lapcqonly`, default false).
    pub ap_cq_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ApTag {
    #[default]
    None,
    /// Single-slot AP decode `a1`..`a6`; `uncertain == true` iff `qual < 0.17`.
    Ap { kind: u8, uncertain: bool },
}

impl ApTag {
    pub fn from_wsjtx(iaptype: u8, qual: f32) -> Self {
        match iaptype {
            0 => Self::None,
            1..=6 => Self::Ap { kind: iaptype, uncertain: qual < 0.17 },
            _ => Self::None,
        }
    }
    /// Exact `raw_metadata` string expected by `digi-dx`'s `strip_decoder_metadata`
    /// and `SlotDecodeReconciler::confidence_rank`.
    pub fn to_metadata_string(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Ap { kind, uncertain: false } => Some(format!("a{kind}")),
            Self::Ap { kind, uncertain: true } => Some(format!("? a{kind}")),
        }
    }
    /// Mirrors `digi-dx/src/qso/reconciler.rs::confidence_rank`:
    /// `None => 2`, `a1..a6 => 1`, `? a1..? a6 => 0`.
    pub fn confidence_rank(self) -> u8 {
        match self {
            Self::None => 2,
            Self::Ap { uncertain: false, .. } => 1,
            Self::Ap { uncertain: true, .. } => 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ApSeed {
    /// Bipolar {-1, +1} AP bits for `MyCall DxCall`, or sentinel `99` at `[0]` / `[29]`.
    pub apsym: [i8; 58],
    /// Bipolar {-1, +1} 10-bit hash of `dx_call`, or sentinel `99` at `[0]`.
    pub aph10: [i8; 10],
}
```

### 3.5 Multi-Pass & 3-Stage Decoding (`crates/ftxr-core/src/ft8/decoder.rs`)

- **`DecodeConfig` additions:**
  - `depth: u8` (`1`, `2`, or `3`)
  - `qso_context: QsoContext`
  - `staged_mode: Ft8StagedMode`:
    - `SingleShot` (default for a single complete 15 s buffer: runs `npass` passes (`2` for depth 1, `3` for depths 2–3), with `nzhsym = 50` so AP is active; optional `refine_dt_subtract` flag for pass-1→pass-2 subtraction refinement when `depth == 3`).
    - `WsjtxThreeStage` (exact `jt9 -8 -d 3` disk-WAV parity: runs `nzhsym = 41` → `nzhsym = 47` (`lrefinedt = depth >= 3` for `dt < 0.396`) → `nzhsym = 50` (`lrefinedt = true` for `dt >= 0.396`, then 3 passes with AP enabled)).
  - **Incremental Pass API for Stage 5 / `digi-dx`:** `Ft8SlotSession` holds `dd`, `dd1`, `early_decodes`, and `all_messages` across explicit `decode_stage(nzhsym, pcm_12k)` calls (`41`, `47`, `50`), directly mapping to `digi-dx`'s 11.8 s / 13.5 s / 14.8 s streaming passes without duplicating logic.
- **Candidate Scratch & Concurrency Preparation:**
  - `Ft8CandidateScratch` bundles `cd0`, `cs`, `s8`, `s2: [f32; 512]`, `bmeta..bmete`, `llra..llre`, `llrz`, `apmask`, and `OsdScratch` — strictly owned per worker thread, zero static mutable state, ready for Stage 5 `rayon`.

---

## 4. Ordered Implementation Tasks

### Task 1 — Bit-Packed GF(2) OSD (`crates/ftxr-core/src/fec/osd.rs`)
- **Inputs:** Stage 1 `LDPC_174_91_GENERATOR`, `crc14`, `encode174_91_nocrc`.
- **Work:**
  1. Implement `BitVec174`, `GeneratorCache` (`GEN_91` + general `77..=91`), and `OsdScratch`.
  2. Implement reliability sorting (`total_cmp` descending on `abs(llr)`), sparse column-to-row permutation (`cols[indices[c]]`), Gauss-Jordan elimination over `id..N_BITS` (tracking `used_extended_pivot`), and `extract_lrb` into `[u128; 91]`.
  3. Implement order-0 evaluation, `ndeep` parameter lookup (`0..=6`, clamping `nt <= N_BITS - k`), fast-path `ndeep == 2` (`nord=1, npre1=1, nt=40, ntheta=10`), general `nextpat91` for `nord = 1..=4`, and thread-safe `Pre2Boxes` for `ndeep >= 3`.
  4. Implement codeword un-permutation (`cw[indices[pos]] = bit`), CRC-14 check, and `nhardmin` sign negation on CRC failure.
- **Acceptance Criteria:** Unit tests pass for known clean/corrupted codewords (0, 1, 2, 3 MRB bit errors), AP mask locking (never flips masked bits), `Keff ∈ {77, 84, 91}`, and `ndeep ∈ 0..=4`.

### Task 2 — Standalone Fortran `osd174_91` Oracle Driver (`tools/ftxr-oracle/src/osd_oracle.rs`)
- **Inputs:** Unmodified WSJT-X `lib/ft8/osd174_91.f90`, `encode174_91_nocrc.f90`, `get_crc14.f90`, `lib/indexx.f90`.
- **Work:**
  1. Build a tiny dev-only Fortran wrapper executable (compiled inside `tools/ftxr-oracle` build/target dir via `gfortran`, touching zero WSJT-X files) that reads binary `(k, ndeep, apmask[174], llr[174])` records from stdin and writes `(nhardmin, dmin, cw[174])` to stdout.
  2. Feed 2,000+ test vectors (clean, moderate/heavy AWGN across `-15` to `-24 dB`, BP-accumulated `zsave` vectors, AP masks `a1`–`a6`, zero/tiny LLR edge cases) to both Fortran and Rust `osd174_91`.
- **Acceptance Criteria:** **100% codeword identity** and `dmin` within `≤ 1e-4` relative error on all distinct-LLR vectors; document and verify any AP-tie edge case yields identical decoded codeword and `nhardmin`.

### Task 3 — Hybrid BP + OSD Integration (`crates/ftxr-core/src/fec/decode174_91.rs`)
- **Inputs:** Stage 2 BP loop (with `zsave` hooks) + Task 1 `osd174_91`.
- **Work:**
  1. Ensure `zsum` accumulates `iter = 0` (`zsum = llr`) and stores `zsave[iter - 1] = zsum` for `iter = 1..=maxosd` (and `zsave[0] = llr` when `maxosd == 0`).
  2. Preserve BP's exact `apmask` clamping (`zn[i] = llr[i]`, while `toc = zn - tov`).
  3. Wire post-BP OSD loop over `0..nosd`, accepting iff `res.crc_ok && res.nhardmin > 0`, recomputing `dmin` against channel `llr` while keeping `nharderrors = res.nhardmin` from `osd174_91`.
- **Acceptance Criteria:** Differential test against WSJT-X `decode174_91` (via `ldpcsim174_91` / oracle harness) matches BP vs. OSD `ntype`, `cw`, `nharderrors`, and `dmin` on identical inputs.

### Task 4 — Multi-Symbol Coherent Bit Metrics & Post-Decode Gates (`crates/ftxr-core/src/ft8/ft8b.rs`)
- **Inputs:** Stage 2 `cs(0:7, 79)` / `s8(0:7, 79)` symbol FFT output.
- **Work:**
  1. Implement hard Costas `nsync` quality check (`syncmin = 6 / 7 / 8` per `imetric` & `ndepth`).
  2. Implement `compute_bit_metrics(cs, imetric)` producing `llra..llre`:
     - Exact `nsym = 1, 2, 3` loops with `ihalf = 1, 2` and `k = 1..=29 step nsym` (preserving the exact Costas `ks+1`/`ks+2` = symbol 37/73 boundary behavior!).
     - Hoist `cs(i1, ks) + cs(i2, ks+1)` out of the `i3 = 0..8` inner loop for `nsym = 3`; compute `norm_sqr()` directly when `imetric == 2` and `norm()` when `imetric == 1`.
     - Compute `bmetd` (bit-by-bit normalized `nsym = 1`) and pre-normalization `bmete` (signed max-abs across `bmeta`, `bmetb`, `bmetc`).
     - Normalize each via `normalizebmet` (guarding `bmetsig == 0.0`) and scale by `2.83`.
  3. Implement all 6 post-decode validation gates (`nharderrors <= 36`, non-all-zero `cw`, `(i3, n3)` filter, `unpack77`, `/R` & `TU; ` filter when `ncontest == 0`, and `nsync <= 10 && xsnr < -25.0` bailout).
- **Acceptance Criteria:** On golden fixture `210703_133430.wav`, `llra..llre` match WSJT-X Fortran `ft8b.f90` dumps within `≤ 1e-4` relative error across both `imetric = 1` and `imetric = 2` (including boundary bits `81..87` and `168..174`).

### Task 5 — Single-Slot AP Engine `a1`–`a6` & `ApTag` (`crates/ftxr-core/src/ft8/ap.rs`)
- **Inputs:** Stage 1 `pack77`, `unpack77`, `is_standard_callsign`, `CallsignHashTable`.
- **Work:**
  1. Define static bipolar `const` patterns `MCQ`, `MCQ_RU`, `MCQ_FD`, `MCQ_TEST`, `MCQ_WW`, `MRRR`, `M73`, `MRR73` (verified bit-for-bit against `ft8b.f90` L40–L47).
  2. Implement `compute_ap_seed(ctx, hash_table) -> ApSeed` porting `ft8apset.f90` (including dummy `"KA1ABC"`, `<nonstd>` wrapping, sentinel `99` rules, and `aph10` calculation).
  3. Implement `apply_ap_hypothesis(iaptype, ctx, seed, f1, llrz_base) -> Option<([f32; 174], [u8; 174])>` covering every bailout condition (`napwid`, Fox, Hound `< 950 Hz`, sentinels) and every `(iaptype = 1..=6, ncontest = 0..=8)` bit mask.
  4. Wire `ipass = 1..=npasses` into `ft8b` with `nzhsym < 50 => npasses = 5`, `llra`/`llrc` pass alternation, and `apmag = max_abs(llrz) * 1.1`.
  5. Implement `ApTag::from_wsjtx(iaptype, qual)` and `to_metadata_string()`, with unit tests asserting exact round-trip compatibility with `digi-dx`'s `strip_decoder_metadata` and `confidence_rank` rules.
- **Acceptance Criteria:** Unit tests cover all 6 `QsoProgress` states × all 9 `ContestMode`s, nonstandard/missing callsign sentinels, Hound `< 950 Hz` cutoff, and `qual = 0.169` (`? a1`) vs `qual = 0.170` (`a1`) vs non-AP (`None`).

### Task 6 — DT-Refined Subtraction (`lrefinedt`) & Full 3-Pass / 3-Stage Scheduler (`subtract.rs`, `decoder.rs`)
- **Inputs:** Stage 2 `subtractft8` and `decoder.rs`.
- **Work:**
  1. Extend `subtractft8` with `lrefinedt: bool` and reusable `SubtractScratch` (pre-allocated `180,000`-pt r2c FFT plan + `cw` filter spectrum + `endcorrection[2001]`):
     - Implement `sqf(idt)` returning residual power over `[ia..=ib]` (`f0 - 9.375 Hz` to `f0 + 53.125 Hz`).
     - Implement `peakup(sqa, sq0, sqb)` with safe finite guard (`!(dx.abs() <= 1.0)` → abort subtraction), final `sqf(nint(90.0 * dx))`, and commit to `dd`.
  2. Update `decoder.rs`:
     - Exact `ndepth = 1/2/3` pass count (`2/3/3`), `syncmin` (`2.1 / 2.1 / 1.3`), `imetric` (`1, 2, 2`), `nfqso ± 10 Hz` candidate prioritization, `MAXCAND = 1000`, `MAX_EARLY = 200`, `qual = 1.0 - (nharderrors + dmin)/60.0`.
     - Implement `Ft8SlotSession` supporting both `SingleShot` and `WsjtxThreeStage` (`nzhsym = 41 / 47 / 50` with `xdt < 0.396` / `>= 0.396` `lrefinedt` re-subtraction).
  3. Populate Stage 3 `DecodeTelemetry` fields (`fec_method`, `ap_tag`, `qual`, `nharderrors`, `dmin`, `nsync_hard`, `subtraction_pass`).
- **Acceptance Criteria:** Post-`lrefinedt` residual `dd` matches Fortran `subtractft8(..., .true.)` within `≤ 1e-4` relative error on strong signals; `Ft8SlotSession` runs cleanly in both `SingleShot` and `WsjtxThreeStage` modes.

### Task 7 — Stage 3 Oracle Verification Suite (`tools/ftxr-oracle/src/ft8_stage3.rs`)
- **Inputs:** Tasks 1–6 + Stage 0 golden corpus and `jt9` binary.
- **Work:** Implement and run all 5 verification suites in Section 5, emit machine-readable JSON parity report (`stage3_scorecard.json`), and tune/fix any numerical discrepancy until every exit gate passes.

---

## 5. Verification, Oracle Testing & Numerical Gates

### 5.1 Suite A — Fortran vs. Rust `osd174_91` Unit Oracle
- **Test set:** 2,000 LLR vectors:
  - 500 uncoded/coded AWGN vectors at `Eb/N0` from `-1` to `+5 dB` (`Keff = 91`, `ndeep = 2`, `apmask = 0`)
  - 500 real BP-accumulated `zsave[:, 1..=2]` vectors dumped from `-20` to `-23 dB` FT8 frames
  - 500 AP-masked vectors (`a1`..`a6`, `apmag = max(|llr|)*1.1`)
  - 250 `Keff ∈ {77, 84, 91}` × `ndeep ∈ {0, 1, 2, 3}` vectors
  - 250 adversarial edge vectors (all-zero LLRs, exact `0.0` LLRs, extreme `±1e4` LLRs, rank-edge permutations)
- **Pass criteria:** **100% identical decoded `cw`** and `nhardmin` on all distinct-LLR vectors; `|dmin_rust - dmin_fortran| / max(1.0, dmin_fortran) <= 1e-4`; 0 panics or NaNs on adversarial vectors.

### 5.2 Suite B — Intermediate Pipeline Numerical Diff
- **Test set:** `samples/FT8/210703_133430.wav` and `191111_110200.wav` (top 10 candidates per pass).
- **Compared arrays:** `bmeta..bmete`, `llra..llre` (both `imetric = 1` and `imetric = 2`), `apsym`/`aph10`, `apmask`/`llrz` (for `a1`–`a6`), and post-`lrefinedt` residual `dd`.
- **Pass criteria:** Relative L2 / max error `<= 1e-4` against Fortran dumps.

### 5.3 Suite C — Deterministic AWGN Sensitivity Cliff (`-15` to `-24 dB`)
- **Generator:** Seeded Rust PRNG AWGN mixed with clean 12 kHz FT8 waveforms (deterministic seed, e.g., `seed = 0x5354_4733`), **100 trials per 1 dB step** from `-15 dB` down to `-24 dB` (`1,000` slots per configuration), randomized `AF ∈ [500, 2500] Hz` and `DT ∈ [-0.5, +0.5] s`.
- **Configurations tested against `jt9 -8 -d 3` (1 WAV per subprocess):**
  1. **Blind (`a1` CQ AP on, `a2`–`a6` off):** Message `"K1ABC W9XYZ EN37"` (non-CQ, tests pure BP+OSD) and `"CQ K1ABC FN42"` (tests `a1` gain). Oracle CLI: `jt9 -8 -d 3 -c b -x b -Q 0 -L 200 -H 3000 <wav>`.
  2. **QSO AP `a2` (`MyCall ??? ???`):** `"K1ABC W9XYZ EN37"`, `-c K1ABC -x b -Q 0`.
  3. **QSO AP `a3` (`MyCall DxCall ???`):** `"K1ABC W9XYZ -18"`, `-c K1ABC -x W9XYZ -g X -f <AF> -Q 2`.
  4. **QSO AP `a4`/`a5`/`a6` (`RRR` / `73` / `RR73`):** `"K1ABC W9XYZ RR73"` (and `RRR`, `73`), `-c K1ABC -x W9XYZ -g X -f <AF> -Q 3`.
  5. **Hound AP (`ncontest = 7`):** `"W9XYZ RR73; K1ABC <KH1/KH7Z> -12"` at `AF = 600 Hz` (`< 950 Hz`) and `AF = 1200 Hz` (`> 950 Hz`, verify AP skipped), `-X 7 -c K1ABC -x KH1/KH7Z -g X -Q 1`.
- **Pass criteria:**
  - Interpolated 50% decode threshold within **≤ 0.5–1.0 dB** of `jt9 -8 -d 3` on every configuration (target `≤ 0.5 dB`, typically `-21 dB` blind and `-23` to `-24 dB` with `a4`–`a6`).
  - Zero `a7`/`a8` lines leaked from `jt9`.

### 5.4 Suite D — Crowded-Band & Real-World Corpus Diff
- **Crowded-band synthetic test:** 20 deterministic slots each containing **15–25 overlapping FT8 signals** across `300–2800 Hz`, spanning **25 dB dynamic range** (`+5 dB` down to `-20 dB`), including deliberate co-channel/adjacent collisions (`Δf = 10–30 Hz`, staggered `DT`) to stress 3-pass subtraction + `lrefinedt`.
- **Real-world corpus:** `samples/FT8/210703_133430.wav`, `191111_110200.wav`, plus all curated quiet/crowded/fading fixtures committed in Stage 0.
- **Oracle invocation:** `jt9 -8 -d 3 -c b -x b -Q 0 -L 200 -H 3000 <wav>` (single file per process). Evaluated for BOTH `Ft8StagedMode::WsjtxThreeStage` (exact parity) and `Ft8StagedMode::SingleShot`.
- **Pass criteria:**
  - Real-world total decode count **≥ 95–98% of `jt9 -8 -d 3`** (excluding any `a7`/`a8`).
  - Crowded-band decode count **≥ 95% of `jt9 -8 -d 3`**.
  - Matched-decode deltas vs. `jt9`: **|ΔAF| ≤ 2 Hz**, **|ΔDT| ≤ 0.1 s**, **|ΔSNR| ≤ 2 dB** (≥ 99% of matched pairs).
  - Verify `SingleShot` vs `WsjtxThreeStage` delta is documented (expected ≤ 1–2% on crowded bands when pass-1→pass-2 `lrefinedt` is enabled).

### 5.5 Suite E — Pure-Noise False-Positive Sweep & AP Tag Audit
- **Test set:** **500 pure AWGN 15 s slots** (deterministic seed, RMS matched to real-world noise floor).
- **Configurations:**
  1. No AP (`ap_enabled = false`)
  2. Blind AP (`ap_enabled = true`, `a1` CQ only) vs `jt9 -8 -d 3 -c b -x b -Q 0`
  3. Full QSO AP (`QsoProgress::RogerReport` = `a3`–`a6`, `rx_freq_hz = 1500.0`, `ap_wid_hz = 75.0`) vs `jt9 -8 -d 3 -c K1ABC -x W9XYZ -g X -f 1500 -Q 3`
- **Pass criteria:**
  - Non-AP false-decode rate **`< 0.1%`** (0–1 FPs across 500 slots) and **≤ `jt9 -8 -d 3`**.
  - AP false-decode rate **≤ `jt9 -8 -d 3`**, with low-quality AP hits accurately tagged `? a1`–`? a6` (`qual < 0.17`).
  - Every emitted `ApTag::to_metadata_string()` passes `digi-dx`'s `strip_decoder_metadata` and `confidence_rank` invariants (`None => 2`, `a1..a6 => 1`, `? a1..? a6 => 0`).

---

## 6. Risks, Gotchas & Cross-Stage Handoff

### 6.1 Top Non-Obvious Gotchas & Mitigations

1. **OSD `nharderror` vs. `dmin` Asymmetry (`decode174_91.f90` L139–L147):** `dmin` is recomputed against raw channel `llr`, while `nharderror` stays as `osd174_91`'s count against `zsave(:, i)`, AND acceptance requires `nharderror > 0` (strictly positive). *Mitigation:* Mirror this exact contract in `decode174_91.rs` and assert it in Suite A/B.
2. **AP Pass Alternation Comment Bug (`ft8b.f90` L263–L270 vs L295–L296):** Comments say `nsym=1`/`nsym=2`, but code actually alternates `llra` (`nsym=1`) and `llrc` (`nsym=3`). *Mitigation:* Follow executable code (`llra`/`llrc`) and assert against Fortran dumps.
3. **Multi-Symbol Boundary Overlap Into Costas Arrays (`ft8b.f90` L189–L205):** `nsym = 2` (`k = 29`) and `nsym = 3` (`k = 28`) coherently sum the final data symbol(s) with Costas symbol `37` or `73`. *Mitigation:* Index into the full 79-symbol `cs` array identically to Fortran.
4. **AP Tie-Breaking in Reliability Sort (`osd174_91.f90` L76):** All AP bits have identical magnitude `2 * apmag` (iter 1) or `3 * apmag` (iter 2). Because AP bits cannot be flipped and sit in `0..k`, tie order among AP bits does not affect valid search patterns unless ties straddle position `k` or `k + nt`. *Mitigation:* Verify AP tie cases explicitly in Suite A; use deterministic `total_cmp` descending sort.
5. **Gauss-Jordan Ad-Hoc `k+20` Pivot Limit (`osd174_91.f90` L87):** Fortran stops searching at column `k + 20 = 111`, which can theoretically leave `genmrb(:, 1:k) ≠ I_k`. *Mitigation:* Search `id..N_BITS` (identical pivot whenever found in `id..k+20`, full-rank safe otherwise) and record `used_extended_pivot` telemetry in oracle runs.
6. **`jt9` Disk-WAV 3-Stage Execution & Hardcoded Callsign Defaults (`jt9.f90` L111–L112, L424–L443):** `jt9` defaults to `K1ABC`/`W9XYZ`/`EN37` (silently activating `a2`–`a6` and `a8`!) and runs `nzhsym = 41 → 47 → 50` with `lrefinedt` re-subtraction. *Mitigation:* Always pass `-c b -x b` (blind) or `-c ... -x ... -g X` (AP `a1`–`a6` with `a8` disabled), run 1 WAV per `jt9` process (disabling `a7`), and provide both `WsjtxThreeStage` and `SingleShot` modes in `ftxr-core`.
7. **Licensing Hygiene:** Never port `bessi0` (Numerical Recipes, unused in `ft8b.f90`) or NR `indexx.f90` into `ftxr-core`.

### 6.2 Explicit Cross-Stage Handoff Contracts

| Boundary | Interface / Invariant |
| :--- | :--- |
| **Consumed from Stage 1** | `pack77`, `unpack77`, `is_standard_callsign`, `CallsignHashTable::save_hash_call -> (u16, u16, u32)`, `crc14` / `crc14_check`, `encode174_91_nocrc`, `LDPC_174_91_GENERATOR`, `ft8_tones_from_77bits`, `gen_ft8_wave`. |
| **Consumed from Stage 2** | `sync8` (with `nfqso ± 10 Hz` top-of-list prioritization), `get_spectrum_baseline`, `ft8_downsample` (pass-level `cx` cache), `sync8d`, `twkfreq1`, BP loop in `decode174_91` (with `zsave` hooks), base `subtractft8`. |
| **Exposed to Stage 4 (FT4)** | Reuses `decode174_91` (`maxosd = 3` at `nfqso`, `norder = 2`, `Keff = 91`), `osd174_91`, `OsdScratch`, `FecMethod`, `QsoContext`, `QsoProgress`, `ContestMode`, `ApTag`, and `qual = 1.0 - (nharderrors + dmin)/60.0` (`? a*` at `< 0.17`). Stage 4 adds `rvec`-scrambled AP masks in `src/ft4/ap.rs`. |
| **Exposed to Stage 5 (`ftxr-modem` & Parallelism)** | Zero global mutable state in `osd.rs`, `decode174_91.rs`, `ap.rs`, `ft8b.rs`, and `subtract.rs`. Immutable pass-level `cx` matches classic `ft8_decode.f90` `newdat` semantics, allowing lock-free `rayon` candidate decoding with deterministic post-pass subtraction ordering. `Ft8SlotSession` directly supports `digi-dx`'s 3-pass cadence (11.8 s / 13.5 s / 14.8 s) and populates `DecodeTelemetry`. |
| **Exposed to Stage 7 (`digi-dx` Blueprint)** | `ApTag::to_metadata_string()` emits exact `None` / `"a1"`–`"a6"` / `"? a1"`–`"? a6"` strings compatible with `digi-dx/src/domain/ft8.rs::strip_decoder_metadata` and `digi-dx/src/qso/reconciler.rs::SlotDecodeReconciler::confidence_rank`. |
