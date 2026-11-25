# `ftxr-modem` — Pure-Rust FT8 & FT4 Modem Engine (Development Workspace)

`ftxr-modem` is an independent, pure-Rust **FT8 and FT4** modem engine (decoder + encoder/modulator) based on the classic single-threaded algorithms of WSJT-X (`lib/ft8/`, `lib/ft4/`, `lib/77bit/`). It is licensed under **GNU General Public License v3.0 only (`GPL-3.0-only`)** (see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE)).

During Stages 0–5 of [`ROADMAP.md`](ROADMAP.md), this Cargo workspace is developed inside `wsjtx/rust-ftx/` alongside WSJT-X's Fortran reference tools for stage-by-stage numerical verification, before lift-and-shift extraction into the standalone repository `mpmbq2/ftxr-modem` in Stage 6.

## Workspace Layout

- [`crates/ftxr-core`](crates/ftxr-core) — Pure-Rust GPLv3 library crate (FT8 & FT4 77-bit codec, FEC, GFSK modulator, RX decoders). Zero dependencies on WSJT-X, Fortran, FFTW, Qt, or `digi-dx`.
- [`crates/ftxr-modem`](crates/ftxr-modem) — Standalone GPLv3 CLI + persistent warm stdio IPC worker binary (`ftxr-modem`).
- [`tools/ftxr-oracle`](tools/ftxr-oracle) — Development-only verification, deterministic corpus generation, and `jt9` benchmarking harness (stays in `wsjtx`, excluded during Stage 6 extraction).
- [`fixtures/committed`](fixtures/committed) — Compact golden real-world & regression WAV fixtures + SHA-256 manifest.
- [`scorecards`](scorecards) — Committed machine-readable `jt9` baseline and parity scorecards.
- [`docs/ipc-protocol.md`](docs/ipc-protocol.md) — Versioned multiplexed stdio IPC contract (populated in Stage 5).

## Headless WSJT-X Oracle Build (Ubuntu 24.04)

Only the 6 reference CLI/simulator executables (`jt9`, `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`) are built into the gitignored `.oracle-build/` directory without modifying any upstream WSJT-X source files:

```bash
cmake -S .. \
  -B .oracle-build \
  -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DWSJT_SKIP_MAP65=ON \
  -DWSJT_GENERATE_DOCS=OFF \
  -DWSJT_SKIP_MANPAGES=ON \
  -DWSJT_BUILD_TESTS=OFF \
  -DWSJT_BUILD_UTILS=ON \
  -DWSJT_SKIP_BUNDLE_FIXUP=ON \
  -Wno-dev

cmake --build .oracle-build \
  --target jt9 ft8code ft4code ft8sim ft4sim ft4sim_mult \
  -j"$(nproc)"
```

## Quick Start

```bash
cargo test --workspace
cargo run --release -p ftxr-oracle -- smoke
cargo run --release -p ftxr-oracle -- generate --suite all --trials 20
cargo run --release -p ftxr-oracle -- baseline \
  --suite all \
  --depths 1,2,3 \
  --profiles blind-no-ap,blind-cq-ap,qso-ap \
  --patience 1 \
  --out scorecards/jt9-baseline-v1.json
```

## Legal & Trademark Notice

`WSJT`, `WSJT-X`, `MAP65`, and `QMAP` are trademarks of the WSJT Development Team. `ftxr-modem` is an independent project based on WSJT-X algorithms and is not affiliated with or endorsed by the WSJT Development Team. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE) for full copyright and license terms.
