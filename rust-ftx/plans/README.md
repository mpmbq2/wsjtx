# `ftxr-modem` Staged Implementation Plans

This directory contains the detailed, code-grounded implementation plans for all 8 stages of [`../ROADMAP.md`](../ROADMAP.md). Every plan was audited directly against WSJT-X's Fortran/C++ sources (classic `lib/ft8/`, `lib/ft4/`, `lib/77bit/`, `lib/jt9.f90`, `lib/decoder.f90`, `Modulator/Modulator.cpp`, `CMakeLists.txt`) and `digi-dx` (read-only reference).

---

## Stage Index

| Stage | Plan Document | Scope & Focus | Key Exit Gate |
| :--- | :--- | :--- | :--- |
| **0** | [`stage-0-environment-oracle.md`](stage-0-environment-oracle.md) | Headless WSJT-X oracle build (`jt9`, `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`), GPLv3 Cargo workspace (`ftxr-core`, `ftxr-modem`, `ftxr-oracle`), compact golden fixtures, deterministic seeded-PRNG sweep generator & `jt9` baseline scorecard | All 6 oracle tools build; workspace compiles cleanly; deterministic baseline scorecard committed |
| **1** | [`stage-1-codec-tx.md`](stage-1-codec-tx.md) | Pure-Rust 77-bit `packjt77` + thread-safe `CallsignHashTable`, CRC-14 (`0x2757`), LDPC(174,91) encoder, FT4 `rvec` scrambler, FT8 (79-sym, `BT=2.0`) & FT4 (105-sym, `BT=1.0`) GFSK TX modulators (real + complex, configurable `start_delay_ms` & late-TX `offset_ms`) | 100% bit-exact parity vs `ft8code`/`ft4code` (5,000+ fuzzed msgs/mode); 100% `jt9 -8/-5` loopback decode incl. FT8 1–5 s & FT4 0.5–1.5 s late-TX + hot-swap |
| **2** | [`stage-2-ft8-core-decoder.md`](stage-2-ft8-core-decoder.md) | Classic FT8 core RX pipeline: `sync8`, baseline, `ft8_downsample` (immutable per-pass `cx`), `sync8d`, `ft8b` (1-sym LLRs), `decode174_91` (BP-only `max_osd = -1` with `zsave` hooks), `subtractft8` | Fortran stage-dump diffs `≤ 1e-4`; ≥99% synthetic (0 to -15 dB); ≥90% `jt9 -8 -d 1` real corpus; 0% pure-noise FPs; passes 5 s late-TX & 11.8/13.5 s partial slots |
| **3** | [`stage-3-ft8-weak-signal-parity.md`](stage-3-ft8-weak-signal-parity.md) | Bit-packed GF(2) `osd174_91`, multi-symbol coherent LLRs (`bmeta`–`bmete`), single-slot AP `a1`–`a6` + `? a*` (`qual < 0.17`), `lrefinedt` & 3-stage `nzhsym=41/47/50` parity vs classic `jt9 -8 -d 3` (`a7`/`a8` deferred) | 100% OSD unit parity vs Fortran `osd174_91`; ≥95–98% classic `jt9 -8 -d 3` real-world decodes; ≤0.5–1.0 dB AWGN cliff delta; bounded FPs |
| **4** | [`stage-4-ft4-decoder-parity.md`](stage-4-ft4-decoder-parity.md) | Complete FT4 RX pipeline: `getcandidates4`, `ft4_baseline`, `ft4_downsample` (7-smooth 72,576/4,032 FFTs), 3-segment `sync4d`, 1/2/4-sym `get_ft4_bitmetrics`, `rvec`-domain AP `a1`–`a6`, `maxosd=3` near `nfqso`, `subtractft4` | Stage diffs `≤ 1e-4`; ≥95% `jt9 -5` decodes; ≤0.5–1.0 dB AWGN cliff delta; bounded FPs; 100% nominal & late-start FT4 loopback |
| **5** | [`stage-5-modem-cli-ipc-parallelism.md`](stage-5-modem-cli-ipc-parallelism.md) | Warm `DecoderEngine` (`FftPlanCache`, `ScratchPool`, private `rayon` pool, deterministic post-pass hash/subtraction commit, `AtomicBool` cancel), optional `DecodeTelemetry`, `ftxr-modem` standalone CLI & multiplexed `FTXR` v1 stdio worker (`--worker`) | Pass 1 <150 ms / Pass 2 <250 ms / Pass 3 <500 ms (dev host); IPC <2 ms; concurrent `encode_tx` unblocked during Pass 3; <50 ms cancel; 1,000-slot soak; zero sensitivity regression |
| **6** | [`stage-6-repo-extraction-packaging.md`](stage-6-repo-extraction-packaging.md) | Lift-and-shift to standalone `GPL-3.0-only` repo `mpmbq2/ftxr-modem`, WSJT-X `TRADEMARK.md` & `NOTICE` compliance, `cargo-deny` audit, GitHub Actions CI & 4-target release matrix (`linux/darwin × x86_64/aarch64`) + `SHA256SUMS` + source tarball | Clean-room Rust-only build/tests pass; 4-target tarballs + `SHA256SUMS` published; final `ftxr-oracle` check matches Stage 5 identically; `wsjtx` `master` clean |
| **7** | [`stage-7-digi-dx-integration-blueprint.md`](stage-7-digi-dx-integration-blueprint.md) | **Read-only blueprint** for future execution in `mpmbq2/digi-dx`: MIT + GPLv3 aggregate install (`/usr/local/lib/digi-dx/ftxr-modem`), `FtxrWorkerClient`, 3-backend `HybridFt8Decoder` (`auto`: `ftxr_modem → jt9 → ft8_lib`), Phase 7A (`jt9`-parity FT8) & Phase 7B (`digi-dx` FT4 live-loop remediation) | Zero GPL crates in `digi-dx/Cargo.lock`; all 3 backends selectable in config/preflight/API/UI; crash-recovery, reconciler, and A/B test suite defined |

---

## Canonical Cross-Stage Contracts (Single Source of Truth)

Because all 8 stage plans were drafted in parallel, the following canonical definitions reconcile minor naming differences across early/late stage plans and govern implementation:

1. **Workspace & Licensing:**
   - Dev workspace: `wsjtx/rust-ftx/Cargo.toml` (`crates/ftxr-core`, `crates/ftxr-modem`, `tools/ftxr-oracle`).
   - Extracted standalone repo: `mpmbq2/ftxr-modem` (`crates/ftxr-core`, `crates/ftxr-modem`; `tools/ftxr-oracle` stays in `wsjtx`).
   - SPDX license: **`GPL-3.0-only`** across `ftxr-core` and `ftxr-modem`, canonical FSF `gpl-3.0.txt` in `LICENSE`, upstream WSJT-X attribution in `NOTICE` conforming to `wsjtx/TRADEMARK.md`, zero Numerical Recipes code (`bessi0`, `indexx`, `shell` excluded/replaced with Rust `std`).
2. **Canonical `ModulateConfig` Fields (`ftxr-core`):**
   - `mode: Mode` (`Ft8 | Ft4`)
   - `af_hz: f32`
   - `sample_rate: u32`
   - `start_delay_ms: u32` — use constructors `ModulateConfig::default_for(mode, af_hz, sample_rate)` (`500` FT8 / `300` FT4) and `ModulateConfig::raw_active(mode, af_hz, sample_rate)` (`0` ms for `digi-dx` Phase 7A parity)
   - `offset_ms: u64` — UTC-aligned late-TX / mid-slot hot-swap offset into the slot
   - `amplitude: f32` — peak amplitude in `(0.0, 1.0]`
3. **Canonical `ApTag` & `raw_metadata` Contract (`ftxr-core` ↔ `digi-dx`):**
   - `ftxr-core` emits `ApTag::None | ApTag::Ap { kind: u8, uncertain: bool }` with `kind ∈ 1..=6` and `uncertain = qual < 0.17`.
   - `wsjtx_metadata()` produces `None`, `Some("a1")`..`Some("a6")`, or `Some("? a1")`..`Some("? a6")` — matching `digi-dx`'s `strip_decoder_metadata` and `SlotDecodeReconciler::confidence_rank` verbatim.
   - `ftxr-oracle`'s `jt9` stdout parser separately recognizes `a7`/`a8` (if ever encountered) so apples-to-apples comparisons exclude deferred `a7`/`a8` decodes.
4. **Canonical Stdio IPC Protocol v1 (`ftxr-modem --worker` ↔ `digi-dx` `FtxrWorkerClient`):**
   - Framing: 16-byte `FTXR` little-endian header (`magic: b"FTXR"`, `version: u16 = 1`, `flags: u16 = 0`, `json_len: u32`, `bin_len: u32`) + UTF-8 JSON + raw binary PCM (`f32le`/`i16le`). Stdout is strictly framed; all diagnostics go to stderr.
   - Canonical op names: `hello` → `ready`, `set_context` → `ok`, `session_reset` → `ok`, `decode_slot` → `0..N decode_event` + `decode_done`, `cancel` (`job_id`) → `cancel_ack` + `decode_done(status="cancelled")`, `encode_tx` (`start_delay_ms`, `offset_ms`) → `encode_result`, `ping` → `pong`, `shutdown` → `ok`, `error`.
   - Multiplexing invariant: `encode_tx` runs on an isolated OS thread outside the RX `rayon` pool so an in-flight Pass 3 deep decode can never stall next-slot TX modulation or mid-slot hot-swap.

---

## Key High-Impact Gotchas Highlighted Across the Audits

- **Oracle CLI hygiene (Stages 0, 2, 3):** Classic `jt9.f90` defaults to `mycall='K1ABC'`, `hiscall='W9XYZ'`, `hisgrid='EN37'`, `nQSOProg=0`, `lft8apon=.true.`. Use `-c b -x b -Q 1` for true zero-AP blind runs, `-g .` (length < 4) to suppress `a8`, and one WAV per `jt9` process to suppress cross-file `a7` leakage. Always pass `-DWSJT_SKIP_MANPAGES=ON -DWSJT_SKIP_MAP65=ON` to CMake, and ensure WAV filenames have ≥6 chars before `.wav` (Release `-fbounds-check` crash otherwise).
- **Exact Fortran parity traps (Stages 1–4):** `RR73` packs as Grid4 `32373` (not `32403`); Type 4 `CQ <nonstd>` writes `hash12` into `n12`; `sync8.f90` truncates `jstrt = 0.5/0.04` to `12`; `subtractft8` / `subtractft4` rotate `cw` by `NFILT/2 + 1` (`2001` / `701`); `ft8b.f90` alternates `llra` & `llrc` (despite comment saying `nsym=2`) and includes Costas symbols 37/73 in `nsym=2/3` sums; `decode174_91` requires `nharderror > 0` for OSD and keeps `zsave`-based `nharderror` while recomputing `dmin` on raw `llr`; `ft4_decode` puts all AP bits in the `rvec`-scrambled domain, checks all-zero `message77` *before* descrambling, and clips trailing `|ibest|` samples when `ibest < 0`.
- **Natural parallelism safety (Stages 2, 4, 5):** Both classic FT8 (`ft8_downsample.f90`) and FT4 (`ft4_decode.f90`) compute the big R2C FFT `cx` once per pass and keep it immutable across all candidates in that pass—enabling exact `rayon` parallelism over `&cx` with deterministic `candidate_idx`-ordered post-pass hash-table commits and subtraction.
- **`digi-dx` boundary & FT4 prerequisites (Stage 7):** `digi-dx` remains untouched during Stages 0–6. When Stage 7 is executed in `digi-dx`, Phase 7A achieves immediate FT8 `jt9`-parity integration, while Phase 7B addresses `digi-dx`'s hardcoded 15 s live-loop/PTT/buffer/parity/ADIF/`jt9 -5` paths to unlock live FT4 RX/TX.
