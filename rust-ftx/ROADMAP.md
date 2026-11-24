# Pure-Rust FT8 & FT4 Engine Roadmap (Develop in `wsjtx` → Independent GPLv3 Project → Bundled 3rd Backend in `digi-dx`)

## 1. Purpose, Licensing & Independence Strategy

This roadmap defines the staged plan to build, verify, and extract a pure-Rust **FT8 + FT4** modem engine (decoder + encoder/modulator) as a standalone project that `digi-dx` installs and bundles as an independent dependency:

1. **Develop & validate inside `wsjtx` (Stages 0–5):** Build the Rust engine on an isolated branch alongside WSJT-X's Fortran reference tools (`jt9`, `ft8code`, `ft4code`, `ft8sim`, `ft4sim`) so every stage is verified numerically and behaviorally against `jt9`.
2. **Lift-and-shift to an independent GPLv3 repository (Stage 6):** Extract the engine into its own standalone Rust project (placeholder name: `ftx-modem`, e.g., `mpmbq2/ftx-modem`) with zero `wsjtx`/Fortran/FFTW dependencies and cross-platform release binaries.
3. **Bundle & integrate as a user-selectable 3rd backend in `digi-dx` (Stage 7):** Install/bundle the `ftx-modem` binary alongside `digi-dx` (mirroring how `digi-dx` already provisions `rigctld` and `jt9` in `mpmbq2/digi-dx:docs/deployment-and-deps.md`) and expose it alongside `jt9` (`src/modem/jt9.rs`) and `ft8_lib` (`src/modem/ft8_lib.rs`).

### Licensing Architecture (MIT `digi-dx` + GPLv3 `ftx-modem`)

> [!IMPORTANT]
> **Why `ftx-modem` runs as an independent warm worker binary rather than a statically linked Cargo crate inside `digi-dx`:**
> - **New Rust modem project (`ftx-modem`) — GPLv3:** Because it is explicitly GPLv3 (with proper attribution to upstream WSJT-X in [`COPYING`](../COPYING)), we can **directly port** WSJT-X's proven Fortran FT8/FT4 algorithms and perform **stage-by-stage numerical verification** against WSJT-X's Fortran internals—the fastest, lowest-risk path to true `jt9` sensitivity parity.
> - **`digi-dx` — MIT (`digi-dx/Cargo.toml`):** Statically linking a GPLv3 Rust crate into the `digi-dx` binary would cause the distributed `digi-dx` executable to fall under GPLv3. Instead, `digi-dx` ships **zero GPL code in its Cargo dependency tree** and communicates with the independent `ftx-modem` binary over a **documented process/IPC boundary** (an aggregate distribution, identical in principle to `digi-dx`'s boundary with `jt9` and `rigctld`).
> - **Zero performance sacrifice:** Unlike cold per-pass `jt9` WAV spawns, `ftx-modem` runs as a **persistent warm worker process** over stdio/local IPC—FFT plans stay warm in RAM, raw 12 kHz PCM buffers stream over a pipe in <1 ms (no temp WAV files), decodes stream back as JSON events immediately, and mid-pass cancellation takes <50 ms.
> - **Ecosystem bonus:** Because `ftx-modem` is a completely independent project, anyone can install and use it standalone (CLI or daemon) without needing WSJT-X *or* `digi-dx`.
> - **Bundling compliance:** When `digi-dx`'s installer or release bundle includes `ftx-modem`, it ships `ftx-modem`'s GPLv3 `LICENSE`, copyright notice, and source repository link alongside the binary.

---

## 2. Independent Project & Lift-and-Shift Architecture

To make Stage 6 a clean extraction into an independent repository, the code developed inside `wsjtx` is structured from day one as an independent GPLv3 Cargo workspace plus a disposable `wsjtx`-only oracle harness:

```mermaid
flowchart LR
    subgraph WSJTX["Phase 1: Dev & Verification (wsjtx branch)"]
        Core["ftx-core (GPLv3 pure-Rust lib: FT8/FT4 RX + TX)"]
        Bin["ftx-modem (GPLv3 binary: standalone CLI + warm IPC worker)"]
        Oracle["ftx-oracle (Dev-only harness, stays in wsjtx)"]
        Ref["WSJT-X Reference Tools (jt9, ft8code, ft4code, ft8sim, ft4sim)"]
        Bin --> Core
        Oracle --> Core
        Oracle --> Bin
        Oracle --> Ref
    end

    subgraph NEWREPO["Phase 2: Independent GPLv3 Project (mpmbq2/ftx-modem)"]
        ExtCore["ftx-core (GPLv3 crate)"]
        ExtBin["ftx-modem (GPLv3 CLI + warm worker binary)"]
        Rel["Cross-Platform Releases (Linux x86_64/aarch64, macOS arm64/x86_64)"]
        ExtBin --> ExtCore
        ExtBin --> Rel
    end

    subgraph DIGIDX["Phase 3: MIT-Licensed digi-dx (mpmbq2/digi-dx)"]
        Inst["Installer / Bundle (install.sh installs pinned ftx-modem binary)"]
        Client["IPC Adapter (src/modem/ftx_modem.rs — MIT)"]
        Router["Selectable 3-Backend Coordinator (ftx_modem | jt9 | ft8_lib | auto)"]
        UI["Config, Preflight, API & Web UI Selector"]
        Inst -. "Bundles separate executable" .-> ExtBin
        Client -. "Persistent warm stdio IPC" .-> ExtBin
        Client --> Router
        UI --> Router
    end

    Core -. "Stage 6: Lift-and-Shift" .-> ExtCore
    Bin -. "Stage 6: Lift-and-Shift" .-> ExtBin
```

### Architectural Invariants
- **Strict zero-coupling inside `ftx-core` / `ftx-modem`:** No dependencies on `wsjtx`, Qt, Fortran, FFTW, or `digi-dx`. Pure Rust (`cargo build --release`) producing a single static/self-contained native executable per platform.
- **Versioned IPC contract:** `ftx-modem --worker` speaks a clean, versioned, documented framing protocol (JSON control/events + raw binary PCM payloads over stdin/stdout) so `digi-dx` and `ftx-modem` remain loosely coupled and independently upgradeable.
- **Standalone CLI mode:** `ftx-modem` also operates as a standalone CLI for decoding `.wav` files and encoding FT8/FT4 signals without `digi-dx`.
- **Mode-aware from day one:** Unified support for both **FT8** (15 s slot) and **FT4** (7.5 s slot), matching `digi-dx`'s `digital_mode` (`"FT8" | "FT4"`) and `SlotClock` (`digi-dx/src/domain/clock.rs`).
- **Reconciler-compatible output:** Emits core fields (`snr_db`, `dt_sec`, `af_hz`, `message`), WSJT-X-compatible AP/confidence tags matching `digi-dx`'s `SlotDecodeReconciler` (`digi-dx/src/qso/reconciler.rs`), and optional rich telemetry (FEC path, sync score, Costas frequency drift, QSB fading envelope).
- **Clean `wsjtx` footprint:** All Rust code lives in an isolated directory (`rust-ftx/`) on a development branch inside `wsjtx` so upstream WSJT-X source files remain untouched (aside from temporary, non-merged Fortran dump hooks if used during Stages 2–4).

---

## 3. Staged Development Roadmap

### Stage Overview

| Stage | Location | Focus | Primary Oracle | Gate Summary |
| :--- | :--- | :--- | :--- | :--- |
| **0** | `wsjtx` | Toolchain, Workspace & Golden Corpus | `jt9`, `ft8sim`, `ft4sim` | Oracle builds, GPLv3 workspace scaffolded, baseline `jt9` scorecard recorded |
| **1** | `wsjtx` | 77-Bit Protocol, FEC & TX Modulator (FT8 + FT4) | `ft8code`, `ft4code`, `jt9` | 100% bit-exact packing/tones; 100% `jt9` loopback decode incl. late TX |
| **2** | `wsjtx` | Core FT8 Decoder MVP (Sync + BP + Subtraction) | Fortran intermediates + `jt9 -8 -d 1`/`2` | Stage diffs within tol; ≥99% synthetic (-15 dB), ≥90% real `-d 1`, 0% noise FPs |
| **3** | `wsjtx` | FT8 Weak-Signal Parity (OSD + Multi-Sym LLRs + AP) | Fortran OSD + `jt9 -8 -d 3` | ≥95–98% of `jt9 -d 3` decodes, ≤0.5–1.0 dB threshold delta, bounded FPs |
| **4** | `wsjtx` | FT4 Decoder Parity | Fortran intermediates + `jt9 -5` | ≥95% of `jt9 -5` decodes, ≤0.5–1.0 dB threshold delta, bounded FPs |
| **5** | `wsjtx` | `ftx-modem` Binary (CLI + Warm IPC Worker), Parallelism & Telemetry | `jt9` 3-pass timing via IPC | Warm IPC Pass 1/2/3 decisively faster than `jt9`, <50 ms cancel, 1,000-slot soak |
| **6** | New GPLv3 Repo | Lift-and-Shift Extraction & Release Packaging | Standalone tests + final oracle check | Independent GPLv3 repo builds & passes with zero `wsjtx` deps; cross-arch releases ready |
| **7** | `digi-dx` (MIT) | Bundled Dependency & Selectable 3rd Backend | `digi-dx` test suite + live A/B | `digi-dx` stays 100% MIT; selectable `ftx_modem` / `jt9` / `ft8_lib` (+ `auto`) in UI/config |

---

### Stage 0 — Baseline Environment, Golden Corpus & Oracle Harness (in `wsjtx`)

**What we develop:**
- Install Fortran/FFTW build dependencies in the dev environment and compile WSJT-X reference binaries (`jt9`, `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`) from `wsjtx`.
- Scaffold the isolated Rust workspace inside `wsjtx` (`rust-ftx/`) with three crates:
  - `ftx-core` (GPLv3 library crate)
  - `ftx-modem` (GPLv3 standalone CLI + warm IPC worker binary crate)
  - `ftx-oracle` (dev-only test & benchmark harness crate — stays in `wsjtx`, never extracted)
- Add GPLv3 `LICENSE` and upstream WSJT-X attribution files inside the extractable project directory.
- Assemble a frozen **Golden Test Corpus**:
  - Real-world recordings (starting from [`samples/FT8/210703_133430.wav`](../samples/FT8/210703_133430.wav), [`samples/FT4/000000_000002.wav`](../samples/FT4/000000_000002.wav), and `digi-dx`'s `tests/fixtures/191111_110200.wav`, plus additional quiet, crowded, and fading recordings).
  - Deterministic `ft8sim` / `ft4sim` synthetic suites: single-signal AWGN sweeps (0 dB to -24 dB in 1 dB steps), crowded overlapping multi-signal slots (20+ dB dynamic range), frequency/DT offsets, truncated/late-TX slots, and pure-noise slots.
- Capture a machine-readable **Baseline `jt9` Scorecard** (decode counts, SNR/DT/AF, AP tags, false-positive rate, and wall-clock runtime at depths `-d 1`, `-d 2`, `-d 3`).

**Testing vs. `jt9`:**
- Confirm `ftx-oracle` reproducibly runs `jt9 -8` and `jt9 -5` across the golden corpus and emits deterministic baseline scorecards.

**Completion criteria:**
- All WSJT-X oracle executables build and pass smoke checks.
- Golden corpus and `jt9` baseline scorecard are generated and committed on the dev branch.
- `ftx-core`, `ftx-modem`, and `ftx-oracle` compile cleanly with stub APIs and GPLv3 licensing metadata in place.

---

### Stage 1 — 77-Bit Message Codec, FEC Encoder & GFSK TX Modulator (FT8 + FT4)

**What we develop:**
- Direct Rust port of WSJT-X's 77-bit message packing/unpacking and callsign hash table ([`lib/77bit/packjt77.f90`](../lib/77bit/packjt77.f90)) covering standard QSOs, directed CQs, compound/nonstandard callsigns, grids, signal reports, contests, telemetry, and free text.
- CRC-14 and LDPC(174,91) encoder shared by FT8 and FT4.
- FT8 (79-symbol, 8-GFSK) and FT4 (105-symbol, 4-GFSK) tone mapping and continuous-phase GFSK waveform generators supporting arbitrary sample rates (12 kHz / 48 kHz) and **UTC-aligned late-TX start offsets** with click-free fade-in (matching [`Modulator/Modulator.cpp`](../Modulator/Modulator.cpp)).

**Testing vs. `jt9` / WSJT-X:**
- **Differential codec fuzzing:** Fuzz thousands of messages across every 77-bit type (`i3`/`n3`) and compare packed 77-bit payloads, unpacked text, and channel tone sequences against `ft8code` and `ft4code`.
- **TX-to-`jt9` loopback:** Feed Rust-synthesized FT8 and FT4 `.wav` files—nominal starts, late starts (1 s–5 s for FT8 and FT4 equivalents), and mid-slot hot-swaps—directly into `jt9 -8` and `jt9 -5`.

**Completion criteria:**
- **100% bit-exact parity** on 77-bit payloads and channel tones against `ft8code` and `ft4code` across all message types.
- **100% round-trip pack → unpack fidelity** on valid messages (including hash-table resolution workflows).
- **100% decode rate by `jt9`** on Rust-synthesized FT8 and FT4 waveforms (including late-TX and hot-swap cases), with recovered AF within ±2 Hz and DT within ±0.1 s.

---

### Stage 2 — Core FT8 Decoder MVP (Sync, Soft LLRs, BP & Subtraction)

**What we develop:**
- Direct Rust port of WSJT-X's core FT8 receive pipeline: spectrogram & candidate search ([`lib/ft8/sync8.f90`](../lib/ft8/sync8.f90)), baseband downsampling ([`lib/ft8/ft8_downsample.f90`](../lib/ft8/ft8_downsample.f90)), fine Costas sync ([`lib/ft8/sync8d.f90`](../lib/ft8/sync8d.f90)), symbol FFT & soft-bit LLRs ([`lib/ft8/ft8b.f90`](../lib/ft8/ft8b.f90)), Belief Propagation LDPC(174,91) decoding ([`lib/ft8/bpdecode174_91.f90`](../lib/ft8/bpdecode174_91.f90)), CRC-14 check, and multi-pass signal subtraction ([`lib/ft8/subtractft8.f90`](../lib/ft8/subtractft8.f90)).

**Testing vs. `jt9` & Fortran Internals:**
- **Stage-by-stage numerical verification (eliminates silent off-by-one sensitivity loss):** Compare intermediate arrays (`cd0`, `sync`/`ibest`/`f1`, `s8`/`cs`, LLR vectors, and post-subtraction residual `dd`) directly against WSJT-X Fortran on reference `.wav` inputs to verify floating-point agreement before FEC can mask indexing bugs.
- **End-to-end differential scoring vs. `jt9 -8 -d 1` / `-d 2`:** Compare decode sets, SNR, DT, and AF across synthetic AWGN sweeps (0 dB to -18 dB) and the real-world FT8 corpus.
- **Late-start & partial-slot tests:** Verify decoding of 5 s late-start FT8 waveforms and 11.8 s / 13.5 s partial-slot audio.

**Completion criteria:**
- Intermediate pipeline arrays match WSJT-X Fortran within floating-point tolerance (`≤1e-4` relative error) on reference fixtures.
- **≥99% decode rate** on single-signal synthetic FT8 from 0 dB to -15 dB.
- **≥90% of `jt9 -8 -d 1` decode count** on the real-world FT8 `.wav` corpus.
- Measurement agreement with `jt9`: AF within ±3 Hz, DT within ±0.15 s, SNR within ±2 dB.
- **0% false decodes** on pure-noise slots; passes 5 s late-start and mid-slot hot-swap tests.

---

### Stage 3 — FT8 Weak-Signal Parity (OSD, Coherent Multi-Symbol LLRs, AP & Deep Subtraction)

**What we develop:**
- Direct Rust port of [`lib/ft8/osd174_91.f90`](../lib/ft8/osd174_91.f90) (Ordered Statistics Decoding over GF(2)) to recover weak frames where BP fails.
- Multi-symbol coherent bit metrics (1-, 2-, and 3-symbol complex sums) and metric normalization.
- A priori (`AP`) decoding hypotheses (`CQ`, `MyCall`, `MyCall + DxCall`, `RRR`/`73`/`RR73`) driven by optional QSO context, emitting WSJT-X-compatible AP/uncertainty tags (`None`, `a1`–`a6`, `? a*`).
- Multi-pass subtraction and candidate-thinning parity for dense, high-dynamic-range bands.

**Testing vs. `jt9` & Fortran Internals:**
- **OSD unit oracle:** Feed identical LLR arrays into Fortran `osd174_91` and Rust OSD and assert identical decoded codewords and distance metrics.
- **AWGN sensitivity cliff sweep:** Run Monte Carlo trials from -15 dB down to -24 dB in 1 dB steps and compare the 50% decode threshold curve directly against `jt9 -8 -d 3`.
- **Crowded-band & real-world diff:** Run `ftx-oracle` across the entire real-world FT8 corpus and overlapping multi-signal synthetic bands.
- **False-positive sweep:** Run hundreds of pure AWGN noise slots (with and without AP enabled) to verify OSD/AP acceptance thresholds.

**Completion criteria:**
- **100% agreement** on OSD decoded codewords given identical input LLRs on reference test vectors.
- **≥95–98% of `jt9 -8 -d 3` total decode count** across the full real-world FT8 `.wav` corpus.
- Synthetic AWGN 50% decode threshold within **≤0.5–1.0 dB** of `jt9 -8 -d 3`.
- Crowded-band overlapping-signal decode count within **≥95%** of `jt9 -8 -d 3`.
- False-decode rate on pure noise **≤ `jt9 -8 -d 3`** (target <0.1% without AP; uncertain AP decodes accurately tagged).
- Measurement agreement with `jt9`: AF within ±2 Hz, DT within ±0.1 s, SNR within ±2 dB.

---

### Stage 4 — FT4 Decoder Parity

**What we develop:**
- Direct Rust port of WSJT-X's FT4 receive pipeline ([`lib/ft4/`](../lib/ft4/) + [`lib/ft4_decode.f90`](../lib/ft4_decode.f90)), reusing Stage 1–3 77-bit codec, CRC-14, and LDPC(174,91) BP + OSD + AP engine:
  - 7.5 s slot spectrogram and 4-Costas-array candidate detection (`getcandidates4`).
  - FT4 downsampling (`ft4_downsample`), fine sync (`sync4d`), coherent multi-symbol bit metrics (`get_ft4_bitmetrics`), and FT4 signal subtraction (`subtractft4`).

**Testing vs. `jt9` & Fortran Internals:**
- Stage-by-stage intermediate verification against WSJT-X FT4 Fortran on reference `.wav` inputs.
- End-to-end differential scoring against `jt9 -5` (depths 1–3) on `ft4sim` / `ft4sim_mult` AWGN sweeps (0 dB to -21 dB), crowded-band fixtures, and real-world FT4 recordings (including [`samples/FT4/000000_000002.wav`](../samples/FT4/000000_000002.wav)).
- Pure-noise false-positive sweep and FT4 loopback tests (nominal + late-start TX).

**Completion criteria:**
- **≥95% of `jt9 -5` decode count** across crowded synthetic and real-world FT4 corpora.
- Synthetic AWGN 50% decode threshold within **≤0.5–1.0 dB** of `jt9 -5`.
- False-decode rate on pure noise **≤ `jt9 -5`**.
- 100% loopback decode on Rust-encoded FT4 signals (nominal and late-start).

---

### Stage 5 — `ftx-modem` Binary (Standalone CLI + Warm IPC Worker), Parallelism & Telemetry

**What we develop:**
- **Engine performance & concurrency (`ftx-core`):**
  - Warm reusable FFT plans and scratch buffers held across slots (zero cold-start stall).
  - `rayon`-based parallel candidate decoding, optional RX-frequency/QSO-first priority ordering, streaming decode callback emission, and cooperative mid-pass cancellation (`AtomicBool`).
  - Optional structured telemetry payload per decode (FEC method BP vs. OSD order, AP type, sync score, subtraction pass, Costas frequency drift Hz/cycle, and symbol amplitude/QSB envelope).
- **Standalone executable (`ftx-modem`) with two clean modes:**
  1. **Standalone CLI mode:** Decode FT8/FT4 `.wav` files to text/JSON and encode FT8/FT4 messages to `.wav`/PCM directly from the command line (useful completely outside `digi-dx`).
  2. **Persistent warm IPC worker mode (`ftx-modem --worker`):** Long-lived subprocess speaking a versioned stdio/pipe framing protocol—accepts raw 12 kHz PCM buffers + mode/depth/QSO-context commands, streams decode JSON events as each candidate finishes, honors in-flight cancel commands, and serves FT8/FT4 TX waveform encode requests.

**Testing vs. `jt9`:**
- **End-to-end warm-IPC 3-pass benchmark:** Drive `ftx-modem --worker` over its real IPC pipe across `digi-dx`'s 3-pass cadence (FT8: 11.8 s / 13.5 s / 14.8 s; plus FT4 equivalents) and compare time-to-first-decode, per-pass wall-clock duration, and reconciled decode counts against `jt9` (cold and warm wisdom).
- **Standalone CLI parity tests:** Verify `ftx-modem` CLI output on golden `.wav` files matches `ftx-core` library results identically.
- **IPC resilience, cancellation & soak testing:** Verify mid-pass cancellation (<50 ms), malformed-frame handling, clean EOF shutdown, and zero memory/thread growth over a 1,000+ slot continuous worker soak test.

**Completion criteria:**
- End-to-end latency **through the warm IPC boundary** decisively faster than `jt9` subprocess on crowded bands (targets: IPC transfer overhead **<2 ms**, Pass 1 **<150 ms**, Pass 2 **<250 ms**, Pass 3 deep **<500 ms**, zero cold-start stall).
- First decode streams over IPC before pass completion on multi-signal slots.
- Mid-pass cancellation halts cleanly in **<50 ms**; 1,000-slot soak test completes with flat memory and zero leaked threads.
- **Zero sensitivity regression** against Stage 3 (FT8) and Stage 4 (FT4) parity scorecards.

---

### Stage 6 — Lift-and-Shift to Independent GPLv3 Repository & Release Packaging

**What we develop:**
- **Repository extraction:** Move `ftx-core` and `ftx-modem` out of `wsjtx` into a **new, independent GPLv3 GitHub repository** (e.g., `mpmbq2/ftx-modem`) with:
  - GPLv3 `LICENSE`, upstream WSJT-X copyright/attribution notices, README, CLI usage docs, and versioned IPC protocol specification.
  - Self-contained unit tests and a compact fixture regression suite (representative FT8/FT4 `.wav` files + message vectors) so the independent repo tests itself with zero `wsjtx`/`jt9`/`gfortran`/`fftw3` dependency.
- **Cross-platform CI & Release artifacts:** Add GitHub Actions CI + tagged release builds matching `digi-dx`'s target matrix (`linux/x86_64`, `linux/aarch64`, `macos/arm64`, and `macos/x86_64` tarballs with `SHA256SUMS`, plus `cargo install` support).
- **Clean up `wsjtx`:** Ensure `wsjtx` `master` has zero stray changes (optionally archive/retain the `feat/ftx-modem` oracle branch for future parity re-benchmarks against newer WSJT-X releases).

**Testing:**
- Clean-room build and full `cargo test` in the new repository on a Rust-only environment (no `wsjtx`, `jt9`, `gfortran`, or `fftw3`).
- Run one final `ftx-oracle` verification pass in `wsjtx` against the tagged release binary of the independent repo to confirm zero drift.

**Completion criteria:**
- Independent GPLv3 repo builds, passes CI/tests, and publishes versioned cross-platform release binaries + `SHA256SUMS`.
- Final `ftx-oracle` run matches Stage 5 parity & latency scorecards identically.
- `wsjtx` `master` remains clean.

---

### Stage 7 — `digi-dx` Integration as Bundled, Selectable 3rd Backend (MIT Preserved)

**What we develop:**
- **Dependency Provisioning & Bundling (MIT + GPLv3 aggregate):**
  - Update `digi-dx`'s `scripts/install.sh` and `docs/deployment-and-deps.md` to install/bundle a pinned release of the independent `ftx-modem` binary (e.g., `/usr/local/lib/digi-dx/ftx-modem`, verified by SHA256 + native execution probe, with `--no-ftx-modem` escape hatch and `DIGI_DX_FTX_MODEM_PATH` env override — mirroring `jt9`/`rigctld`).
  - Ship `ftx-modem`'s GPLv3 license notice and upstream source repository link alongside the bundled binary while keeping `digi-dx`'s own codebase and `Cargo.toml` **100% MIT** (zero GPL crates linked).
- **Backend #3 Warm IPC Adapter (`src/modem/ftx_modem.rs`):**
  - Manage the persistent `ftx-modem --worker` child process (auto-start, protocol version handshake, health/restart supervision on unexpected exit, clean shutdown).
  - Bridge streamed IPC decodes and `CancellationToken` directly into `digi-dx`'s `mpsc::UnboundedSender<Ft8Decode>` and `SlotDecodeReconciler` (`src/qso/reconciler.rs`).
  - Support both **FT8** and **FT4** RX decoding and TX waveform modulation (including late-start TX offsets for both modes).
- **Selectable 3-Backend Coordinator & UI:**
  - Generalize `HybridFt8Decoder` (`src/modem/mod.rs`) to support explicit user selection across all 3 backends:
    - `ftx_modem` (bundled pure-Rust GPLv3 worker — FT8 + FT4)
    - `jt9` (external WSJT-X subprocess)
    - `ft8_lib` (built-in MIT C fallback)
    - `auto` (priority fallback across available backends)
  - Wire backend selection end-to-end: config/env (`DIGI_DX_DECODER_BACKEND`), SQLite preference persistence, preflight checks (`src/preflight/mod.rs`) reporting all 3 backends' status/versions, REST/WebSocket API, and Web UI backend selector + per-decode backend display.

**Testing:**
- **License boundary audit:** Verify via `cargo metadata` / `Cargo.lock` that `digi-dx` links zero GPLv3 crates and remains strictly MIT.
- **Integration suite expansion:** Extend `digi-dx`'s `tests/modem_integration.rs`, `tests/api_integration.rs`, `tests/qso_integration.rs`, and UI smoke checks to cover all 3 selectable backends:
  - Offline `.wav` fixtures (`ftx_modem` vs. `jt9` vs. `ft8_lib`).
  - 3-pass streaming + `SlotDecodeReconciler` confidence-upgrade behavior.
  - FT8 & FT4 loopback RX/TX (including 5 s late-start TX and mid-slot hot-swap).
  - Worker crash-recovery (kill `ftx-modem` mid-session, confirm clean respawn or fallback) and live backend switching in UI/API.
- **Side-by-side A/B evaluation:** Compare decode counts and decode-to-UI latency across all 3 backends on recorded multi-band FT8 and FT4 sessions.

**Completion criteria:**
- `digi-dx` remains 100% MIT-licensed with `ftx-modem` installed/bundled purely as an independent external executable dependency.
- User can select `ftx_modem`, `jt9`, `ft8_lib`, or `auto` from `digi-dx`'s UI/config and see active status in preflight and the console.
- All existing and new `digi-dx` Rust and UI tests pass with zero regressions to `jt9` or `ft8_lib`.
- A/B validation confirms `ftx_modem` matches `jt9` parity (≥95% decodes across FT8 and FT4), outperforms `ft8_lib` in sensitivity, and delivers markedly lower decode-to-UI latency than `jt9`.

---

## 4. Remaining Open Decisions Before Stage 0

1. **Independent repo & binary name:** The roadmap uses `ftx-modem` (e.g., `mpmbq2/ftx-modem`) as a placeholder—do you have a preferred name for the standalone GPLv3 project?
2. **TX modulation scope:** Confirm whether `ftx-modem` should serve **both RX and TX** for FT8 and FT4 over the warm IPC worker (unlocking full FT4 TX in `digi-dx`, since `ft8_lib` is currently FT8-only), or RX-only first.
3. **`auto` backend priority order:** Once Stage 7 ships, should `auto` prefer `ftx_modem` → `jt9` → `ft8_lib`, or keep `jt9` first until proven on-air?
4. **Rich telemetry UI timing:** Should Stage 7 wire only core decode fields + AP metadata into `digi-dx`'s UI first (deferring frequency-drift / QSB / FEC-path display to a follow-up UI pass), or surface them right away?
