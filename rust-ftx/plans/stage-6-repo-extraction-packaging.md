# Stage 6 Plan — Lift-and-Shift Extraction (`mpmbq2/ftxr-modem`) & Release Packaging

## 1. Header & Scope

- **Stage:** 6 of 7 — Independent GPLv3 Repository Extraction, Legal/License Compliance, Cross-Platform CI/Release Packaging & Clean-Room Verification
- **Source Development Workspace:** `/home/antigravity/mpmbq2/wsjtx/rust-ftx/` (branch `feat/ftx-modem`)
- **Target Independent Repository:** `https://github.com/mpmbq2/ftxr-modem` (default branch `main`, initial release tag `v0.1.0`)
- **Upstream Read-Only Reference:** `/home/antigravity/mpmbq2/wsjtx/` (`master` untouched) and `/home/antigravity/mpmbq2/digi-dx/` (read-only; never modified during Stage 6)

### Prerequisites (Must Be Complete Before Starting Stage 6)
1. **Stage 0–4 complete:** `crates/ftxr-core` achieves full FT8 (`jt9 -8 -d 3`, AP `a1`–`a6`) and FT4 (`jt9 -5 -d 3`, AP `a1`–`a6`) RX/TX parity with committed scorecards in `wsjtx/rust-ftx/`.
2. **Stage 5 complete:** `crates/ftxr-modem` passes standalone CLI parity tests, warm `--worker` multiplexed IPC tests, `<50 ms` cancellation, 1,000-slot soak test, and dev-host 3-pass latency gates (IPC overhead `<2 ms`, Pass 1 `<150 ms`, Pass 2 `<250 ms`, Pass 3 `<500 ms`), with `docs/ipc-protocol.md` finalized.
3. **Zero temporary WSJT-X Fortran hooks remaining:** Any temporary Fortran debug/dump instrumentation introduced during Stages 2–4 inside `wsjtx/lib/` has been reverted.

### Deliverables
1. **Independent GPLv3 GitHub repository `mpmbq2/ftxr-modem`** containing `crates/ftxr-core` and `crates/ftxr-modem` (with `tools/ftxr-oracle` intentionally excluded), verbatim FSF GPLv3 `LICENSE`, authoritative WSJT-X attribution `NOTICE`, `README.md`, `CHANGELOG.md`, `docs/ipc-protocol.md`, `deny.toml`, and a compact self-contained `tests/fixtures/` regression suite (`< 1.5 MiB`).
2. **Automated GPLv3 compliance & clean-room audit suite** (`deny.toml`, license/dependency verification script, Numerical Recipes exclusion check, crate-level `LICENSE`/`NOTICE` packaging verification, and `ftxr-modem --version` / `--license` legal notice output).
3. **GitHub Actions CI (`.github/workflows/ci.yml`)** enforcing `rustfmt`, `clippy -D warnings`, `rustdoc -D warnings`, `cargo-deny` + pure-Rust audit, multi-OS/arch tests (`linux-x86_64`, `linux-aarch64`, `darwin-aarch64`, `darwin-x86_64`), MSRV check, and a clean-room Linux container test with zero WSJT-X/Fortran/FFTW/Qt packages.
4. **GitHub Actions Release workflow (`.github/workflows/release.yml`)** triggered on `v*.*.*` tags, enforcing tag-to-`Cargo.toml` parity and an all-platforms-ready gate, producing 4 platform tarballs (`ftxr-modem-<tag>-<linux|darwin>-<x86_64|aarch64>.tar.gz`), a GPLv3 §6(d) Corresponding Source archive (`ftxr-modem-<tag>-src.tar.gz`), and `SHA256SUMS` strictly matching `digi-dx/scripts/install.sh`, plus verified `cargo install` support.
5. **Final `ftxr-oracle` parity & latency verification** inside `wsjtx` executed against the downloaded tagged `ftxr-modem` release binary, and verified `wsjtx` branch hygiene (`master` untouched, `feat/ftx-modem` archived cleanly).

### Exit Gate Checklist
- [ ] `mpmbq2/ftxr-modem` builds and passes `cargo test --workspace --locked` in a clean-room Rust-only container (no `wsjtx`, `jt9`, `gfortran`, `fftw3`, `Qt`, or `digi-dx`).
- [ ] `cargo deny check` and `cargo metadata` license audit confirm 100% GPLv3-compatible crates, zero `*-sys`/`cc` C dependencies, and zero Numerical Recipes code (`bessi0`, `indexx.f90`, `shell.f90`).
- [ ] `NOTICE`, `LICENSE`, `README.md`, and source headers satisfy GPLv3 §4–§6 and `wsjtx/TRADEMARK.md` (distinct name, independent SemVer, explicit "based on WSJT-X" attribution and non-endorsement statement).
- [ ] GitHub Actions `ci.yml` is green on `main` across Linux (`x86_64`, `aarch64`), macOS (`aarch64`, `x86_64`), MSRV, lint, license audit, and clean-room container jobs.
- [ ] Tagging `v0.1.0` produces all 4 release tarballs + `ftxr-modem-v0.1.0-src.tar.gz` + `SHA256SUMS`; every tarball extracts `ftxr-modem`, `LICENSE`, `NOTICE`, `README.md` flat at archive root and passes `grep "  $tarball$" SHA256SUMS | sha256sum -c -` (Linux) and `shasum -a 256 -c -` (macOS).
- [ ] `cargo install --locked --git https://github.com/mpmbq2/ftxr-modem --tag v0.1.0 ftxr-modem` succeeds cleanly from a cold environment.
- [ ] Final `ftxr-oracle` regression pass in `wsjtx` pointing at the downloaded tagged `ftxr-modem` release binary matches Stage 5 FT8/FT4 decode parity identically and stays within Stage 5 latency targets (and `≤ 10%` of Stage 5 glibc latency if musl static binary is used).
- [ ] `git -C wsjtx diff origin/master -- ':!rust-ftx'` is completely empty; `origin/master` is untouched; `feat/ftx-modem` retains `tools/ftxr-oracle` + final Stage 6 scorecard as the archival oracle branch.

---

## 2. Source Audit & Technical Reference

All facts, file paths, versions, legal notices, and packaging contracts below were verified directly against the repositories on disk.

### 2.1 Upstream WSJT-X Copyright, License & Trademark Audit
| Source File | Verified Lines / Values | Stage 6 Implication |
| :--- | :--- | :--- |
| `wsjtx/CMakeLists.txt` | L63–66: `project(wsjtx VERSION 3.0.2.0)`; L118–121: `PROJECT_VENDOR = "WSJT Development Group"`, `PROJECT_COPYRIGHT = "Copyright (C) 2001-2026 by Joe Taylor, K1JT"`, `PROJECT_HOMEPAGE = https://wsjt.sourceforge.io/wsjtx.html` | Authoritative upstream version (`3.0.2.0`) and copyright year span (`2001–2026`) for `NOTICE` (supersedes stale `2001 - 2023` / `2.6.1` text in `wsjtx/README:14-16`). |
| `wsjtx/widgets/about.cpp` | L22–33: `© 2001-2026 by Joe Taylor, K1JT, Bill Somerville, G4WJS, Steve Franke, K9AN, Nico Palermo, IV3NWV, Uwe Risse, DG2YCB, Brian Moran, N9ADG, Roger Rehr, W3SZ, John Nelson, G4KLA, Charlie Suckling, DL3WDG, and Terrell Deppe, KJ5HST`; `WSJT-X is licensed under the terms of Version 3 of the GNU General Public License (GPL)` | Authoritative contributor roster for `NOTICE` (explicitly referenced by `wsjtx/AUTHORS:3-4`). Note exact license statement: Version 3 without "or later". |
| `wsjtx/AUTHORS` & `wsjtx/README` | `AUTHORS:1`: `Joe Taylor, K1JT <k1jt@arrl.net>`; `README:88-93`: GPLv3 license + SourceForge repo `https://sourceforge.net/p/wsjt/wsjtx/ci/master/tree/` | Include upstream SourceForge repository URL alongside homepage in `NOTICE`. |
| `wsjtx/COPYING` | L1–152: GPLv3 (29 June 2007), but formatted with curly quotes (`“`/`”`), single-line wrapped paragraphs, and ends at L151 (`END OF TERMS AND CONDITIONS`) without the FSF appendix | **Gotcha:** Do **not** blindly copy `wsjtx/COPYING` verbatim into `mpmbq2/ftxr-modem/LICENSE`; GitHub's `licensee` detector can misclassify re-wrapped curly-quoted variants. Use the canonical FSF plain-text `https://www.gnu.org/licenses/gpl-3.0.txt` as `LICENSE`. |
| `wsjtx/README.md` & `wsjtx/debian/copyright.in` | `README.md:22`: `GNU General Public License v3.0`; `debian/copyright.in:8`: `License: GPL-3+` | **Gotcha:** Because `README.md`, `README`, and `about.cpp` specify GPLv3 *without* an explicit "or any later version" clause, GPLv3 §14 (`COPYING:137`) makes **`GPL-3.0-only`** the legally airtight SPDX license for `ftxr-modem` (compatible with MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, BSL-1.0, CC0-1.0, Unlicense). |
| `wsjtx/TRADEMARK.md` | L3–25: `WSJT™`, `WSJT-X™`, `MAP65™`, `QMAP™` are trademarks of the WSJT Development Team. Explicitly **allows** describing a derivative as `"based on WSJT-X"` or `"based on WSJT-X code"`; **forbids** using `"WSJT"`/`"WSJT-X"` in the primary title or adopting WSJT-X version numbering (e.g., `3.0.2`). | `ftxr-modem` (`mpmbq2/ftxr-modem`), independent SemVer (`0.1.0`), explicit `"based on WSJT-X algorithms"` attribution, and non-endorsement notice in `NOTICE` and `README.md` ensure full trademark compliance. |

### 2.2 Non-GPL / Third-Party Code Exclusion Audit in WSJT-X `lib/`
A repository-wide audit of `wsjtx/lib/ft8/`, `wsjtx/lib/ft4/`, `wsjtx/lib/77bit/`, and shared helpers uncovered four specific routines requiring explicit clean-room verification during extraction:
1. **`wsjtx/lib/ft8/ft8b.f90:509-530` (`function bessi0(x)`):** Commented `! From Numerical Recipes` on L510, with zero callers anywhere in `wsjtx/lib/`. **Must be 100% absent** from `ftxr-core` (checked automatically in CI).
2. **`wsjtx/lib/indexx.f90` (`M=7, NSTACK=50` quicksort index routine, called by `osd174_91.f90:76` and `sync8.f90:100,111,154`) & `wsjtx/lib/shell.f90` (called via `pctile.f90` by `baseline.f90:28` and `ft4_baseline.f90:29`):** Classic textbook/Numerical Recipes-style Fortran sorts. Rust implementation **must** use `std` sorting (`slice::sort_by`, `sort_unstable_by`, `select_nth_unstable_by`) rather than ported Fortran sort loops.
3. **`wsjtx/lib/polyfit.f90` & `wsjtx/lib/determ.f90` (called by `baseline.f90:39` and `ft4_baseline.f90:40`):** Bevington-style weighted polynomial least squares. Rust `src/dsp/baseline.rs` uses clean-room ordinary least squares normal equations (`5×5` Gaussian elimination with partial pivoting).
4. **`wsjtx/lib/crc14.cpp:1-32`:** Wraps `boost::augmented_crc<14, 0x2757>`. Rust `src/protocol/crc14.rs` implements CRC-14 natively in pure Rust with zero Boost/C++ dependencies.

### 2.3 `digi-dx` Installer & CI Packaging Contract (Read-Only Reference)
| Source File | Verified Lines / Contract | Exact `ftxr-modem` Release Requirement |
| :--- | :--- | :--- |
| `digi-dx/scripts/install.sh` | L172–204 (`detect_platform`): `Linux → linux`, `Darwin → darwin`; `x86_64\|amd64 → x86_64`, `aarch64\|arm64 → aarch64`; rejects 32-bit `armv7*`/`armhf`/`i386`/`i686` | Release asset matrix **must** use the exact 4 normalized slugs: `linux-x86_64`, `linux-aarch64`, `darwin-x86_64`, `darwin-aarch64`. |
| `digi-dx/scripts/install.sh` | L410–420 (`provision_binary`): `https://github.com/${REPO}/releases/download/${ver_tag}/${name}-${ver_tag}-${OS_NAME}-${ARCH_NORM}.tar.gz` and `.../SHA256SUMS` | Release tarball names **must** be `ftxr-modem-<tag>-<os>-<arch>.tar.gz` (e.g., `ftxr-modem-v0.1.0-linux-aarch64.tar.gz`) with `v`-prefixed `<tag>` matching the GitHub Release tag verbatim. |
| `digi-dx/scripts/install.sh` | L435–441: `grep "  $tarball\$" SHA256SUMS \| sha256sum -c -` (Linux) and `grep "  $tarball\$" SHA256SUMS \| shasum -a 256 -c -` (macOS) | **Critical gotcha:** `SHA256SUMS` **must** use exact two-space text format `<64-lowercase-hex>  <bare-filename>\n` — NO `*` binary flag (`sha256sum -b`), NO `./` prefix, and NO `dist/` subdirectory prefix. |
| `digi-dx/scripts/install.sh` | L443–445: `tar -xzf "$tmp_dir/$tarball" -C "$tmp_dir"` followed by `[[ -f "$tmp_dir/<binary>" ]]` | **Critical gotcha:** Tarball layout **must** be flat at archive root (`ftxr-modem`, `LICENSE`, `NOTICE`, `README.md`), NOT nested inside a versioned subfolder. |
| `digi-dx/scripts/install.sh` | L375–408 (`--from-source`): requires `cargo (Rust >= 1.82)` | Document MSRV alignment (`1.85` if `Cargo.lock` v4 is committed; `1.82` if lockfile v3 is preserved — see §3.2) and support `cargo install --locked --git ... --tag <tag> ftxr-modem` for Stage 7 `--from-source` fallback. |
| `digi-dx/docs/deployment-and-deps.md` | L11, L34–42, L48: Primary targets are Debian/Ubuntu/Raspberry Pi OS 64-bit (`glibc 2.36` on Bookworm, `glibc 2.35` on Ubuntu 22.04), Fedora, Arch, and macOS (`arm64`/`x86_64`). `jt9` multi-arch trap requires native arch check + execution probe. | Linux release binaries **must not** link `GLIBC_2.38`/`2.39` symbols from `ubuntu-24.04`! Use static `musl` builds (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`) with a latency regression gate (and `ubuntu:22.04` glibc 2.35 container fallback if needed). |
| `digi-dx/.github/workflows/ci.yml` & `wsjtx/.github/workflows/*.yml` | `digi-dx/ci.yml`: `permissions: contents: read`, `concurrency` cancel-in-progress, `dtolnay/rust-toolchain`, `Swatinem/rust-cache@v2`. `wsjtx/build-linux.yml:33`: `ubuntu-24.04` + `ubuntu-24.04-arm`. `wsjtx/build-macos.yml:56`: `macos-15` (`arm64`, min `11.0`) + `macos-15-intel` (`x86_64`). `wsjtx/release.yml:52-66,204-241`: tag-version parity gate + all-platforms-ready gate + `git archive` source tarball. | Adopt identical proven GitHub runner labels (`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-15`, `macos-15-intel`) and release safety gates in `mpmbq2/ftxr-modem`. |

### 2.4 Existing Fixtures & Branch State Verified on Disk
- **Committed WSJT-X samples:**
  - `wsjtx/samples/FT8/210703_133430.wav` — `360,246` bytes (`352 KiB`), GPLv3.
  - `wsjtx/samples/FT4/000000_000002.wav` — `145,196` bytes (`142 KiB`), GPLv3.
- **Reference `digi-dx` fixture:**
  - `digi-dx/tests/fixtures/191111_110200.wav` — `360,246` bytes (`352 KiB`), originally from MIT-licensed `kgoba/ft8_lib` (`vendor/ft8_lib/LICENSE` is MIT, GPLv3-compatible; if copied into `mpmbq2/ftxr-modem/tests/fixtures/`, credit `kgoba/ft8_lib` MIT provenance in `tests/fixtures/README.md` and `NOTICE`).
- **Current `wsjtx` git refs:**
  - `refs/heads/master` (on `origin`): `967c85a6119110788b0df3782cf796485b2cee14` (`ci(release): warm RC1 Intel Qt cache`).
  - `refs/heads/feat/ftx-modem`: branched off `967c85a` (`3e5e0b9` roadmap commit + Stage 0–5 commits).

---

## 3. Architecture & Concrete Rust / Packaging Design

### 3.1 Target Repository Layout (`mpmbq2/ftxr-modem`)

Lifts `crates/ftxr-core` and `crates/ftxr-modem` out of `wsjtx/rust-ftx/`, drops `tools/ftxr-oracle` completely, and adds standalone licensing, CI/release automation, documentation, and a self-contained regression suite (`< 1.5 MiB` total fixtures):

```text
ftxr-modem/
├── .github/
│   └── workflows/
│       ├── ci.yml                     # fmt, clippy, rustdoc, cargo-deny, 4-platform tests, MSRV, clean-room container
│       └── release.yml                # v*.*.* tag parity gate, 4-target builds, src tarball, SHA256SUMS, GH Release
├── Cargo.toml                         # Workspace root (members: crates/ftxr-core, crates/ftxr-modem)
├── Cargo.lock                         # Committed for reproducible --locked builds and cargo install --locked
├── deny.toml                          # cargo-deny v2 policy: GPLv3-compatible license allowlist + zero C/-sys ban
├── LICENSE                            # Canonical FSF plain-text GPLv3 (gpl-3.0.txt)
├── NOTICE                             # Upstream WSJT-X (K1JT/G4WJS/K9AN/team) copyright, trademark & §6 source notice
├── README.md                          # Standalone CLI + worker overview, install, GPLv3 notice, WSJT-X attribution
├── CHANGELOG.md                       # Keep a Changelog format (v0.1.0 initial release entry)
├── rustfmt.toml                       # Formatting settings (if customized; otherwise standard rustfmt defaults)
├── docs/
│   └── ipc-protocol.md                # Versioned multiplexed stdio IPC contract (from Stage 5)
├── scripts/
│   └── verify-compliance.sh           # Automated GPLv3/clean-room/Numerical-Recipes/crate-license audit script
├── crates/
│   ├── ftxr-core/
│   │   ├── Cargo.toml                 # Pure-Rust GPL-3.0-only lib crate (bundles crate-local LICENSE + NOTICE)
│   │   ├── LICENSE                    # Byte-identical copy of root LICENSE (for `cargo package` / crates.io compliance)
│   │   ├── NOTICE                     # Byte-identical copy of root NOTICE
│   │   └── src/
│   │       ├── lib.rs                 # Mode, DecodeConfig, QsoContext, ApTag, DecodedMessage, DecodeTelemetry, ModulateConfig, FtxrError, DecoderEngine
│   │       ├── protocol/              # mod.rs, types.rs, pack77.rs, unpack77.rs, hashtable.rs, crc14.rs, ldpc.rs, rvec.rs
│   │       ├── modulate/              # mod.rs, gfsk.rs, ft8.rs, ft4.rs
│   │       ├── dsp/                   # mod.rs, fft.rs, window.rs, baseline.rs, twkfreq.rs, downsample.rs
│   │       ├── fec/                   # mod.rs, bp.rs, osd.rs, decode174_91.rs
│   │       ├── ft8/                   # mod.rs, params.rs, sync8.rs, sync8d.rs, ft8b.rs, ap.rs, subtract.rs, decoder.rs
│   │       ├── ft4/                   # mod.rs, params.rs, sync4d.rs, candidates.rs, bitmetrics.rs, ap.rs, subtract.rs, decoder.rs
│   │       └── telemetry.rs
│   └── ftxr-modem/
│       ├── Cargo.toml                 # GPL-3.0-only binary crate (`ftxr-modem`), depends on ftxr-core via path + version
│       ├── LICENSE                    # Byte-identical copy of root LICENSE
│       ├── NOTICE                     # Byte-identical copy of root NOTICE
│       ├── src/
│       │   ├── main.rs                # CLI entrypoint + --worker dispatch + --version / --license notices
│       │   ├── cli.rs                 # clap CLI definitions (decode, encode, worker, license/version metadata)
│       │   ├── wav.rs                 # Pure-Rust PCM16 WAV reader/writer
│       │   ├── worker.rs              # Persistent warm multiplexed stdio IPC worker (from Stage 5)
│       │   └── legal.rs               # Embedded GPLv3 §5(d)/§6(d) short notice & source URL constants
│       └── tests/
│           ├── cli_regression.rs      # End-to-end CLI decode/encode tests against tests/fixtures/
│           ├── worker_ipc.rs          # Framed IPC handshake, streaming, cancel (<50ms), concurrent TX-during-RX
│           └── synthetic_parity.rs    # Seeded deterministic FT8/FT4 AWGN, OSD, AP a1-a6, crowded subtraction, pure noise
└── tests/
    └── fixtures/
        ├── README.md                  # Provenance & license of every fixture file
        ├── 210703_133430.wav          # 352 KiB real-world FT8 recording (from WSJT-X samples/FT8, GPLv3)
        ├── 000000_000002.wav          # 142 KiB real-world FT4 recording (from WSJT-X samples/FT4, GPLv3)
        ├── 191111_110200.wav          # 352 KiB real-world FT8 recording (from kgoba/ft8_lib, MIT)
        ├── codec_vectors.json         # Deterministic 77-bit pack/unpack/CRC14/LDPC/tone vectors (all i3/n3 types)
        ├── expected_ft8.json          # Golden FT8 decodes + SNR/DT/AF bounds for 210703_133430 & 191111_110200
        └── expected_ft4.json          # Golden FT4 decodes + SNR/DT/AF bounds for 000000_000002
```

### 3.2 Workspace & Crate Manifest Specifications

#### Root `Cargo.toml`
```toml
[workspace]
resolver = "2"
members = [
    "crates/ftxr-core",
    "crates/ftxr-modem",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
# Note: 1.85 supports Cargo.lock v4 natively while staying well below current stable (1.100+).
# Keep aligned with CI MSRV job; lower to 1.82 only if Cargo.lock is kept in v3 format.
rust-version = "1.85"
license = "GPL-3.0-only"
repository = "https://github.com/mpmbq2/ftxr-modem"
homepage = "https://github.com/mpmbq2/ftxr-modem"
authors = ["ftxr-modem Contributors"]
keywords = ["ft8", "ft4", "amateur-radio", "dsp", "modem"]
categories = ["multimedia::audio", "science", "command-line-utilities"]

[workspace.dependencies]
ftxr-core = { path = "crates/ftxr-core", version = "0.1.0" }
rustfft = "6"
rayon = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "symbols"
```

#### `crates/ftxr-core/Cargo.toml` and `crates/ftxr-modem/Cargo.toml` Packaging Rules
- Both crates inherit `[workspace.package]` fields (`version.workspace = true`, `edition.workspace = true`, `rust-version.workspace = true`, `license.workspace = true`, `repository.workspace = true`).
- `crates/ftxr-modem/Cargo.toml` declares:
  ```toml
  [[bin]]
  name = "ftxr-modem"
  path = "src/main.rs"
  ```
- Both crates specify explicit `include = ["src/**", "Cargo.toml", "LICENSE", "NOTICE", "README.md"]` (excluding `tests/fixtures/*.wav` from `crates.io` tarballs so `cargo package` stays lightweight, while guaranteeing verbatim `LICENSE` and `NOTICE` ship inside every crate package without invalid `../` paths).
- **Dependency License Table (Expected Pure-Rust Dependency Tree):**

| Direct Crate | Role | SPDX License | GPL-3.0-only Compatible? | Pure Rust (No `cc`/`*-sys`)? |
| :--- | :--- | :--- | :--- | :--- |
| `ftxr-core` (workspace) | FT8/FT4 RX+TX DSP engine | `GPL-3.0-only` | Native | Yes |
| `rustfft` (+ `realfft` if used) | Complex/real FFT plans | `MIT OR Apache-2.0` | Yes (FSF-verified) | Yes |
| `rayon` | Parallel candidate decode pool | `MIT OR Apache-2.0` | Yes | Yes |
| `serde`, `serde_json` | IPC frames & JSON CLI output | `MIT OR Apache-2.0` | Yes | Yes |
| `thiserror` | Typed `FtxrError` definitions | `MIT OR Apache-2.0` | Yes | Yes |
| `clap` | Standalone CLI argument parser | `MIT OR Apache-2.0` | Yes | Yes |
| `tracing`, `tracing-subscriber` | Structured diagnostics (`stderr` only) | `MIT` | Yes | Yes |

### 3.3 CLI Legal Notice & Metadata Hook (`crates/ftxr-modem/src/legal.rs`)
Add a minimal legal/provenance module inside `ftxr-modem` so GPLv3 §5(d)/§6(d) compliance and binary version inspection are self-contained inside the distributed executable:

```rust
// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Derived from WSJT-X (Copyright (C) 2001-2026 Joe Taylor, K1JT, et al.)

pub const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");
pub const LICENSE_SPDX: &str = env!("CARGO_PKG_LICENSE");

pub const SHORT_LEGAL_NOTICE: &str = concat!(
    "ftxr-modem ", env!("CARGO_PKG_VERSION"), "\n",
    "License: GNU General Public License v3.0 only (GPL-3.0-only)\n",
    "This program comes with ABSOLUTELY NO WARRANTY; run `ftxr-modem --license` for details.\n",
    "Based on FT8/FT4 algorithms from WSJT-X (Copyright (C) 2001-2026 Joe Taylor, K1JT, et al.).\n",
    "Corresponding Source: ", env!("CARGO_PKG_REPOSITORY"), "\n",
);

pub fn tagged_source_url() -> String {
    format!("{REPOSITORY_URL}/tree/v{PACKAGE_VERSION}")
}
```
- `ftxr-modem --version` prints `ftxr-modem <semver>` (clean single line for automated probes like Stage 7's `digi-dx` preflight).
- `ftxr-modem --license` (or `ftxr-modem license`) prints `SHORT_LEGAL_NOTICE`, the exact `tagged_source_url()`, upstream WSJT-X attribution, and the WSJT-X trademark non-endorsement notice.
- Both `--version` and `--license` exit `0` immediately without spawning worker threads or initializing FFT plans (ensuring fast `<20 ms` native execution probes in installers and preflight checks).

### 3.4 `NOTICE` File Specification (Required Content)
`NOTICE` (identical at repo root, crate roots, and inside every release tarball) must contain:
1. **Project Copyright & License Declaration (GPLv3 §4, §5(a)–(c)):**
   - `ftxr-modem — Pure-Rust FT8 & FT4 Modem Engine`
   - `Copyright (C) 2026 ftxr-modem Contributors`
   - `Modified / ported to Rust in 2026 from WSJT-X v3.0.2.0 classic FT8/FT4 routines.`
   - `Licensed under the GNU General Public License, Version 3 only (GPL-3.0-only). See LICENSE.`
2. **Upstream WSJT-X Attribution (verified from `wsjtx/CMakeLists.txt:64,118-121`, `wsjtx/widgets/about.cpp:22-33`, `wsjtx/AUTHORS`, `wsjtx/README`):**
   - `This software is derived from and ports the FT8, FT4, and 77-bit message encoding/decoding algorithms of WSJT-X (version 3.0.2.0, classic lib/ft8/, lib/ft4/, and lib/77bit/ implementations):`
   - `Copyright (C) 2001-2026 by Joe Taylor (K1JT), Bill Somerville (G4WJS), Steve Franke (K9AN), Nico Palermo (IV3NWV), Uwe Risse (DG2YCB), Brian Moran (N9ADG), Roger Rehr (W3SZ), John Nelson (G4KLA), Charlie Suckling (DL3WDG), Terrell Deppe (KJ5HST), and the WSJT Development Group.`
   - `Upstream WSJT-X Homepage: https://wsjt.sourceforge.io/wsjtx.html`
   - `Upstream WSJT-X Source: https://sourceforge.net/p/wsjt/wsjtx/ci/master/tree/`
3. **Trademark Disclaimer (verified from `wsjtx/TRADEMARK.md`):**
   - `"WSJT", "WSJT-X", "MAP65", and "QMAP" are trademarks of the WSJT Development Team. ftxr-modem is an independent project based on WSJT-X code and is neither affiliated with nor endorsed by the WSJT Development Team.`
4. **Test Fixture Attribution:**
   - `tests/fixtures/210703_133430.wav` and `tests/fixtures/000000_000002.wav`: WSJT-X sample recordings (GPLv3).
   - `tests/fixtures/191111_110200.wav`: from `kgoba/ft8_lib` (MIT License, Copyright (c) Karlis Goba).
5. **Corresponding Source Offer (GPLv3 §6(d)):**
   - `Machine-readable Corresponding Source for every official binary release of ftxr-modem is published alongside the release binaries at https://github.com/mpmbq2/ftxr-modem/releases (both as ftxr-modem-<tag>-src.tar.gz and at the corresponding git tag).`

### 3.5 `deny.toml` Specification (`cargo-deny` v2 Policy)
```toml
[graph]
targets = [
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-gnu",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
]
all-features = true

[advisories]
version = 2
yanked = "deny"

[licenses]
version = 2
confidence-threshold = 0.8
allow = [
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "BSL-1.0",
    "CC0-1.0",
    "Unlicense",
]
unused-allowed-license = "allow"

[bans]
multiple-versions = "warn"
wildcards = "deny"
highlight = "all"
deny = [
    # Enforce zero C/Fortran/FFTW/Qt/Hamlib native build dependencies
    { name = "cc", reason = "ftxr-modem must remain 100% pure Rust (zero C/C++ compilation)" },
    { name = "cmake", reason = "ftxr-modem must remain 100% pure Rust" },
    { name = "bindgen", reason = "ftxr-modem must remain 100% pure Rust" },
    { name = "pkg-config", reason = "ftxr-modem must not link host system libraries" },
    { name = "fftw-sys", reason = "use pure-Rust rustfft instead of FFTW" },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```
*(Note: If any transitive dependency of a pure-Rust crate like `libc` appears on Unix targets, `libc` is pure Rust FFI bindings without `cc`; `deny.toml` specifically blocks native C compilers/libraries and verifies all transitive licenses.)*

### 3.6 Automated Compliance Audit Script (`scripts/verify-compliance.sh`)
A self-contained Bash script run both locally and in CI (`ci.yml` + `release.yml`):
1. **License/NOTICE file parity:** Verifies `LICENSE` matches FSF GPLv3 SHA-256 and `cmp -s` confirms `crates/ftxr-core/{LICENSE,NOTICE}` and `crates/ftxr-modem/{LICENSE,NOTICE}` are byte-identical to repo root.
2. **SPDX source headers:** Verifies every `*.rs` file under `crates/ftxr-core/src` and `crates/ftxr-modem/src` carries `SPDX-License-Identifier: GPL-3.0-only` within the first 5 lines.
3. **Numerical Recipes & WSJT-X exclusion scan:** Runs `rg` across `crates/` asserting zero matches for `bessi0`, `Numerical Recipes`, `3.5156229`, `0.39894228`, `wsjtx_config`, `fftw3`, `libgfortran`, or `ftxr-oracle`.
4. **Trademark hygiene check:** Verifies `Cargo.toml` package names/descriptions and `README.md` top heading do not use `WSJT` / `WSJT-X` as the project title or adopt `3.0.x` WSJT-X version numbering.
5. **Crate metadata & license tree audit:** Parses `cargo metadata --format-version 1 --locked` (using `python3` or `jq`) to assert:
   - `ftxr-core` and `ftxr-modem` both report `license == "GPL-3.0-only"`.
   - Zero workspace or external packages depend on `*-sys` crates or `cc`/`cmake`/`pkg-config` (unless explicitly allowlisted, e.g., `windows-sys` if Windows is ever added later — none needed for Linux/macOS).
   - Every resolved package license expression evaluates to allowed GPLv3-compatible licenses.
6. **Packaging dry-run:** Runs `cargo package --locked --no-verify --allow-dirty -p ftxr-core` and `-p ftxr-modem` (inspecting archive lists via `cargo package --list`) to confirm `LICENSE` and `NOTICE` are packaged and `tests/fixtures/*.wav` are excluded from crate packages.

### 3.7 GitHub Actions CI Workflow (`.github/workflows/ci.yml`)
Mirrors `digi-dx/.github/workflows/ci.yml` conventions (`permissions: contents: read`, `concurrency` cancel-in-progress, `dtolnay/rust-toolchain`, `Swatinem/rust-cache@v2`):

| Job | Runner | Commands / Assertions |
| :--- | :--- | :--- |
| `lint-and-audit` | `ubuntu-24.04` | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`; `EmbarkStudios/cargo-deny-action@v2` (`check licenses bans advisories sources`); `bash -n scripts/verify-compliance.sh && shellcheck scripts/verify-compliance.sh && ./scripts/verify-compliance.sh`. |
| `test-matrix` | Matrix: `ubuntu-24.04` (`linux-x86_64`), `ubuntu-24.04-arm` (`linux-aarch64`), `macos-15` (`darwin-aarch64`), `macos-15-intel` (`darwin-x86_64`) | `cargo test --workspace --locked`; `cargo build --release --locked --bin ftxr-modem`; run CLI smoke probe (`--version`, `--license`, decode `tests/fixtures/210703_133430.wav` FT8 & `tests/fixtures/000000_000002.wav` FT4, `--worker` hello/shutdown). |
| `msrv` | `ubuntu-24.04` | `dtolnay/rust-toolchain` with `toolchain: "1.85.0"` (matching `workspace.package.rust-version`); `cargo check --workspace --all-targets --locked` + `cargo test --workspace --locked`. |
| `clean-room-linux` | `ubuntu-24.04` with `container: rust:1.85-slim` | Assert `(! command -v jt9 && ! command -v gfortran && ! command -v gcc)` and zero `fftw3`/`qt5`/`qt6`/`wsjtx` libs installed; run `cargo test --workspace --locked` and `cargo build --release --locked --bin ftxr-modem` cleanly from zero host C/Fortran tooling! *(Note: pure-Rust Linux target needs only the linker bundled in `rust:1.85-slim` or `rust-lld` via `-C linker=rust-lld` / musl target.)* |

### 3.8 GitHub Actions Release Workflow (`.github/workflows/release.yml`)
Triggered on `push: tags: ["v*.*.*"]` (and optional `workflow_dispatch` with `dry_run: true`). Adopts `wsjtx/.github/workflows/release.yml`'s fail-fast parity and all-platforms-ready gates alongside `digi-dx/scripts/install.sh`'s exact naming and `SHA256SUMS` contract:

#### Release Matrix
| Matrix Entry | Runner | Rust Target | Normalized Asset Slug | Linker / Portability Settings |
| :--- | :--- | :--- | :--- | :--- |
| Linux x86_64 | `ubuntu-24.04` | `x86_64-unknown-linux-musl` | `linux-x86_64` | `RUSTFLAGS="-C target-feature=+crt-static"` — 100% static ELF (`ldd` reports `not a dynamic executable`), runs on Ubuntu 20.04/22.04/24.04, RPi OS Bookworm/Trixie, Fedora, Arch, Alpine with zero glibc mismatch. |
| Linux aarch64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-musl` | `linux-aarch64` | Native ARM64 build + native execution probe (`RUSTFLAGS="-C target-feature=+crt-static"`). |
| macOS aarch64 | `macos-15` | `aarch64-apple-darwin` | `darwin-aarch64` | `MACOSX_DEPLOYMENT_TARGET=11.0`; native Apple Silicon execution probe; `otool -L` confirms zero non-system dylibs. |
| macOS x86_64 | `macos-15-intel` | `x86_64-apple-darwin` | `darwin-x86_64` | `MACOSX_DEPLOYMENT_TARGET=11.0`; native Intel execution probe; `otool -L` confirms zero non-system dylibs. *(Fallback if Intel runners retire: build on `macos-15` with `rustup target add x86_64-apple-darwin` and run smoke probe under Rosetta 2 if enabled or verify `lipo -archs` + `otool -L`.)* |

*(**Musl Allocator Latency Fallback Contingency:** Because `ftxr-core` pre-allocates warm FFT/scratch buffers in Stage 5, static `musl` binaries are expected to match glibc latency closely. If Task 6.7's final `ftxr-oracle` benchmark observes `>10%` Pass 3 latency regression under `musl` due to `mallocng` lock contention under `rayon`, switch the two Linux release jobs to `container: ubuntu:22.04` on `ubuntu-24.04` / `ubuntu-24.04-arm` targeting `*-unknown-linux-gnu` and enforce a CI gate checking `readelf --version-info` max `GLIBC_* <= 2.35` so Raspberry Pi OS Bookworm (`glibc 2.36`) and Ubuntu 22.04+ remain 100% supported.)*

#### Exact Release Pipeline Jobs
1. **`prepare` (`ubuntu-24.04`):**
   - Validates `GITHUB_REF_NAME` matches `^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$`.
   - Extracts `CARGO_VERSION` from `cargo metadata --no-deps --format-version 1` for package `ftxr-modem` (and asserts `ftxr-core` version matches).
   - **Tag ↔ `Cargo.toml` parity gate:** fails immediately if `v${CARGO_VERSION} != ${GITHUB_REF_NAME}` (prevents wasting matrix build minutes on a mis-tagged commit).
   - Runs `./scripts/verify-compliance.sh` and `cargo-deny check`.
   - Exports outputs: `tag`, `version`, `is_prerelease` (`true` if tag contains `-`).
2. **`build` (4-platform matrix, `needs: prepare`):**
   - Builds `cargo build --release --locked --target ${{ matrix.rust_target }} --bin ftxr-modem`.
   - **Binary architecture & dependency gate:**
     - Linux: `file "$BIN"` matches `ELF 64-bit LSB.*(x86-64|ARM aarch64)`; `ldd "$BIN"` confirms static executable (or only `libc`/`libm`/`libgcc_s` with `GLIBC <= 2.35` if glibc fallback); zero `fftw3`/`gfortran`/`Qt`/`hamlib`.
     - macOS: `lipo -archs "$BIN"` equals exact arch (`arm64` or `x86_64`); `otool -L "$BIN"` has zero `/opt/homebrew` or `/usr/local` dylib paths; `vtool -show-build "$BIN"` verifies `minos <= 11.0`.
   - **Native execution smoke gate:**
     - `"$BIN" --version` equals `ftxr-modem ${{ needs.prepare.outputs.version }}`.
     - `"$BIN" --license` exits `0` and mentions `GPL-3.0-only` and `WSJT-X`.
     - `"$BIN" decode --mode ft8 --depth 3 tests/fixtures/210703_133430.wav` and `--mode ft4 --depth 3 tests/fixtures/000000_000002.wav` exit `0` with non-empty decodes.
     - `"$BIN" --worker` responds cleanly to a framed `hello` + `shutdown` probe with zero stray bytes on `stdout`.
   - **Flat Tarball Assembly (`digi-dx/scripts/install.sh:443-445` & GPLv3 §6):**
     ```bash
     STAGE_DIR="$(mktemp -d)"
     install -m 0755 "target/${RUST_TARGET}/release/ftxr-modem" "${STAGE_DIR}/ftxr-modem"
     install -m 0644 LICENSE NOTICE README.md "${STAGE_DIR}/"
     TARBALL="ftxr-modem-${TAG}-${OS_SLUG}-${ARCH_SLUG}.tar.gz"
     tar -czf "${TARBALL}" -C "${STAGE_DIR}" ftxr-modem LICENSE NOTICE README.md
     # Verify flat root layout (exact 4 entries, no leading ./ or subdir)
     test "$(tar -tzf "${TARBALL}" | sort | tr '\n' ' ')" = "LICENSE NOTICE README.md ftxr-modem "
     ```
   - Uploads `${TARBALL}` via `actions/upload-artifact@v4`.
3. **`publish` (`needs: [prepare, build]`, `runs-on: ubuntu-24.04`, `permissions: contents: write`):**
   - Downloads all 4 platform artifacts into `dist/`.
   - **All-platforms-ready gate:** Verifies all 4 required files exist and are `> 500 KiB`:
     - `dist/ftxr-modem-${TAG}-linux-x86_64.tar.gz`
     - `dist/ftxr-modem-${TAG}-linux-aarch64.tar.gz`
     - `dist/ftxr-modem-${TAG}-darwin-x86_64.tar.gz`
     - `dist/ftxr-modem-${TAG}-darwin-aarch64.tar.gz`
   - **GPLv3 §6(d) Corresponding Source Tarball:**
     ```bash
     git archive --format=tar.gz \
       --prefix="ftxr-modem-${TAG}/" \
       -o "dist/ftxr-modem-${TAG}-src.tar.gz" \
       "${TAG}"
     ```
   - **Canonical `SHA256SUMS` Generation & Self-Verification (`digi-dx/scripts/install.sh:435-441`):**
     ```bash
     (
       cd dist
       LC_ALL=C sha256sum ftxr-modem-*.tar.gz | sort -k2 > SHA256SUMS
       # Enforce exact two-space format with bare filenames (no '*', no './', no 'dist/')
       while IFS= read -r line; do
         [[ "$line" =~ ^[0-9a-f]{64}\ \ ftxr-modem-.*\.tar\.gz$ ]] || {
           echo "::error::Invalid SHA256SUMS line format: $line"
           exit 1
         }
       done < SHA256SUMS
       # Replay exact digi-dx/scripts/install.sh verification command per tarball
       for f in ftxr-modem-*.tar.gz; do
         grep "  ${f}\$" SHA256SUMS | sha256sum -c -
       done
     )
     ```
   - **GitHub Release Creation:** Uses `gh release create "${TAG}"` (setting `--prerelease` if `is_prerelease == true`), attaching all 4 binary tarballs + `ftxr-modem-${TAG}-src.tar.gz` + `SHA256SUMS`, with release notes including:
     - `CHANGELOG.md` section for `${TAG}`
     - Verification commands (`grep "  <tarball>$" SHA256SUMS | sha256sum -c -` and `shasum -a 256 -c -`)
     - `cargo install --locked --git https://github.com/mpmbq2/ftxr-modem --tag ${TAG} ftxr-modem`
     - Explicit GPLv3 §6(d) Corresponding Source link (`https://github.com/mpmbq2/ftxr-modem/tree/${TAG}`) & WSJT-X attribution/trademark disclaimer.

---

## 4. Ordered Implementation Tasks

Execute these tasks sequentially after Stage 5 passes all exit gates.

### Task 6.1 — Pre-Extraction Audit & Cleanup Inside `wsjtx/rust-ftx`
- **Inputs:** Completed Stage 5 workspace in `/home/antigravity/mpmbq2/wsjtx/rust-ftx/` (`feat/ftx-modem` branch).
- **Actions:**
  1. Verify `git -C wsjtx diff origin/master -- ':!rust-ftx'` is completely empty (confirming zero leftover Stage 2–4 Fortran dump hooks or stray edits to WSJT-X upstream files).
  2. Audit `crates/ftxr-core` and `crates/ftxr-modem` to confirm zero path/dev dependencies on `tools/ftxr-oracle`, `wsjtx`, or `digi-dx`, and zero `bessi0` / Numerical Recipes / C `-sys` dependencies.
  3. Add `crates/ftxr-modem/src/legal.rs` (`--version`, `--license` flags) and ensure every `.rs` source file carries the `// SPDX-License-Identifier: GPL-3.0-only` header and WSJT-X derivative attribution where ported.
  4. Freeze Stage 5's `ftxr-oracle` parity & latency scorecard JSON as the authoritative baseline for Task 6.7 (`rust-ftx/scorecards/stage5-baseline.json`).
- **Acceptance Criteria:** `cargo test --workspace` passes inside `wsjtx/rust-ftx/`; `git diff origin/master -- ':!rust-ftx'` is empty.

### Task 6.2 — Assemble Self-Contained Repository Tree (`mpmbq2/ftxr-modem`)
- **Inputs:** Audited `crates/ftxr-core`, `crates/ftxr-modem`, `docs/ipc-protocol.md`, Stage 1 codec vectors, Stage 3/4 golden fixture expectations, and WAV fixtures (`210703_133430.wav`, `000000_000002.wav`, `191111_110200.wav`).
- **Actions:**
  1. Initialize clean git repository `ftxr-modem` (default branch `main`, remote `git@github.com:mpmbq2/ftxr-modem.git` or HTTPS).
  2. Copy `crates/ftxr-core`, `crates/ftxr-modem`, and `docs/ipc-protocol.md` (explicitly omitting `tools/ftxr-oracle` and any WSJT-X build/cache files).
  3. Author standalone root `Cargo.toml` (§3.2, workspace members `crates/ftxr-core` and `crates/ftxr-modem` only) and generate a clean `Cargo.lock` containing zero `ftxr-oracle` crates.
  4. Fetch and commit canonical FSF plain-text GPLv3 as `LICENSE`, author `NOTICE` (§3.4), and copy both byte-identically into `crates/ftxr-core/` and `crates/ftxr-modem/`.
  5. Author `README.md` (standalone CLI examples, `--worker` IPC pointer, build/install instructions, GPLv3 license & WSJT-X trademark/attribution section) and `CHANGELOG.md` (`v0.1.0` section).
  6. Populate `tests/fixtures/` with the 3 compact WAVs (`~846 KiB`), `codec_vectors.json`, `expected_ft8.json`, `expected_ft4.json`, and `tests/fixtures/README.md` documenting each fixture's license provenance.
  7. Add self-contained integration tests in `crates/ftxr-modem/tests/` (`cli_regression.rs`, `worker_ipc.rs`, `synthetic_parity.rs`) exercising all golden WAVs, codec vectors, deterministic seeded AWGN/OSD/AP/subtraction scenarios, and worker IPC multiplexing/cancellation without `jt9`.
- **Acceptance Criteria:** Total `tests/fixtures/` size `< 1.5 MiB`; `cargo test --workspace --locked` passes standalone with zero external binaries on `PATH`.

### Task 6.3 — Configure GPLv3 Compliance, `deny.toml` & Audit Script
- **Inputs:** Standalone `ftxr-modem` workspace from Task 6.2.
- **Actions:**
  1. Add `deny.toml` (§3.5) restricting licenses to the GPLv3-compatible allowlist and banning `cc`, `cmake`, `bindgen`, `pkg-config`, and `fftw-sys`.
  2. Create `scripts/verify-compliance.sh` (§3.6) with executable permissions (`chmod 0755`) covering: FSF GPLv3 `LICENSE` hash check, crate-level `LICENSE`/`NOTICE` byte parity, SPDX header presence, Numerical Recipes / WSJT-X exclusion regexes, trademark title/version sanity check, `cargo metadata` license/pure-Rust JSON audit, and `cargo package --list` verification.
  3. Run `cargo deny check` and `./scripts/verify-compliance.sh` locally and fix any warnings or metadata omissions.
- **Acceptance Criteria:** `cargo deny check` exits `0` with zero license/ban/source errors; `./scripts/verify-compliance.sh` exits `0`; `cargo package --list -p ftxr-core` and `-p ftxr-modem` include `LICENSE` + `NOTICE` and omit `tests/fixtures/*.wav`.

### Task 6.4 — Author GitHub Actions CI & Release Workflows
- **Inputs:** Verified `ftxr-modem` repo from Task 6.3.
- **Actions:**
  1. Create `.github/workflows/ci.yml` (§3.7) with `lint-and-audit`, `test-matrix` (4 OS/arch pairs), `msrv`, and `clean-room-linux` (`rust:1.85-slim` container) jobs.
  2. Create `.github/workflows/release.yml` (§3.8) with `prepare` (tag ↔ `Cargo.toml` parity + compliance gate), `build` (4-target matrix with arch/linkage/smoke checks + flat tarball assembly), and `publish` (all-platforms-ready gate, `git archive` source tarball, strict two-space `SHA256SUMS` + self-check, GitHub Release upload).
  3. Validate workflow YAML syntax and shell scripts (`shellcheck` on `scripts/verify-compliance.sh`).
- **Acceptance Criteria:** All workflow YAML files pass validation; pushing `main` to `mpmbq2/ftxr-modem` turns every `ci.yml` job green.

### Task 6.5 — Clean-Room Build, Test & `cargo install` Verification
- **Inputs:** `mpmbq2/ftxr-modem` `main` branch.
- **Actions:**
  1. Execute clean-room verification inside a minimal Rust-only container (e.g., `rust:1.85-slim`) with no `wsjtx`, `jt9`, `gfortran`, `libfftw3*`, `libqt5*`, or `digi-dx` present.
  2. Run `cargo test --workspace --locked`, `cargo build --release --locked --bin ftxr-modem`, and `cargo install --locked --path crates/ftxr-modem --root /tmp/ftxr-clean-install`.
  3. Run the installed `/tmp/ftxr-clean-install/bin/ftxr-modem` against `tests/fixtures/` (CLI FT8/FT4 decode, encode round-trip, `--version`, `--license`, and `--worker` IPC smoke).
  4. Verify GitHub repository license detection displays `GPL-3.0 license` cleanly on `https://github.com/mpmbq2/ftxr-modem`.
- **Acceptance Criteria:** Zero missing tools/libs in clean-room container; 100% tests pass; `cargo install` binary passes all CLI and worker probes.

### Task 6.6 — Cut & Verify Initial Tagged Release (`v0.1.0`)
- **Inputs:** Green `ci.yml` on `mpmbq2/ftxr-modem` `main`.
- **Actions:**
  1. Confirm `Cargo.toml` workspace version is `0.1.0` and `CHANGELOG.md` has the finalized `v0.1.0` entry with date.
  2. Create annotated tag `git tag -a v0.1.0 -m "ftxr-modem v0.1.0"` and push `git push origin v0.1.0`.
  3. Monitor `.github/workflows/release.yml` to completion across all 4 matrix targets + `publish`.
  4. Download the published assets from `https://github.com/mpmbq2/ftxr-modem/releases/download/v0.1.0/`:
     - Verify `SHA256SUMS` using the exact `digi-dx/scripts/install.sh:435-441` pipeline on Linux (`sha256sum -c`) and macOS (`shasum -a 256 -c`).
     - Verify each tarball unpacks flat (`ftxr-modem`, `LICENSE`, `NOTICE`, `README.md`).
     - Verify `cargo install --locked --git https://github.com/mpmbq2/ftxr-modem --tag v0.1.0 ftxr-modem` succeeds in a fresh temp prefix.
- **Acceptance Criteria:** Release `v0.1.0` contains all 6 assets (`4×` platform tarballs, `1×` `-src.tar.gz`, `1×` `SHA256SUMS`); every verification command exits `0`.

### Task 6.7 — Final `ftxr-oracle` Release-Binary Verification & `wsjtx` Branch Hygiene
- **Inputs:** Downloaded & checksum-verified `ftxr-modem-v0.1.0-linux-<arch>.tar.gz` release binary; `wsjtx` (`feat/ftx-modem`) with built WSJT-X oracle tools (`jt9`, `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`) and Stage 5 baseline scorecard.
- **Actions:**
  1. Extract the tagged `ftxr-modem` binary from the verified release tarball into a temp directory (never rebuilding from `wsjtx/rust-ftx/`).
  2. Run `ftxr-oracle` in `wsjtx/rust-ftx/` explicitly pointed at the extracted external release binary across:
     - Full real-world FT8 (`jt9 -8 -d 3`) and FT4 (`jt9 -5 -d 3`) golden corpora
     - Deterministic AWGN sensitivity sweep (FT8 `0..-24 dB`, FT4 `0..-21 dB`)
     - Pure-noise false-positive sweep
     - Warm-IPC 3-pass latency & cancellation benchmark (`11.8 s / 13.5 s / 14.8 s` FT8 + FT4 equivalents)
  3. Diff the resulting JSON scorecard against `stage5-baseline.json` (checking musl vs glibc latency delta `≤ 10%`; if musl regresses Pass 3 latency `> 10%`, trigger the §3.8 glibc-2.35 container fallback and cut `v0.1.1`).
  4. Commit `rust-ftx/scorecards/stage6-release-v0.1.0.json` and an archival pointer note in `rust-ftx/README.md` on `feat/ftx-modem`.
  5. Perform final `wsjtx` branch hygiene verification:
     - `git -C wsjtx fetch origin master`
     - Verify `git -C wsjtx diff origin/master -- ':!rust-ftx'` produces zero output.
     - Verify `origin/master` still points strictly to upstream WSJT-X (`967c85a` or latest upstream) and `feat/ftx-modem` is NOT merged into `master`.
- **Acceptance Criteria:** Zero decode/sensitivity/FP drift vs Stage 5; latency meets Stage 5 targets and `≤ 10%` delta vs Stage 5; `wsjtx` `master` is untouched and `feat/ftx-modem` is cleanly archived.

---

## 5. Verification, Oracle Testing & Numerical Gates

### 5.1 Self-Contained Regression Gates (`mpmbq2/ftxr-modem` CI — Zero `wsjtx` Deps)
| Test Suite | Target / Command | Pass / Fail Criterion |
| :--- | :--- | :--- |
| Codec vectors (`codec_vectors.json`) | `cargo test -p ftxr-core --locked` | **100%** bit-exact 77-bit payload, CRC-14, 174-bit codeword, and FT8/FT4 channel tone match across all `i3`/`n3` message types (captured from Stage 1 `ft8code`/`ft4code` oracle). |
| Golden FT8 WAVs (`210703_133430.wav`, `191111_110200.wav`) | `cargo test -p ftxr-modem --test cli_regression --locked` | Exact expected decode set from `expected_ft8.json` at `-d 3`; AF within `±2 Hz`, DT within `±0.1 s`, SNR within `±2 dB`. |
| Golden FT4 WAV (`000000_000002.wav`) | `cargo test -p ftxr-modem --test cli_regression --locked` | Exact expected decode set from `expected_ft4.json` at `-d 3`; AF within `±2 Hz`, DT within `±0.1 s`, SNR within `±2 dB`. |
| Deterministic synthetic RX/TX (`synthetic_parity.rs`) | `cargo test -p ftxr-modem --test synthetic_parity --locked` | 100% nominal & late-TX loopback (`1–5 s` FT8, `0.5–1.5 s` FT4); OSD weak-signal recovery (`-20 dB` FT8 / `-16 dB` FT4); AP `a1`–`a6` recovery & `? a*` tagging; crowded 20 dB dynamic-range subtraction; **0** false decodes on seeded pure-AWGN slots. |
| Warm Worker IPC (`worker_ipc.rs`) | `cargo test -p ftxr-modem --test worker_ipc --locked` | Versioned protocol handshake, zero stray stdout logs, streaming decode events before pass completion, concurrent TX modulation during active Pass 3 decode, mid-pass cancellation `< 50 ms`, clean EOF exit `0`. |
| Compliance & packaging | `./scripts/verify-compliance.sh && cargo deny check` | Exit `0`: 100% GPLv3-compatible deps, zero `cc`/`*-sys` native deps, zero Numerical Recipes strings, byte-identical `LICENSE`/`NOTICE` in root + both crates, valid `cargo package` lists. |

### 5.2 Release Artifact & Portability Gates (`release.yml`)
| Gate | Exact Verification Command | Pass / Fail Criterion |
| :--- | :--- | :--- |
| Tag ↔ Manifest Parity | `v$(cargo metadata ... version) == ${GITHUB_REF_NAME}` | Exact string match; fails fast before platform builds. |
| Binary Architecture & Linkage (Linux) | `file ftxr-modem` & `ldd ftxr-modem` | Matches `x86-64` / `ARM aarch64`; static executable (or max `GLIBC <= 2.35` if glibc fallback); zero `fftw3`/`gfortran`/`Qt`/`hamlib`. |
| Binary Architecture & Linkage (macOS) | `lipo -archs ftxr-modem`, `otool -L ftxr-modem`, `vtool -show-build ftxr-modem` | Exact single arch (`arm64`/`x86_64`); only `/usr/lib/libSystem.B.dylib` (zero `/opt/homebrew` or `/usr/local`); `minos <= 11.0`. |
| Tarball Flat Layout | `tar -tzf ftxr-modem-<tag>-<os>-<arch>.tar.gz \| sort` | Exactly 4 root entries: `LICENSE`, `NOTICE`, `README.md`, `ftxr-modem` (matches `digi-dx/scripts/install.sh:443-445`). |
| `SHA256SUMS` Exact Format | `grep "  $tarball\$" SHA256SUMS \| sha256sum -c -` (Linux) & `shasum -a 256 -c -` (macOS) | Every line matches `^[0-9a-f]{64}  ftxr-modem-.*\.tar\.gz$`; exit `0` on both GNU `sha256sum` and BSD/macOS `shasum`. |
| All-Platforms-Ready | Check all 4 platform tarballs + `-src.tar.gz` before `gh release create` | Refuses to publish partial release if any target is missing. |
| `cargo install` Gate | `cargo install --locked --git https://github.com/mpmbq2/ftxr-modem --tag <tag> ftxr-modem --root <tmp>` | Compiles cleanly and installed binary passes `--version`, `--license`, and fixture decode smoke test. |

### 5.3 Final `ftxr-oracle` Regression Gate (in `wsjtx` Against Tagged Release Binary)
Run `ftxr-oracle` in `wsjtx/rust-ftx/` pointing at the extracted tagged release binary (not the local dev build), using standardized oracle CLI flags (`jt9 -8 -d 3 -c b -x b` for blind FT8, `jt9 -5 -d 3 -c b -x b` for blind FT4, explicit `-c`/`-x`/`-g`/`-Q` for AP `a1`–`a6`):
- **Real-world FT8 decode count:** Identical (`0` decode regression) vs Stage 5 scorecard (`≥ 95–98%` of classic `jt9 -8 -d 3`).
- **Real-world FT4 decode count:** Identical (`0` decode regression) vs Stage 5 scorecard (`≥ 95%` of `jt9 -5 -d 3`).
- **Deterministic AWGN 50% threshold (FT8 & FT4):** `0.0 dB` delta vs Stage 5 (`≤ 0.5–1.0 dB` vs `jt9 -d 3`).
- **False-positive rate on pure noise:** Identical to Stage 5 (`≤ jt9 -d 3`; `< 0.1%` without AP).
- **Warm IPC 3-pass latency (dev host):** IPC transfer `< 2 ms`, Pass 1 `< 150 ms`, Pass 2 `< 250 ms`, Pass 3 deep `< 500 ms`, cancel `< 50 ms`, and **`≤ 10%` regression** vs Stage 5 glibc benchmark (guards against static `musl` allocator contention under `rayon`).

---

## 6. Risks, Gotchas & Cross-Stage Handoff

### Key Risks & Mitigations
1. **Linux `glibc` Version Mismatch on Raspberry Pi OS / Ubuntu 22.04:** Building dynamically linked `gnu` binaries on `ubuntu-24.04` (`glibc 2.39`) breaks execution on Raspberry Pi OS Bookworm (`glibc 2.36`) and Ubuntu 22.04 (`glibc 2.35`). **Mitigation:** Build fully static `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` binaries; verify with Task 6.7's latency gate, with an explicit fallback to `ubuntu:22.04` (`glibc 2.35`) container builds verified by `readelf --version-info` if `musl` allocator contention ever degrades Pass 3 latency `> 10%`.
2. **`SHA256SUMS` Format Drift Breaking `digi-dx/scripts/install.sh`:** Using `sha256sum -b` (`*filename`), `sha256sum dist/*.tar.gz` (`dist/filename`), or single-space separators silently breaks `grep "  $tarball$" SHA256SUMS | sha256sum -c -` (`install.sh:436,438`). **Mitigation:** Generate `SHA256SUMS` inside `cd dist` with standard two-space text mode and replay the exact `install.sh` verification loop inside `release.yml` before uploading.
3. **Nested Tarball Directory Breaking `install.sh` Extraction:** `install.sh:443-444` expects `$tmp_dir/<binary>` directly at tarball root. **Mitigation:** Stage binaries flat via `tar -czf ... -C "$STAGE_DIR" ftxr-modem LICENSE NOTICE README.md` and assert exact `tar -tzf` entry list in CI.
4. **`Cargo.lock` v4 vs MSRV Drift:** Cargo 1.85+ defaults to `Cargo.lock` format version 4, which older toolchains (e.g., Rust `1.82`) may reject under `--locked`. **Mitigation:** Set `workspace.package.rust-version = "1.85"` (or maintain v3 lockfile if `1.82` is required) and gate every PR with an explicit pinned-toolchain `msrv` CI job running `--locked`.
5. **WSJT-X `COPYING` Format & `GPL-3.0-only` vs `GPL-3.0-or-later` Subtlety:** Copying `wsjtx/COPYING`'s curly-quoted, single-line-wrapped text can break GitHub Licensee detection; claiming `GPL-3.0-or-later` exceeds `wsjtx/README.md` and `wsjtx/widgets/about.cpp`. **Mitigation:** Ship canonical FSF `gpl-3.0.txt` as `LICENSE` (and crate-local copies for `cargo package`), declare `license = "GPL-3.0-only"`, and honor `wsjtx/TRADEMARK.md` explicitly in `NOTICE` and `README.md`.
6. **macOS Intel Runner Lifecycle (`macos-15-intel`):** If GitHub retires `macos-15-intel` in future releases, `darwin-x86_64` can seamlessly cross-compile on `macos-15` (`rustup target add x86_64-apple-darwin`, `MACOSX_DEPLOYMENT_TARGET=11.0`, `cargo build --release --target x86_64-apple-darwin`) because Apple's Xcode SDK is universal and `ftxr-modem` has zero native C dependencies.

### Cross-Stage Handoff Contracts
- **Consumed from Stages 0–5 (`wsjtx/rust-ftx/`):**
  - Stable public library API (`ftxr-core`: `Mode::{Ft8, Ft4}`, `DecodeConfig`, `QsoContext`, `ApTag`, `DecodedMessage`, `DecodeTelemetry`, `ModulateConfig`, `FtxrError`, `DecoderEngine`).
  - Finalized CLI flags, framed multiplexed `--worker` stdio IPC protocol, and `docs/ipc-protocol.md` (Stage 5).
  - Golden fixture expectation files, Stage 1 `codec_vectors.json`, and Stage 5 `ftxr-oracle` baseline scorecard (`stage5-baseline.json`).
  - `ftxr-oracle` support for benchmarking an external `--ftxr-modem-bin <path>` binary (Stage 5/6 handoff flag).
- **Exposed to Stage 7 (`digi-dx` Integration Blueprint):**
  - Pinned GitHub Release URL convention:
    - `https://github.com/mpmbq2/ftxr-modem/releases/download/<tag>/ftxr-modem-<tag>-<linux|darwin>-<x86_64|aarch64>.tar.gz`
    - `https://github.com/mpmbq2/ftxr-modem/releases/download/<tag>/SHA256SUMS` (exact `"  $tarball$"` two-space format matching `digi-dx/scripts/install.sh:435-441`; Stage 7 **must** pin an explicit tag like `v0.1.0` rather than interpolating literal `"latest"` into the tarball filename).
  - Flat tarball extraction contract: `ftxr-modem` executable + `LICENSE` + `NOTICE` + `README.md` at archive root, enabling `digi-dx`'s installer to place `/usr/local/lib/digi-dx/ftxr-modem` (`0755`) and `/usr/local/share/doc/ftxr-modem/{LICENSE,NOTICE}` (or `/usr/local/lib/digi-dx/ftxr-modem.{LICENSE,NOTICE}`) to fulfill GPLv3 §6 bundling compliance while keeping `digi-dx` strictly MIT (`Cargo.toml` zero GPL crates).
  - `--from-source` fallback command for `digi-dx/scripts/install.sh`: `cargo install --locked --git https://github.com/mpmbq2/ftxr-modem --tag <pinned_tag> ftxr-modem` (no `gfortran`/`fftw3`/`Qt` required).
  - Fast native execution probe for `digi-dx` installer & preflight (`src/preflight/mod.rs`): `ftxr-modem --version` (exits `0`, prints `ftxr-modem <semver>`) + `--worker` protocol version handshake.
