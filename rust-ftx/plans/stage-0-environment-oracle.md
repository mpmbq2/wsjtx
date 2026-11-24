# Stage 0 Plan — Baseline Environment, Golden Corpus & Oracle Harness

- **Stage:** 0 of 7 (`wsjtx` development branch `feat/ftx-modem`)
- **Primary Oracle Targets:** `jt9` (classic single-threaded `-8` / `-5`, **not** `-M`/`lib/ft8var/`), `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`
- **Target Host Baseline:** Ubuntu 24.04.5 LTS (`noble`, `x86_64`), `gcc`/`g++` 13.3.0, `cmake` 3.28.3, `ninja`, `rustc`/`cargo` (currently `1.100.0`)
- **Workspace Root:** `/home/antigravity/mpmbq2/wsjtx/rust-ftx/`

## 1. Scope, Prerequisites & Exit Gate

### Scope
Stage 0 establishes a reproducible, self-contained development and verification harness inside `/home/antigravity/mpmbq2/wsjtx/rust-ftx/` without modifying any upstream WSJT-X source files or `/home/antigravity/mpmbq2/digi-dx` (strictly read-only reference):

1. **Headless WSJT-X Oracle Build:** Install the minimal Ubuntu 24.04 apt toolchain (`gfortran`, FFTW3, Boost.Log, Hamlib, libusb, required Qt5 dev headers) and compile **only** the 6 WSJT-X reference targets into a gitignored out-of-tree build directory (`rust-ftx/.oracle-build/`).
2. **GPLv3 Rust Workspace Scaffolding:** Create `rust-ftx/Cargo.toml` with `crates/ftxr-core`, `crates/ftxr-modem`, and `tools/ftxr-oracle`, complete with GPLv3 `LICENSE`, WSJT-X `NOTICE` attribution, `README.md`, `docs/ipc-protocol.md` stub, `.gitignore`, and compiling stub public APIs matching the cross-stage contract.
3. **Golden Corpus & Deterministic Synthetic Generator:** Assemble a compact committed golden fixture set (<2 MB in git) paired with `ftxr-oracle`'s deterministic `ChaCha8Rng`-seeded synthetic AWGN/multi-signal/offset/late-TX/pure-noise/AP corpus generator calibrated identically to `ft8sim`/`ft4sim` (`SNR_2500`).
4. **Baseline `jt9` Scorecard:** Capture committed, machine-readable JSON scorecards for classic `jt9 -8` and `jt9 -5` across depths `-d 1/2/3` and blind vs. AP profiles.

### Prerequisites
- Clean git checkout on `feat/ftx-modem` inside `/home/antigravity/mpmbq2/wsjtx` (`rust-ftx/ROADMAP.md` present).
- Working Rust toolchain (`rustc`, `cargo`) and `sudo apt-get` access on Ubuntu 24.04 LTS.
- Read-only access to `/home/antigravity/mpmbq2/digi-dx/tests/fixtures/191111_110200.{wav,txt}` and `/home/antigravity/mpmbq2/wsjtx/samples/{FT8,FT4}/`.

### Deliverables
- Built & smoke-tested oracle binaries in `rust-ftx/.oracle-build/{jt9,ft8code,ft4code,ft8sim,ft4sim,ft4sim_mult}`.
- Extractable GPLv3 project files: `rust-ftx/{Cargo.toml,LICENSE,NOTICE,README.md,.gitignore,docs/ipc-protocol.md}`, `crates/ftxr-core/`, `crates/ftxr-modem/`.
- Dev-only oracle harness: `rust-ftx/tools/ftxr-oracle/`.
- Compact committed golden fixtures + SHA-256 manifest: `rust-ftx/fixtures/committed/`.
- Committed JSON baseline scorecard: `rust-ftx/scorecards/jt9-baseline-v1.json`.

### Exit Gate Checklist
- [ ] All 6 WSJT-X oracle binaries compile cleanly in `rust-ftx/.oracle-build/` with zero edits to upstream WSJT-X files and pass `ftxr-oracle smoke`.
- [ ] `cargo check --workspace`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets` pass cleanly with zero warnings.
- [ ] GPLv3 `LICENSE`, WSJT-X `NOTICE`, and `Cargo.toml` license metadata (`GPL-3.0-or-later`) are present; all workspace crates have GPLv3-compatible licenses (MIT/Apache-2.0/BSD).
- [ ] `ftxr-oracle generate` produces bit-identical WAV files across repeated runs and matches theoretical `SNR_2500` within `±0.05 dB` mean bias.
- [ ] `ftxr-oracle baseline` produces deterministic JSON scorecards for `jt9 -8` and `jt9 -5` across depths 1/2/3 and blind vs. AP profiles, committed at `rust-ftx/scorecards/jt9-baseline-v1.json`.
- [ ] `git status` shows zero untracked build artifacts, zero cached synthetic WAVs, zero stray `000000_*.wav`/`timer.out`/`jt9_wisdom.dat` files, and zero modifications to upstream `wsjtx` files or `digi-dx`.

---

## 2. Source Audit & Technical Reference

All constants, build requirements, CLI flags, stdout formats, and calibration formulas below were verified directly against the WSJT-X (`v3.0.2.0`) and `digi-dx` source trees on disk.

### 2.1 WSJT-X CMake Build Graph & Dependencies (`CMakeLists.txt`)

| CMake Requirement | Source Location | Ubuntu 24.04 Package(s) / Mitigation |
| :--- | :--- | :--- |
| `project(wsjtx VERSION 3.0.2.0 LANGUAGES C CXX Fortran)` | `CMakeLists.txt:63-66` | `build-essential`, `cmake`, `ninja-build`, `gfortran` (`4:13.2.0-7ubuntu1`) |
| `find_package(Boost 1.62 REQUIRED COMPONENTS log_setup log)` | `CMakeLists.txt:1050` | `libboost-dev`, `libboost-log-dev` (Boost 1.83 on Noble) |
| `find_package(OpenMP)` (`wsjt_fort_omp` + `jt9`) | `CMakeLists.txt:1055,1316,1643` | Provided by `gcc`/`gfortran` (`libgomp1`) |
| `find_package(FFTW3 COMPONENTS single threads REQUIRED)` | `CMakeLists.txt:1075`, `CMake/Modules/FindFFTW3.cmake` | `libfftw3-dev` (provides `fftw3.h`, `libfftw3f`, `libfftw3f_threads`) |
| `find_package(Hamlib REQUIRED)` | `CMakeLists.txt:1080`, `CMake/Modules/FindHamlib.cmake:32` | `libhamlib-dev`, `pkg-config`, `libudev-dev` (needed for static ldflags in `FindHamlib`) |
| `find_package(Usb REQUIRED)` | `CMakeLists.txt:1088`, `CMake/Modules/FindUsb.cmake` | `libusb-1.0-0-dev` |
| `find_package(Qt5 COMPONENTS Widgets SerialPort Multimedia PrintSupport Sql LinguistTools WebSockets REQUIRED)` | `CMakeLists.txt:1099,1270-1271` | `qtbase5-dev`, `qttools5-dev`, `qttools5-dev-tools`, `qtmultimedia5-dev`, `libqt5serialport5-dev`, `libqt5websockets5-dev` |
| `WSJT_SKIP_MAP65` (`OFF` by default -> `find_package(Portaudio REQUIRED)`) | `CMakeLists.txt:174,1683-1686` | Pass `-DWSJT_SKIP_MAP65=ON` (avoids `portaudio19-dev` + MAP65) |
| `WSJT_GENERATE_DOCS` (`ON` by default -> `doc/CMakeLists.txt` `asciidoctor`) | `CMakeLists.txt:167,1301-1303` | Pass `-DWSJT_GENERATE_DOCS=OFF` (avoids `asciidoctor`/Ruby) |
| `WSJT_SKIP_MANPAGES` (`OFF` by default -> `manpages/CMakeLists.txt:19` `SEND_ERROR` without `a2x` in Release!) | `CMakeLists.txt:166,1824-1827`, `manpages/CMakeLists.txt:16-26` | Pass `-DWSJT_SKIP_MANPAGES=ON` (critical for Release builds without `asciidoc`) |
| `WSJT_BUILD_TESTS` (`ON` by default -> `tests/CMakeLists.txt:1` `Qt5Test` + `pFUnit`) | `CMakeLists.txt:171,1304-1307`, `tests/CMakeLists.txt:1` | Pass `-DWSJT_BUILD_TESTS=OFF` |
| `WSJT_BUILD_UTILS` (`ON` by default -> registers `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`) | `CMakeLists.txt:170,1348,1419-1448` | Pass `-DWSJT_BUILD_UTILS=ON` |
| `WSJT_SKIP_BUNDLE_FIXUP` | `CMakeLists.txt:173,2061-2066` | Pass `-DWSJT_SKIP_BUNDLE_FIXUP=ON` |

**Target Link Graph for the 6 Oracle Binaries:**
- `jt9` (`CMakeLists.txt:1642-1681`): links `wsjt_fort_omp` (compiled with OpenMP `-fopenmp` + `-fbounds-check -funroll-all-loops -fno-f2c`), `wsjt_cxx` (`crc10/13/14.cpp`, `Logger.cpp`, `gran.c`, `sgran.c`, `init_random_seed.c`, etc.), `fort_qt` (`lib/shmem.cpp` -> `Qt5::Core`), and depends on `revisiontag` (`CMakeLists.txt:2018`).
- `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult` (`CMakeLists.txt:1419-1448`): each links `wsjt_fort` (non-OpenMP static Fortran lib) + `wsjt_cxx`.
- None of the 6 targets compile `wsjtx`, `wsjt_qt`, `qcp`, `qmap`, `wsprd`, or Qt translations when built selectively with `cmake --build ... --target jt9 ft8code ft4code ft8sim ft4sim ft4sim_mult`.
- **Fortran Compiler Flags (`CMakeLists.txt:1185`):** Release builds include `-fbounds-check -funroll-all-loops -fno-f2c -ffpe-summary=invalid,zero,overflow,underflow -Wall -Wno-conversion -Wno-c-binding-type -fno-second-underscore`. Because `-fbounds-check` is active in Release, malformed CLI arguments (e.g., short `.wav` filenames) trigger immediate Fortran runtime bounds aborts!

### 2.2 `jt9` CLI Flags, Defaults & Internal Execution Mechanics (`lib/jt9.f90`, `lib/decoder.f90`, `lib/ft8_decode.f90`, `lib/ft4_decode.f90`)

| Flag | Long Option | Variable & Verified Default (`lib/jt9.f90:29-35,111-119`) | Critical Behavior / Gotchas |
| :--- | :--- | :--- | :--- |
| `-8` | `--ft8` | `mode = 8` (default `0` -> JT65+JT9) | Classic single-threaded `lib/ft8/` decoder (when `-M` is **not** set). |
| `-5` | `--ft4` | `mode = 5` | Classic `lib/ft4/` decoder. Sets `npts = 21 * 3456 = 72576` (`jt9.f90:306`). |
| `-M` | `--multithreadft8` | `multift8 = .false.` | **DO NOT PASS.** Activates `lib/ft8var/`, which is out of oracle scope. |
| `-d <1\|2\|3>` | `--depth` | `ndepth = 1` | **FT8 (`ft8_decode.f90:102-108,176-182`, `ft8b.f90:428-434`):** `-d 1` skips `nzhsym=41/47`, runs 2 passes at `nzhsym=50`, `syncmin=2.1`, BP-only (`maxosd=-1`); `-d 2` enables `nzhsym=41/47/50`, 3 passes, `syncmin=2.1`, BP+OSD (`maxosd=2`, no `lrefinedt` at `nzhsym=47`); `-d 3` sets `syncmin=1.3`, `lrefinedt=.true.`, BP+OSD (`maxosd=2`). **FT4 (`ft4_decode.f90:194-206,326`):** `-d 1` runs 1 pass, no subtraction, BP-only, **no AP** (`npasses=3`); `-d 2` runs 3 passes with subtraction, BP-only (`doosd=.false.`), AP enabled; `-d 3` runs 3 passes with subtraction + BP+OSD (`maxosd=2`, or `3` within `±50 Hz` of `nfqso`) + AP. |
| `-c <CALL>` | `--my-call` | `mycall = 'K1ABC'` (`jt9.f90:111`) | **Gotcha:** Passing `-c b` blanks `mycall` to 12 spaces (`jt9.f90:257`). |
| `-x <CALL>` | `--his-call` | `hiscall = 'W9XYZ'` (`jt9.f90:111`) | **Gotcha:** Passing `-x b` blanks BOTH `hiscall` (12 spaces) AND `hisgrid` (6 spaces) (`jt9.f90:258-261`). |
| `-g <GRID>` | `--his-grid` | `hisgrid = 'EN37'` (`jt9.f90:112`) | **Gotcha:** If `-x <CALL>` is passed without `-g`, `hisgrid` remains `'EN37'` (len 4), enabling `ft8_a8d` (`ft8_decode.f90:289`)! Pass `-g .` (len 1 < 4) when testing `a1`–`a6` without `a8`. |
| `-Q <0..5>` | `--QSOprog` | `nQSOProg = 0` (`jt9.f90:30`!) | **Gotcha:** Help text at `jt9.f90:95` erroneously says `default PROGRESS=1`, but code initializes `nQSOProg=0`. With `nQSOProg=0` and `lft8apon=.true.` (`jt9.f90:350`), `a1` (`CQ ??? ???`) **still runs even with `-c b -x b`**! Passing `-c b -x b -Q 1` selects `naptypes(1,:)=[2,3,0,0]`, both skipped because `mycall`/`hiscall` are blank -> **true zero-AP blind mode**. |
| `-L <HZ>` / `-H <HZ>` | `--lowest` / `--highest` | `flow = 200`, `fhigh = 4000` | Passband search bounds (`nfa`, `nfb`). FT4 clamps to `[200, 4910]` (`getcandidates4.f90:45-47`). |
| `-f <HZ>` / `-F <HZ>` | `--rx-frequency` / `--freq-tolerance` | `nrxfreq = 1500`, `ntol = 20` | `nfqso` prioritizes candidates within `±20 Hz` (FT4) / `±napwid=75 Hz` (FT8 AP `a3`–`a6`, `jt9.f90:352`). |
| `-w <0..4>` | `--patience` | `npatience = 1` (`jt9.f90:115`) | Maps in `four2a.f90:62-66` to `0=FFTW_ESTIMATE`, `1=FFTW_ESTIMATE_PATIENT`, `2=FFTW_MEASURE`, `3=FFTW_PATIENT`, `4=FFTW_EXHAUSTIVE`. |
| `-a <PATH>` | `--data-path` | `data_dir = '.'` (`prog_args.f90:3`) | `jt9` reads/writes `<data_dir>/jt9_wisdom.dat` (`jt9.f90:242,504`) and `<data_dir>/timer.out` (`jt9.f90:302`). |
| `-t <PATH>` | `--temp-path` | `temp_dir = '.'` (`prog_args.f90:3`) | `jt9` opens/overwrites `<temp_dir>/decoded.txt` (`decoder.f90:149`). |
| `-m <N>` | `--fft-threads` | `nthreads = 1` | Keep `-m 1` (plus `OMP_NUM_THREADS=1`) for deterministic single-threaded baseline. |
| `-q` | `--quiet` | `lquiet = .false.` | Suppresses `<DecodeFinished>`. Do **not** pass `-q` so parser verifies completion marker. |

**Critical `jt9` Disk-WAV Mechanics (`lib/jt9.f90:284-334,424-443`):**
1. **Filename Parsing Crash Risk (`jt9.f90:286-294`):** `jt9` finds `i1 = index(infile, '.wav')` (or `.WAV`), checks `infile(i1-5:i1-5) .eq. '_'`, and reads either `infile(i1-4:i1-1)` (`HHMM`) or `infile(i1-6:i1-1)` (`HHMMSS`) into `nutc`. Because Release builds compile with `-fbounds-check`, any path with fewer than 6 characters before `.wav` crashes! **Always** name WAV inputs `YYMMDD_HHMMSS.wav` (e.g., `000000_000001.wav`).
2. **Silence Gate (`lib/decoder.f90:92-93`):** `rms = sqrt(dot_product(float(id2(1:180000)), float(id2(1:180000))) / 180000.0); if (rms .lt. 0.5) go to 800`. Even in FT4 mode (where `id2(72577:180000)=0`), `rms` is averaged over 180,000 samples! With `gain = 100.0` on unit-variance AWGN, FT4 `rms ≈ 100 * sqrt(72576/180000) ≈ 63.5 >> 0.5`.
3. **Block Reading & EOF (`lib/jt9.f90:295-334`):** Reads in chunks of `kstep = 3456` samples (`nsps = 6912`).
   - FT8 stops at `k = 52 * 3456 = 179,712` samples (ignoring final 288 of 180,000).
   - FT4 stops at `npts = 21 * 3456 = 72,576` samples (`6.048 s`).
   - On shorter/truncated WAVs, `read(..., end=3)` prints `EOF on input file <path>` to **stdout** and leaves the remaining buffer zero-padded.
4. **FT8 Internal 3-Pass `nzhsym = 41 / 47 / 50` on Disk WAVs (`lib/jt9.f90:424-443`, `lib/ft8_decode.f90:102-162`):**
   - Pass A (`nzhsym = 41`): feeds `1..141,696` (`11.808 s`) zero-padded to 180,000. Skipped for `-d 1`; for `-d 2/3` runs 3 passes without AP (`ft8b.f90:281`), caching early decodes (`ndec_early`). `<DecodeFinished>` is suppressed (`decoder.f90:1400`).
   - Pass B (`nzhsym = 47`): feeds `1..162,432` (`13.536 s`). Does **not** run `sync8`/`ft8b`; only subtracts early decodes with `xdt_save - 0.5 < 0.396` into saved `dd1` (`lrefinedt = (ndepth == 3)`). `<DecodeFinished>` is suppressed.
   - Pass C (`nzhsym = 50`): copies `dd1(1:162432)` + `id2(162433:172800)` (`14.400 s`), **explicitly zeroing `id2a(172801:180000)` (the last 0.600 s!)**, subtracts remaining `ndec_early`, runs 2 passes (`-d 1`) or 3 passes (`-d 2/3`) with AP, runs `a7` (if cross-slot history exists in `ft8_a7`), runs `a8` (if `hiscall` & `hisgrid` set and `ltry_a8` true), and prints `<DecodeFinished>`.
5. **Cross-File `a7` State Leakage (`lib/ft8_decode.f90:88-100,251-286`):** Passing multiple WAVs to one `jt9 -8` process persists `msg0`/`dt0`/`f0` in Fortran `save` arrays and triggers `a7` decodes on subsequent files. `ftxr-oracle` MUST spawn **one `jt9` process per WAV file** for single-slot benchmarking.

### 2.3 `jt9` Stdout Formats (`lib/decoder.f90`)

- **FT8 (`decoder.f90:1741-1755`):**
  ```fortran
  1001 format(i6.6,i4,f5.1,i5,' ~ ',1x,a37,1x,a2)
  ```
  - Columns (1-indexed): `1..6` UTC (`HHMMSS`), `7..10` SNR dB (`i4`), `11..15` DT s (`f5.1`), `16..20` AF Hz (`i5`), `21..23` `' ~ '`, `24` `' '`, `25..61` `decoded0` (37 chars; col `61` = `'?'` iff `nap != 0 && qual < 0.17`), `62` `' '`, `63..64` `annot` (`'  '` if `nap == 0`, else `'a1'`..`'a8'`).
- **FT4 (`decoder.f90:1821-1828`):**
  ```fortran
  1001 format(i6.6,i4,f5.1,i5,' + ',1x,a37,1x,a2)
  ```
  - Identical column layout except columns `21..23` are `' + '`.
- **Completion Marker (`decoder.f90:1403-1404`):**
  ```fortran
  1010 format('<DecodeFinished>',2i4,i9)
  ```
- **AP Tag & Uncertainty Semantics (`digi-dx/src/qso/reconciler.rs:22-32`, `decoder.f90:1741-1745`):**
  - `nap == 0` -> `raw_metadata = None` (confidence rank 2, verified CRC)
  - `nap in 1..=8`, `qual >= 0.17` -> `raw_metadata = Some("aN")` (confidence rank 1)
  - `nap in 1..=8`, `qual < 0.17` -> `raw_metadata = Some("? aN")` (confidence rank 0)
  - **Parser Gotcha:** Free-text 77-bit messages may legitimately end with `?` (valid character in 42-char free-text alphabet, e.g., `"PSE QSL?"`), and FT4 messages may contain `+` (e.g., `K1ABC W9XYZ +05`). Therefore the parser MUST: (a) parse the mode marker `' ~ '` / `' + '` strictly at token position 4 (columns 21..23) rather than a blind `split_once('+')`, and (b) treat a trailing `?` as AP uncertainty **only** when a valid `a1`..`a8` annotation follows it.

### 2.4 Protocol Parameters & Exact `SNR_2500` Calibration (`ft8_params.f90`, `ft4_params.f90`, `ft8sim.f90`, `ft4sim.f90`, `ft4sim_mult.f90`, `gran.c`)

| Constant | FT8 (`lib/ft8/ft8_params.f90`) | FT4 (`lib/ft4/ft4_params.f90`) |
| :--- | :--- | :--- |
| Sample rate `fs` | `12_000` Hz | `12_000` Hz |
| Information + CRC bits `KK` | `91` (`77 + 14`) | `91` (`77 + 14`) |
| Codeword bits `N` | `174` | `174` |
| Data symbols `ND` | `58` (3 bits/sym, 8-GFSK, `BT=2.0`) | `87` (2 bits/sym, 4-GFSK, `BT=1.0`) |
| Sync symbols `NS` | `21` (three 7-sym Costas arrays) | `16` (four 4-sym Costas arrays) |
| Active symbols `NN` / Total `NN2` | `79` / `79` | `103` / `105` (`+2` ramp symbols) |
| Samples/symbol `NSPS` | `1920` (`0.160 s`, `6.25 Hz` spacing) | `576` (`0.048 s`, `20.8333 Hz` spacing) |
| Active samples `NZ` / Shaped `NZ2` | `151_680` (`12.64 s`) | `59_328` (`4.944 s`) / `60_480` (`5.04 s`) |
| WAV length `NMAX` | `180_000` (`15.0 s`) | `72_576` (`21 * 3456 = 6.048 s`) |
| `NFFT1`, `NH1`, `NSTEP`, `NHSYM`, `NDOWN` | `3840`, `1920`, `480`, `372`, `60` | `2304`, `1152`, `576`, `122`, `18` |
| Nominal TX start (`Modulator.cpp:76,79`) | `500 ms` (`6000` samples) | `300 ms` (`3600` samples -> ramp start; sync at `348 ms`) |
| Simulator `DT = 0.0` sync start | `6000` (`0.500 s`, `ft8sim.f90:129`) | `6000` (`0.500 s`; ramp starts at `k = 6000 - 576 = 5424`, `ft4sim.f90:85`) |

**Exact `SNR_2500` Calibration Math (`ft8sim.f90:92-94,131-163`, `ft4sim.f90:52-54,93-114`, `ft4sim_mult.f90:40,76,89-92`):**
- Let `fs = 12000.0` Hz, Nyquist bandwidth $B_{\text{nyq}} = f_s / 2 = 6000.0\text{ Hz}$, reference bandwidth $B_{\text{ref}} = 2500.0\text{ Hz}$:
  $$\text{bandwidth\_ratio} = \frac{2500.0}{f_s / 2.0} = \frac{5}{12} \approx 0.41666667$$
- A unit-amplitude constant-envelope GFSK real waveform $s_{\text{unit}}[n] = \sin(\phi[n])$ (or $\cos(\phi[n])$) has active-window mean-square power $P_{\text{unit}} = \tfrac{1}{2}$ (RMS $= 1/\sqrt{2}$).
- Adding i.i.d. zero-mean unit-variance Gaussian noise $w[n] \sim \mathcal{N}(0, 1)$ (`lib/gran.c`, Marsaglia polar transform) produces noise power $P_{\text{noise},2500} = \text{bandwidth\_ratio} \cdot 1.0$ inside 2500 Hz.
- Scaling each signal by:
  $$\text{sig} = \sqrt{2 \cdot \text{bandwidth\_ratio}} \cdot 10^{0.05 \cdot \text{snr\_db}} = \sqrt{\frac{5}{6}} \cdot 10^{\text{snr\_db}/20}$$
  yields exact:
  $$\text{SNR}_{2500} = 10 \log_{10}\!\left(\frac{\text{sig}^2 / 2}{\text{bandwidth\_ratio} \cdot 1.0}\right) = \text{snr\_db}\text{ dB}.$$
- **Simulator Determinism & Bugs to Avoid in `ftxr-oracle`:**
  1. `ft8sim` (`ft8sim.f90:124`) and `ft4sim` (`ft4sim.f90:79`) call `sgran()` -> `init_random_seed()` (`lib/init_random_seed.c:33`), which reads `/dev/urandom`, making noisy runs non-deterministic. However, with `fspread = 0.0`, `delay = 0.0`, and `snrdb = 99.0` (`> 90.0`), neither `watterson()` nor `gran()` is called: both tools output a **100% deterministic clean waveform** normalized to peak `32766.9` (`ft8sim.f90:159`, `ft4sim.f90:110`).
  2. `ft8sim.f90:129` uses circular `cshift(c0, -nint((xdt+0.5)/dt))`, which wraps signals around when `xdt < -0.5` or `xdt > +1.86 s` (unlike `ft4sim.f90:87-88` which zeroes the wrapped tail). `ftxr-oracle` MUST extract clean templates at `xdt = 0.0` and apply **linear zero-padded shifting/truncation** in Rust.
  3. `ft8sim.f90:142` measures diagnostic `psig`/`pnoise` over `6001..157692` (12 samples past `6000 + 151680 = 157680`), which is display-only. `ftxr-oracle` measures empirical `SNR_2500` over the exact active non-ramp interval (`6000 + 240 .. 157680 - 240` for FT8; `6000 .. 65328` for FT4) after subtracting known signal from noise.
  4. `ft8sim`, `ft4sim`, and `ft4sim_mult` all write `000000_000001.wav` into CWD (`ft4sim_mult` also requires `messages.txt` in CWD, found at `lib/ft4/messages.txt`). Always run them inside a `tempfile::TempDir`.

### 2.5 Existing Sample Fixtures Verified on Disk

| Fixture | Path | Size (B) | Format | Samples | Duration | Notes |
| :--- | :--- | ---: | :--- | ---: | ---: | :--- |
| WSJT-X FT8 sample | `wsjtx/samples/FT8/210703_133430.wav` | 360,246 | 16-bit PCM mono 12 kHz (BWF header) | 180,000 | 15.000 s | Real crowded HF FT8 slot |
| WSJT-X FT4 sample | `wsjtx/samples/FT4/000000_000002.wav` | 145,196 | 16-bit PCM mono 12 kHz (44-B header) | 72,576 | 6.048 s | Real/WSJT-X reference FT4 slot |
| `digi-dx` FT8 fixture | `digi-dx/tests/fixtures/191111_110200.wav` | 360,202 | 16-bit PCM mono 12 kHz (BWF header) | 180,000 | 15.000 s | 5 known decodes in `191111_110200.txt` (read-only source; copy into `rust-ftx/fixtures/committed/ft8/`) |

---

## 3. Architecture & Concrete Rust Design

### 3.1 Workspace Layout (`/home/antigravity/mpmbq2/wsjtx/rust-ftx/`)

```text
rust-ftx/
├── Cargo.toml                         # Workspace root (resolver = "2")
├── LICENSE                            # Verbatim GPLv3 (copied from ../COPYING)
├── NOTICE                             # Upstream WSJT-X attribution + GPLv3 §5 notice
├── README.md                          # Workspace architecture & oracle build guide
├── .gitignore                         # Ignores target/, .oracle-build/, .oracle-cache/, stray WAVs/logs
├── ROADMAP.md                         # Existing (unmodified)
├── docs/
│   └── ipc-protocol.md                # Placeholder stub (populated in Stage 5)
├── fixtures/
│   └── committed/
│       ├── manifest.json              # Fixture metadata + SHA-256 checksums + reference paths
│       ├── ft8/
│       │   ├── 191111_110200.wav      # Copied from read-only digi-dx fixture
│       │   ├── 191111_110200.txt      # Reference decode listing
│       │   ├── 000000_000101.wav      # ft8_crowded_20db (10-sig, 20+ dB dynamic range)
│       │   ├── 000000_000102.wav      # ft8_fading_mm (Watterson MM, frozen)
│       │   └── 000000_000103.wav      # ft8_late_tx_5s (+5.0 s late start)
│       └── ft4/
│           └── 000000_000201.wav      # ft4_crowded_20db (10-sig, 20+ dB dynamic range)
├── scorecards/
│   └── jt9-baseline-v1.json           # Committed Stage 0 baseline scorecard
├── crates/
│   ├── ftxr-core/
│   │   ├── Cargo.toml                 # Pure-Rust GPLv3 library (serde, thiserror only in Stage 0)
│   │   └── src/
│   │       ├── lib.rs                 # Public API types & DecoderEngine stub
│   │       ├── telemetry.rs           # DecodeTelemetry, FecMethod
│   │       ├── protocol/{mod.rs,types.rs,pack77.rs,unpack77.rs,hashtable.rs,crc14.rs,ldpc.rs,rvec.rs}
│   │       ├── modulate/{mod.rs,gfsk.rs,ft8.rs,ft4.rs}
│   │       ├── dsp/{mod.rs,fft.rs,window.rs,baseline.rs,twkfreq.rs,downsample.rs}
│   │       ├── fec/{mod.rs,bp.rs,osd.rs,decode174_91.rs}
│   │       ├── ft8/{mod.rs,params.rs,sync8.rs,sync8d.rs,ft8b.rs,ap.rs,subtract.rs,decoder.rs}
│   │       └── ft4/{mod.rs,params.rs,candidates.rs,sync4d.rs,bitmetrics.rs,ap.rs,subtract.rs,decoder.rs}
│   └── ftxr-modem/
│       ├── Cargo.toml                 # GPLv3 standalone CLI + warm worker binary (stub)
│       └── src/main.rs
└── tools/
    └── ftxr-oracle/
        ├── Cargo.toml                 # Dev-only oracle harness (never extracted in Stage 6)
        └── src/
            ├── main.rs                # clap CLI: smoke, generate, run-jt9, baseline, compare
            ├── wav.rs                 # RIFF/BWF 12 kHz mono i16 WAV reader/writer + naming
            ├── jt9.rs                 # Jt9Runner, wisdom manager, FT8(~) + FT4(+) + AP parser
            ├── sim.rs                 # Wrappers for ft8sim, ft4sim, ft4sim_mult, ft8code, ft4code
            ├── synth.rs               # Deterministic ChaCha8Rng + Marsaglia polar + SNR_2500 mixer
            ├── corpus.rs              # Committed manifest + synthetic suite definitions
            ├── scorecard.rs           # Truth matcher, threshold estimator, JSON v1 schema
            └── smoke.rs               # 6-binary build & functional smoke checks
```

### 3.2 Workspace `Cargo.toml` & License Compatibility Matrix

```toml
[workspace]
resolver = "2"
members = [
    "crates/ftxr-core",
    "crates/ftxr-modem",
    "tools/ftxr-oracle",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.75"
license = "GPL-3.0-or-later"
repository = "https://github.com/mpmbq2/ftxr-modem"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
clap = { version = "4", features = ["derive", "env"] }
rand_chacha = "0.3"
rand_core = "0.6"
sha2 = "0.10"
tempfile = "3"
chrono = { version = "0.4", default-features = false, features = ["clock", "std", "serde"] }
```

| Crate | Used By (Stage 0) | License | GPLv3 Compatible? |
| :--- | :--- | :--- | :--- |
| `serde`, `serde_json` | `ftxr-core`, `ftxr-modem`, `ftxr-oracle` | `MIT OR Apache-2.0` | Yes |
| `thiserror` | `ftxr-core`, `ftxr-modem`, `ftxr-oracle` | `MIT OR Apache-2.0` | Yes |
| `clap` | `ftxr-modem`, `ftxr-oracle` | `MIT OR Apache-2.0` | Yes |
| `rand_chacha`, `rand_core` | `ftxr-oracle` | `MIT OR Apache-2.0` | Yes (dev-only) |
| `sha2`, `tempfile`, `chrono` | `ftxr-oracle` | `MIT OR Apache-2.0` | Yes (dev-only) |

*Note:* `ftxr-core` has zero dependency on `wsjtx`, Qt, Fortran, FFTW, `digi-dx`, or any non-GPLv3-compatible code. Unused Numerical Recipes `bessi0` (`lib/ft8/ft8b.f90:509-530`) is explicitly excluded across all stages.

### 3.3 `crates/ftxr-core` Public API Contract (`src/lib.rs` & `src/telemetry.rs`)

```rust
use std::sync::atomic::AtomicBool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod dsp;
pub mod fec;
pub mod ft4;
pub mod ft8;
pub mod modulate;
pub mod protocol;
pub mod telemetry;

pub use telemetry::{DecodeTelemetry, FecMethod};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ft8,
    Ft4,
}

impl Mode {
    pub const SAMPLE_RATE_HZ: u32 = 12_000;

    pub const fn slot_duration_ms(self) -> u32 {
        match self { Self::Ft8 => 15_000, Self::Ft4 => 7_500 }
    }
    pub const fn rx_window_samples(self) -> usize {
        match self { Self::Ft8 => ft8::params::NMAX, Self::Ft4 => ft4::params::NMAX }
    }
    pub const fn nominal_tx_delay_ms(self) -> u32 {
        match self { Self::Ft8 => 500, Self::Ft4 => 300 }
    }
    pub const fn jt9_mode_marker(self) -> char {
        match self { Self::Ft8 => '~', Self::Ft4 => '+' }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum ContestMode {
    #[default]
    None = 0,
    NaVhf = 1,
    EuVhf = 2,
    FieldDay = 3,
    RttyRoundup = 4,
    WwDigi = 5,
    Fox = 6,
    Hound = 7,
    ArrlDigi = 8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QsoContext {
    pub my_call: Option<String>,
    pub his_call: Option<String>,
    pub his_grid: Option<String>,
    /// WSJT-X `nQSOProgress` (`0..=5`: 0=CALLING/CQ, 1=REPORT, 2=ROGER_REPORT, 3=ROGERS, 4=SIGNOFF, 5=CALLING_73).
    pub qso_progress: u8,
    pub rx_freq_hz: f32,
    pub tx_freq_hz: f32,
    pub ap_enabled: bool,
    pub ap_cq_only: bool,
    /// AP frequency window around `rx_freq_hz`/`tx_freq_hz` (`75.0` Hz FT8, `50.0` Hz FT4).
    pub ap_width_hz: f32,
    pub contest: ContestMode,
}

impl Default for QsoContext {
    fn default() -> Self {
        Self {
            my_call: None,
            his_call: None,
            his_grid: None,
            qso_progress: 0,
            rx_freq_hz: 1500.0,
            tx_freq_hz: 1500.0,
            ap_enabled: false,
            ap_cq_only: false,
            ap_width_hz: 75.0,
            contest: ContestMode::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeConfig {
    pub mode: Mode,
    /// Decoding depth: `1` (Fast/BP), `2` (Normal), `3` (Deep/BP+OSD).
    pub depth: u8,
    pub freq_min_hz: f32,
    pub freq_max_hz: f32,
    pub rx_freq_hz: f32,
    pub freq_tol_hz: f32,
    pub qso: Option<QsoContext>,
    pub enable_telemetry: bool,
    pub max_threads: Option<usize>,
}

impl DecodeConfig {
    pub fn for_mode(mode: Mode) -> Self {
        Self {
            mode,
            depth: 3,
            freq_min_hz: 200.0,
            freq_max_hz: 4000.0,
            rx_freq_hz: 1500.0,
            freq_tol_hz: 20.0,
            qso: None,
            enable_telemetry: false,
            max_threads: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApTag {
    #[default]
    None,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
    /// Cross-slot AP (emitted by `jt9` oracle; deferred in `ftxr-core` Stage 3).
    A7,
    /// RX-freq list AP (emitted by `jt9` oracle; deferred in `ftxr-core` Stage 3).
    A8,
}

impl ApTag {
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::A1 => Some("a1"),
            Self::A2 => Some("a2"),
            Self::A3 => Some("a3"),
            Self::A4 => Some("a4"),
            Self::A5 => Some("a5"),
            Self::A6 => Some("a6"),
            Self::A7 => Some("a7"),
            Self::A8 => Some("a8"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedMessage {
    pub mode: Mode,
    pub snr_db: i8,
    pub snr_raw: f32,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub message: String,
    pub ap_tag: ApTag,
    /// True when WSJT-X quality threshold (`qual < 0.17`) flags AP decode as uncertain (`? aN`).
    pub ap_uncertain: bool,
    pub telemetry: Option<DecodeTelemetry>,
}

impl DecodedMessage {
    /// Formats WSJT-X / `digi-dx` `SlotDecodeReconciler`-compatible `raw_metadata` (`None`, `"a1"`, `"? a1"`).
    pub fn wsjtx_metadata(&self) -> Option<String> {
        let tag = self.ap_tag.as_str()?;
        if self.ap_uncertain {
            Some(format!("? {tag}"))
        } else {
            Some(tag.to_string())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FecMethod {
    BeliefPropagation,
    Osd { order: u8 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeTelemetry {
    pub fec_method: FecMethod,
    pub sync_score: f32,
    pub subtraction_pass: u8,
    pub hard_errors: u16,
    pub osd_distance: f32,
    pub quality: f32,
    pub freq_drift_hz: Option<f32>,
    pub qsb_envelope: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModulateConfig {
    pub mode: Mode,
    pub frequency_hz: f32,
    pub sample_rate_hz: u32,
    /// Nominal slot start delay in ms (`None` -> 500 ms FT8 / 300 ms FT4; `Some(0)` -> raw active waveform).
    pub start_delay_ms: Option<u32>,
    /// UTC-aligned late-TX offset into active transmission (ms).
    pub late_start_ms: u32,
    /// Click-free fade-in duration for late-TX starts (ms).
    pub fade_in_ms: f32,
    /// Output peak amplitude in `(0.0, 1.0]`.
    pub amplitude: f32,
}

impl ModulateConfig {
    pub fn for_mode(mode: Mode, frequency_hz: f32) -> Self {
        Self {
            mode,
            frequency_hz,
            sample_rate_hz: Mode::SAMPLE_RATE_HZ,
            start_delay_ms: None,
            late_start_ms: 0,
            fade_in_ms: 10.0,
            amplitude: 1.0,
        }
    }
}

#[derive(Debug, Error)]
pub enum FtxrError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
    #[error("invalid message: {0}")]
    InvalidMessage(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("unsupported sample rate: {0} Hz")]
    InvalidSampleRate(u32),
    #[error("invalid audio length: got {actual} samples, expected at least {expected_min}")]
    InvalidAudioLength { expected_min: usize, actual: usize },
    #[error("operation cancelled")]
    Cancelled,
    #[error("internal modem error: {0}")]
    Internal(String),
}

#[derive(Debug, Default)]
pub struct DecoderEngine {
    _private: (), // Holds session CallsignHashTable & warm FFT plans in Stages 1–5
}

impl DecoderEngine {
    pub fn new() -> Self { Self { _private: () } }
    pub fn reset_session(&mut self) {}

    pub fn decode(
        &mut self,
        samples_12k: &[f32],
        cfg: &DecodeConfig,
    ) -> Result<Vec<DecodedMessage>, FtxrError> {
        let cancel = AtomicBool::new(false);
        self.decode_streaming(samples_12k, cfg, &cancel, |_| {})
    }

    pub fn decode_streaming<F>(
        &mut self,
        _samples_12k: &[f32],
        _cfg: &DecodeConfig,
        _cancel: &AtomicBool,
        _on_decode: F,
    ) -> Result<Vec<DecodedMessage>, FtxrError>
    where
        F: FnMut(DecodedMessage),
    {
        Err(FtxrError::Unimplemented("DecoderEngine::decode_streaming (Stage 2/4)"))
    }

    pub fn modulate(
        &mut self,
        _message: &str,
        _cfg: &ModulateConfig,
    ) -> Result<Vec<f32>, FtxrError> {
        Err(FtxrError::Unimplemented("DecoderEngine::modulate (Stage 1)"))
    }
}
```

### 3.4 `tools/ftxr-oracle` Design

#### CLI Subcommands (`tools/ftxr-oracle/src/main.rs`)
Global flags:
- `--oracle-bin-dir <PATH>` (env `FTXR_ORACLE_BIN_DIR`, default `<workspace>/rust-ftx/.oracle-build`)
- `--cache-dir <PATH>` (env `FTXR_ORACLE_CACHE_DIR`, default `<workspace>/rust-ftx/.oracle-cache`)
- `--jobs <N>` (default `1` for timing runs, `<= num_cpus` for corpus generation)

Subcommands:
1. **`ftxr-oracle smoke`**
   Verifies all 6 binaries exist, execute cleanly, and pass functional checks (`jt9` FT8 & FT4 samples, `ft8code -T`, `ft4code -t`, `ft8sim -> jt9 -8`, `ft4sim -> jt9 -5`, `ft4sim_mult -> jt9 -5`).
2. **`ftxr-oracle generate [--suite <NAME>] [--trials <N>] [--seed-version <v1>]`**
   Generates deterministic synthetic WAV suites + `manifest.json` under `.oracle-cache/corpus/<seed-version>/`. Suites: `all`, `ft8_awgn`, `ft4_awgn`, `ft8_crowded`, `ft4_crowded`, `ft8_offsets`, `ft4_offsets`, `ft8_late_tx`, `ft4_late_tx`, `ft8_noise`, `ft4_noise`, `ft8_ap_qso`, `ft4_ap_qso`. Default `--trials 20` (fast baseline; `--trials 100` for high-precision cliff curves).
3. **`ftxr-oracle run-jt9 --mode <ft8|ft4> --depth <1|2|3> --profile <blind-no-ap|blind-cq-ap|qso-ap> [--patience <0|1>] [--my-call <C>] [--his-call <C>] [--his-grid <G>] [--qso-progress <0..5>] [--rx-freq <HZ>] <WAV>`**
   Invokes a single isolated `jt9` subprocess, returns parsed JSON decodes + elapsed ms + `<DecodeFinished>` stats.
4. **`ftxr-oracle baseline [--suite <all|committed|synthetic>] [--depths 1,2,3] [--profiles blind-no-ap,blind-cq-ap,qso-ap] [--patience 1] [--out scorecards/jt9-baseline-v1.json]`**
   Pre-warms `jt9_wisdom.dat` once, executes `jt9` across committed fixtures + synthetic suites, computes metrics, and writes the versioned JSON scorecard.
5. **`ftxr-oracle compare --baseline <A.json> --candidate <B.json> [--max-threshold-delta-db 0.5] [--min-decode-ratio 0.95]`**
   Compares two scorecards (used in Stage 0 for repeatability self-check `baseline vs baseline2` and in Stages 2–5 for `ftxr-core` vs `jt9` parity gates).

#### `jt9` Invocation Profiles & Concurrency Safety (`src/jt9.rs`)
- **Wisdom Isolation:** Before running any batch with `-w 1`, `Jt9Runner::ensure_warm_wisdom()` runs a single warm-up decode of FT8 and FT4 into `.oracle-cache/wisdom/jt9_wisdom.dat`. Every individual `jt9` invocation receives its own `TempDir` (`data_dir = tmp/data`, `temp_dir = tmp/temp`) with `jt9_wisdom.dat` copied into `tmp/data/` before spawn. This eliminates parallel clobbering of `jt9_wisdom.dat`, `timer.out`, and `decoded.txt`!
- **Process Environment:** Always sets `OMP_NUM_THREADS=1`, passes `-m 1`, `-L 200`, `-H 4000`, and passes **exactly one** WAV file per `jt9` invocation (preventing cross-slot `ft8_a7` state leakage).
- **Standardized Profile Flags:**
  - `blind-no-ap`: `-c b -x b -Q 1` (blanks `mycall`/`hiscall`/`hisgrid` AND sets `nQSOProgress=1` so `a1` is also disabled -> pure non-AP decode).
  - `blind-cq-ap`: `-c b -x b -Q 0` (blanks `mycall`/`hiscall`/`hisgrid`, leaves `a1` CQ-AP active, matching WSJT-X default idle listening).
  - `qso-ap`: `-c <mycall> -x <hiscall> -g . -Q <0..5> -f <rx_freq>` (enables `a1`–`a6` while `-g .` sets `len_trim(hisgrid)=1 < 4` to suppress `a8`; optional `--allow-a8` passes real 4-char `hisgrid`).
- **Stdout Parser (`parse_jt9_stdout`):**
  - Recognizes `<DecodeFinished>` (`nsynced`, `ndecoded`, `navg0`) and ignores `EOF on input file ...` and blank lines.
  - Parses each decode line by splitting the 4 leading numeric fields `(utc, snr, dt, af_hz)` and matching the 5th token as `"~"` (`Mode::Ft8`) or `"+"` (`Mode::Ft4`).
  - Slices the message+metadata substring immediately after the mode marker (`" ~ "` or `" + "`), inspects the trailing whitespace-separated token against `^a[1-8]$`:
    - If matched -> extracts `ap_tag = ApTag::A1..A8`, trims right, checks if remaining payload ends with whitespace + `?` (or column-37 `?`) -> strips `?` and sets `ap_uncertain = true`.
    - Otherwise -> `ap_tag = ApTag::None`, `ap_uncertain = false` (preserving literal `?` in free-text messages).
  - Collapses internal whitespace runs in `message` to single spaces.

#### Deterministic Synthetic Generator (`src/synth.rs`)
- **Seed Derivation:** `seed_bytes = SHA256("ftxr-synth-v1|" + suite + "|" + mode + "|" + case_id + "|" + trial_idx)`. Feed all 32 bytes into `ChaCha8Rng::from_seed(seed_bytes)`. Zero OS entropy; identical output on Linux/macOS and x86_64/aarch64.
- **Gaussian Noise (`marsaglia_polar_f64`):** Implements the exact polar Box-Muller algorithm from `lib/gran.c:19-27` using `u = 2.0 * next_unit_f64() - 1.0` from `ChaCha8Rng`, returning `f64` samples with exact mean `0.0` and variance `1.0`.
- **Clean Template Extraction (`src/sim.rs`):**
  - Invokes `ft8sim "<msg>" <f0> 0.0 0.0 0.0 1 99` or `ft4sim "<msg>" <f0> 0.0 0.0 0.0 1 99` inside a `TempDir`.
  - Reads `000000_000001.wav`, converts `i16` to `f64` by dividing by `32766.9`, and verifies active-interval RMS is within `1/sqrt(2) ± 0.01`, rescaling by `(1/sqrt(2)) / rms_active` so $P_{\text{unit}} = 0.5$ exactly.
  - Caches clean templates by `SHA256(mode|msg|f0_millihz)` under `.oracle-cache/templates/`.
- **Linear Shifting, Superposition & Scaling:**
  - Applies linear shift `delta_samples = (dt_sec * 12000.0).round() as isize` with zero-padding and truncation to `[0, NMAX)` (no circular `cshift` wrap).
  - For late-TX truncation tests (`late_start_ms`), zeroes the first `late_start_samples` of the active signal (or shifts start time forward by `late_start_ms`, truncating at slot boundary) and applies a 10 ms raised-cosine fade-in.
  - For partial-slot RX tests (`truncate_ms`), zeroes all samples (signal + noise) at indices `>= truncate_samples` (matching `jt9`/`digi-dx` partial pass buffers).
  - Mixes `wave[i] = sum_k(sig_k * sig_k_wave[i]) + noise[i]` where `sig_k = (2.0 * 2500.0 / 6000.0).sqrt() * 10_f64.powf(0.05 * snr_db_k)`.
  - Scales by `gain = 100.0` (single-signal / pure-noise) or `gain = 100.0_f64.min(30_000.0 / peak)` (multi-signal crowded slots, asserting `rms >= 15.0 >> 0.5`), rounds half-away-from-zero (`round()`) to `i16`, and writes a 44-byte 12 kHz mono PCM WAV named `000000_<6-digit-id>.wav`.

#### Golden Corpus Suite Matrix
| Suite | Location | Mode(s) | Cases / Trials | Purpose |
| :--- | :--- | :--- | :--- | :--- |
| `committed_real` | `samples/FT8/210703_133430.wav`, `samples/FT4/000000_000002.wav`, `fixtures/committed/ft8/191111_110200.wav` | FT8, FT4 | 3 real-world slots | Real HF off-air benchmark |
| `committed_reg` | `fixtures/committed/{ft8,ft4}/000000_000*.wav` | FT8, FT4 | 4 compact slots (~1.2 MB) | Crowded 20+ dB FT8/FT4, Watterson `MM` fading, +5 s late TX |
| `ft8_awgn` | `.oracle-cache/` (generated) | FT8 | `0..-24 dB` (1 dB step) × `20` (or `100`) | 50% decode threshold curve |
| `ft4_awgn` | `.oracle-cache/` (generated) | FT4 | `0..-21 dB` (1 dB step) × `20` (or `100`) | 50% decode threshold curve |
| `ft8_crowded` / `ft4_crowded` | `.oracle-cache/` (generated) | FT8, FT4 | 5 slots × 10–12 signals (`+2` to `-20`/`-18 dB`, ≥20 dB DR, tight 50–80 Hz / 90–140 Hz spacing) | Multi-pass subtraction stress |
| `ft8_offsets` / `ft4_offsets` | `.oracle-cache/` (generated) | FT8, FT4 | Grid of DT (`-2.0..+2.0 s` FT8; `-0.5..+0.75 s` FT4) × AF (`250..3500 Hz`) at `-10` & `-16 dB` | Time/frequency acquisition bounds |
| `ft8_late_tx` / `ft4_late_tx` | `.oracle-cache/` (generated) | FT8, FT4 | Late TX (`+1..+5 s` FT8; `+0.5..+1.5 s` FT4) + partial RX slots (`11.8/13.5/14.4/14.8 s` FT8; `5.6/6.048 s` FT4) | Late-start & 3-pass cadence |
| `ft8_noise` / `ft4_noise` | `.oracle-cache/` (generated) | FT8, FT4 | `100` (fast) / `500` (full) pure AWGN slots | False-positive rate (blind & AP) |
| `ft8_ap_qso` / `ft4_ap_qso` | `.oracle-cache/` (generated) | FT8, FT4 | `QSOProgress 0..5` × `a1`–`a6` messages (`N0AAA`/`K9BBB`/`DM79`), `-18..-24 dB` FT8 / `-15..-21 dB` FT4 | AP `a1`–`a6` gain & `?` calibration |

*Synthetic Callsign Hygiene:* All synthetic messages use neutral callsigns (`N0AAA`, `K9BBB`, `AA1AA`, `BB2BB`, `VE3CCC`, `DL4DDD`) — **never** `K1ABC`/`W9XYZ`/`EN37` — to guarantee zero accidental overlap with `jt9.f90` defaults.

#### JSON Baseline Scorecard Schema (`scorecards/jt9-baseline-v1.json`)

```json
{
  "schema_version": 1,
  "generated_at_utc": "2026-11-24T22:30:00Z",
  "host": {
    "os": "Ubuntu 24.04.5 LTS",
    "arch": "x86_64",
    "cpu_count": 8,
    "rustc_version": "rustc 1.100.0"
  },
  "engine": {
    "name": "jt9",
    "wsjtx_git_rev": "<sha>",
    "wsjtx_version": "3.0.2.0",
    "fftw_patience": 1,
    "wisdom_warm": true,
    "omp_num_threads": 1
  },
  "runs": [
    {
      "mode": "ft8",
      "depth": 3,
      "profile": "blind-no-ap",
      "cli_template": "jt9 -8 -d 3 -w 1 -m 1 -L 200 -H 4000 -f 1500 -F 20 -c b -x b -Q 1 ...",
      "committed_fixtures": [
        {
          "fixture_id": "wsjtx_sample_ft8_210703_133430",
          "sha256": "<hex>",
          "elapsed_ms": 412.5,
          "decode_count": 35,
          "ap_counts": { "none": 35, "a1": 0, "a2": 0, "a3": 0, "a4": 0, "a5": 0, "a6": 0, "a7": 0, "a8": 0 },
          "uncertain_ap_count": 0,
          "decodes": [
            { "snr_db": -12, "dt_sec": 0.4, "af_hz": 1234.0, "message": "CQ ...", "ap_tag": "none", "ap_uncertain": false }
          ]
        }
      ],
      "awgn_sweep": {
        "trials_per_snr": 20,
        "threshold_50pct_db": -20.8,
        "points": [
          {
            "snr_db": -20.0,
            "trials": 20,
            "decoded": 16,
            "false_positives": 0,
            "decode_rate": 0.80,
            "mean_snr_err_db": 0.4,
            "mean_abs_af_err_hz": 0.7,
            "mean_abs_dt_err_sec": 0.04,
            "median_elapsed_ms": 185.2
          }
        ]
      },
      "crowded": {
        "total_truth_signals": 50,
        "true_positives": 47,
        "false_positives": 0,
        "recall": 0.94,
        "median_elapsed_ms": 390.1
      },
      "pure_noise": {
        "slots": 100,
        "false_decodes_total": 0,
        "false_decodes_verified": 0,
        "false_decodes_ap_certain": 0,
        "false_decodes_ap_uncertain": 0,
        "fp_rate_per_slot": 0.0
      },
      "timing_ms": { "min": 92.1, "median": 188.4, "p95": 420.3, "mean": 211.7 }
    }
  ]
}
```

---

## 4. Ordered Implementation Tasks

### Task 0.1 — Install Headless WSJT-X Build Dependencies (Ubuntu 24.04)
- **Command:**
  ```bash
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends \
    build-essential cmake ninja-build pkg-config gfortran \
    libfftw3-dev libboost-dev libboost-log-dev \
    libhamlib-dev libusb-1.0-0-dev libudev-dev \
    qtbase5-dev qttools5-dev qttools5-dev-tools \
    qtmultimedia5-dev libqt5serialport5-dev libqt5websockets5-dev
  ```
- **Acceptance Criteria:** `gfortran --version`, `pkg-config --modversion fftw3f hamlib libusb-1.0`, and `qmake -version` all succeed.

### Task 0.2 — Configure & Selectively Build the 6 WSJT-X Oracle Targets
- **Commands:**
  ```bash
  cmake -S /home/antigravity/mpmbq2/wsjtx \
    -B /home/antigravity/mpmbq2/wsjtx/rust-ftx/.oracle-build \
    -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DWSJT_SKIP_MAP65=ON \
    -DWSJT_GENERATE_DOCS=OFF \
    -DWSJT_SKIP_MANPAGES=ON \
    -DWSJT_BUILD_TESTS=OFF \
    -DWSJT_BUILD_UTILS=ON \
    -DWSJT_SKIP_BUNDLE_FIXUP=ON \
    -Wno-dev

  cmake --build /home/antigravity/mpmbq2/wsjtx/rust-ftx/.oracle-build \
    --target jt9 ft8code ft4code ft8sim ft4sim ft4sim_mult \
    -j"$(nproc)"
  ```
- **Acceptance Criteria:** All 6 executables exist and are executable in `rust-ftx/.oracle-build/`; `git -C /home/antigravity/mpmbq2/wsjtx status` shows zero modified upstream WSJT-X files.

### Task 0.3 — Manual Oracle Smoke Verification
- **Commands (run inside `mktemp -d` so CWD stays clean):**
  ```bash
  BIN=/home/antigravity/mpmbq2/wsjtx/rust-ftx/.oracle-build
  TMP=$(mktemp -d)
  "$BIN/ft8code" -T | tail -n 5
  "$BIN/ft8code" "CQ N0AAA EM10"
  "$BIN/ft4code" -t | tail -n 5
  "$BIN/ft4code" "CQ N0AAA EM10"
  (cd "$TMP" && "$BIN/ft8sim" "CQ N0AAA EM10" 1500.0 0.0 0.0 0.0 1 -10 \
    && OMP_NUM_THREADS=1 "$BIN/jt9" -8 -d 1 -w 0 -m 1 -c b -x b -Q 1 -a "$TMP" -t "$TMP" "$TMP/000000_000001.wav")
  (cd "$TMP" && "$BIN/ft4sim" "CQ N0AAA EM10" 1500.0 0.0 0.0 0.0 1 -10 \
    && OMP_NUM_THREADS=1 "$BIN/jt9" -5 -d 1 -w 0 -m 1 -c b -x b -Q 1 -a "$TMP" -t "$TMP" "$TMP/000000_000001.wav")
  (cd "$TMP" && cp /home/antigravity/mpmbq2/wsjtx/lib/ft4/messages.txt . \
    && "$BIN/ft4sim_mult" 20 1 \
    && OMP_NUM_THREADS=1 "$BIN/jt9" -5 -d 3 -w 0 -m 1 -c b -x b -Q 1 -a "$TMP" -t "$TMP" "$TMP/000000_000001.wav")
  OMP_NUM_THREADS=1 "$BIN/jt9" -8 -d 3 -w 0 -m 1 -c b -x b -Q 1 -a "$TMP" -t "$TMP" /home/antigravity/mpmbq2/wsjtx/samples/FT8/210703_133430.wav
  OMP_NUM_THREADS=1 "$BIN/jt9" -5 -d 3 -w 0 -m 1 -c b -x b -Q 1 -a "$TMP" -t "$TMP" /home/antigravity/mpmbq2/wsjtx/samples/FT4/000000_000002.wav
  rm -rf "$TMP"
  ```
- **Acceptance Criteria:** `ft8sim`/`ft4sim` round-trip decodes recover `"CQ N0AAA EM10"` at `~1500 Hz`, `DT ≈ 0.0 s`; `ft4sim_mult` produces multiple `+` decodes; real FT8 & FT4 samples produce non-empty decodes followed by `<DecodeFinished>`.

### Task 0.4 — Scaffold GPLv3 Workspace, Licensing & `.gitignore`
- Create `rust-ftx/LICENSE` (copy of `wsjtx/COPYING`), `rust-ftx/NOTICE` (WSJT-X copyright 2001–2026 attribution citing `wsjtx/widgets/about.cpp` & `wsjtx/COPYING`, GPLv3 §5 modification notice, and explicit exclusion of Numerical Recipes `bessi0`), `rust-ftx/README.md`, `rust-ftx/docs/ipc-protocol.md` (Stage 5 placeholder), and `rust-ftx/.gitignore`:
  ```gitignore
  /target/
  /.oracle-build/
  /.oracle-cache/
  000000_*.wav
  !fixtures/committed/**/*.wav
  jt9_wisdom.dat
  timer.out
  decoded.txt
  *.tmp
  ```
- Create `rust-ftx/Cargo.toml` with the workspace definition from Section 3.2.
- **Acceptance Criteria:** `git check-ignore -v rust-ftx/.oracle-build rust-ftx/.oracle-cache rust-ftx/target rust-ftx/000000_000001.wav` confirms all generated artifacts are ignored while `rust-ftx/fixtures/committed/ft8/000000_000101.wav` is **not** ignored.

### Task 0.5 — Scaffold `crates/ftxr-core` and `crates/ftxr-modem` Stubs
- Create all modules listed in Section 3.1 under `crates/ftxr-core/src/` with the exact public API from Section 3.3.
- Populate verified protocol constants in `ft8/params.rs` and `ft4/params.rs` (from Section 2.4, plus Costas arrays `ICOS7 = [3,1,4,0,6,5,2]` and FT4 `ICOS4_A..D`, plus `RVEC` in `protocol/rvec.rs` from `lib/ft4/ft4code.f90:17-19`).
- Create `crates/ftxr-modem/src/main.rs` with stub `clap` CLI (`decode`, `encode`, `--worker`, `--version`) printing GPLv3 copyright notice on `--version` and returning exit code 2 (`not yet implemented`) for unimplemented Stage 5 commands.
- **Acceptance Criteria:** `cargo check --workspace` and `cargo test -p ftxr-core -p ftxr-modem` pass with unit tests covering `Mode` constants, `ApTag::wsjtx_metadata()` formatting (`None`, `"a1"`, `"? a1"`), and `QsoContext`/`DecodeConfig` defaults.

### Task 0.6 — Implement `tools/ftxr-oracle` (WAV I/O, `jt9` Runner & Parser, Sim Wrappers)
- Implement `wav.rs`: 12 kHz mono 16-bit PCM reader handling arbitrary RIFF/BWF chunks (`fmt `, `bext`, `junk`, `data` with even-byte padding matching `lib/readwav.f90:60-70`) and standard 44-byte WAV writer matching `lib/wavhdr.f90:20-36`.
- Implement `jt9.rs`: `Jt9Runner`, wisdom pre-warmer, per-call isolated temp dir (`-a`/`-t`), `OMP_NUM_THREADS=1`, timeout handling, and strict `parse_jt9_stdout()` handling FT8 `~`, FT4 `+`, `a1`..`a8`, `? a1`..`? a8`, free-text `?`, FT4 `+05` reports, `<DecodeFinished>`, and `EOF on input file`.
- Implement `sim.rs` and `smoke.rs`: TempDir-isolated wrappers for `ft8code`, `ft4code`, `ft8sim`, `ft4sim`, `ft4sim_mult`.
- **Acceptance Criteria:** Unit tests pass for every `parse_jt9_line` edge case; `cargo run -p ftxr-oracle -- smoke` passes all 6 binary checks.

### Task 0.7 — Implement Deterministic Synthetic Generator & Committed Golden Fixtures
- Implement `synth.rs` and `corpus.rs` per Section 3.4.
- Copy read-only `/home/antigravity/mpmbq2/digi-dx/tests/fixtures/191111_110200.{wav,txt}` into `rust-ftx/fixtures/committed/ft8/`.
- Generate and freeze the 4 compact regression WAVs (`ft8_crowded_20db`, `ft8_fading_mm`, `ft8_late_tx_5s`, `ft4_crowded_20db`) into `rust-ftx/fixtures/committed/{ft8,ft4}/` and write `rust-ftx/fixtures/committed/manifest.json` with SHA-256 checksums, ground-truth signal lists, and relative links to `../../samples/FT8/210703_133430.wav` and `../../samples/FT4/000000_000002.wav`.
- **Acceptance Criteria:** Total size of `rust-ftx/fixtures/committed/` is `< 2.0 MB`; running `ftxr-oracle generate` twice into two directories produces byte-for-byte identical WAVs (`diff -r` empty); empirical `SNR_2500` calibration check passes within `±0.05 dB` mean error across 20 trials.

### Task 0.8 — Capture & Commit Baseline `jt9` Scorecard (`scorecards/jt9-baseline-v1.json`)
- Implement `scorecard.rs` (truth matching, 50% threshold interpolation, timing percentiles, JSON v1 serialization, `compare` subcommand).
- Run `ftxr-oracle generate --suite all --trials 20` and `ftxr-oracle baseline --suite all --depths 1,2,3 --profiles blind-no-ap,blind-cq-ap,qso-ap --patience 1 --out rust-ftx/scorecards/jt9-baseline-v1.json`.
- Run a second pass on committed fixtures + `ft8_awgn` (`-d 3`) into `/tmp/check.json` and verify `ftxr-oracle compare` reports `0` decode-set delta (100% deterministic repeatability).
- **Acceptance Criteria:** `rust-ftx/scorecards/jt9-baseline-v1.json` validates against the schema in Section 3.4 and contains non-empty baseline curves for FT8 (`-d 1/2/3`) and FT4 (`-d 1/2/3`).

---

## 5. Verification, Oracle Testing & Numerical Gates

### 5.1 Unit Tests (`cargo test --workspace`)
1. **`ftxr-core` contract tests:**
   - `ft8::params` & `ft4::params` constant invariants (`NZ == NN * NSPS`, `NMAX == 180_000` FT8 / `72_576` FT4, `RVEC.len() == 77`).
   - `DecodedMessage::wsjtx_metadata()` maps `(ApTag::None, false) -> None`, `(ApTag::A1, false) -> Some("a1")`, `(ApTag::A3, true) -> Some("? a3")`.
2. **`ftxr-oracle` WAV & parser unit tests:**
   - BWF header skip: parses `samples/FT8/210703_133430.wav` (360,246 B -> 180,000 samples) and `191111_110200.wav` (360,202 B -> 180,000 samples) and `samples/FT4/000000_000002.wav` (145,196 B -> 72,576 samples).
   - `parse_jt9_line` golden table:
     - FT8 verified: `133430  -4  0.7  683 ~  CQ TA6CQ KN70                         ` -> `Mode::Ft8`, `ap_tag=None`, `ap_uncertain=false`.
     - FT8 certain AP: `110200 -14  0.6 1030 ~  CQ LZ1JZ KN22                         a1` -> `ap_tag=A1`, `ap_uncertain=false`.
     - FT8 uncertain AP: `110115 -22  0.3 1993 ~  CQ PY7XC HI21                       ? a1` -> `ap_tag=A1`, `ap_uncertain=true`.
     - FT8 `a7`/`a8`: `110115 -24  0.2 1500 ~  N0AAA K9BBB DM79                    ? a8` -> `ap_tag=A8`, `ap_uncertain=true`.
     - FT8 free text with literal `?`: `110115  -5  0.1 1500 ~  PSE QSL?                              ` -> `message="PSE QSL?"`, `ap_tag=None`, `ap_uncertain=false`.
     - FT4 verified with `+` in report: `000002  +3  0.1 1240 +  K1ABC W9XYZ R+05                      ` (and `000002   3  0.1 1240 +  K1ABC W9XYZ R+05                      `) -> `Mode::Ft4`, `message="K1ABC W9XYZ R+05"`, `ap_tag=None`.
     - FT4 uncertain AP: `000002 -18  0.2 1500 +  N0AAA K9BBB RR73                    ? a6` -> `Mode::Ft4`, `ap_tag=A6`, `ap_uncertain=true`.
     - Non-decode lines: `<DecodeFinished>   0   5        0`, `EOF on input file 000000_000001.wav`, empty line -> ignored (with `<DecodeFinished>` count captured).
3. **`ftxr-oracle` synthetic calibration unit tests:**
   - Bit-exact determinism: two `generate_slot()` calls with identical seed produce identical `Vec<i16>` (SHA-256 match).
   - `SNR_2500` empirical accuracy: across 20 trials at `-10 dB` and `-20 dB`, measured `mean(snr_2500_empirical - snr_db_target)` is within `±0.05 dB` (and noise std dev within `1.00 ± 0.01`).
   - Linear shift check: FT8 signal shifted `+3.0 s` has zero energy in samples `0..42_000` (confirming no `cshift` wraparound into the start of the buffer).

### 5.2 Oracle Functional & Scorecard Gates
| Gate | Command / Check | Pass Criterion |
| :--- | :--- | :--- |
| 6-binary smoke | `cargo run -p ftxr-oracle -- smoke` | Exit 0; all 6 tools pass round-trip & fixture checks |
| Real-world FT8 sanity | `jt9 -8 -d 3 -c b -x b -Q 1` on `191111_110200.wav` and `210703_133430.wav` | `191111_110200.wav` recovers all 5 reference messages in `191111_110200.txt`; `210703_133430.wav` recovers ≥20 decodes |
| Real-world FT4 sanity | `jt9 -5 -d 3 -c b -x b -Q 1` on `000000_000002.wav` | Recovers ≥10 `+` decodes with zero parser errors |
| Monotonic depth sanity | Baseline scorecard `runs` | For both FT8 and FT4 on `awgn_sweep` & `crowded`, `decode_count(d=3) >= decode_count(d=2) >= decode_count(d=1)` and `threshold_50pct_db(d=3) <= threshold_50pct_db(d=1)` |
| AP vs. blind sanity | `ft8_ap_qso` (`qso-ap` vs `blind-no-ap`) | `qso-ap` emits `a1`–`a6` tags and decodes strictly deeper than `blind-no-ap`; `blind-no-ap` emits **zero** AP tags (`a1`–`a8` all 0) |
| Repeatability | `ftxr-oracle compare` on two consecutive baseline runs | 100% identical decode counts & messages across all fixtures and sweeps |
| Clean git & license check | `git status` + `cargo metadata` license inspection | Zero upstream WSJT-X modifications; zero GPL-incompatible dependencies; `fixtures/committed/` < 2 MB |

---

## 6. Risks, Gotchas & Cross-Stage Handoff

### Top Codebase Gotchas Verified in Source
1. **`jt9 -Q` default mismatch (`lib/jt9.f90:30` vs `:95`) & `a1` leakage:** Help text claims `default PROGRESS=1`, but code sets `nQSOProg=0`. With `lft8apon=.true.` hardcoded, `-c b -x b` alone disables `a2`–`a6`/`a8` while leaving `a1` (`CQ`) active! True blind non-AP evaluation requires `-c b -x b -Q 1`.
2. **`jt9` `hisgrid='EN37'` default triggers `a8` (`lib/jt9.f90:112`, `lib/ft8_decode.f90:289`):** Setting `-c`/`-x` without `-g` leaves `hisgrid='EN37'` (len 4) and activates `ft8_a8d`. Pass `-g .` when benchmarking Stage 3's `a1`–`a6` AP scope without `a8`.
3. **Classic `jt9 -8` disk-WAV `nzhsym=41/47/50` truncates at 14.4 s (`lib/jt9.f90:424-443`):** Samples `172,801..180,000` (last 0.60 s) are zeroed on disk WAVs, and multi-file CLI calls leak `a7` state across slots (`ft8_decode.f90:88-100`). Always run one WAV per `jt9` process and account for the 14.4 s ceiling on extreme late-TX tests.
4. **Release `-fbounds-check` filename crash (`CMakeLists.txt:1185`, `lib/jt9.f90:286-292`):** Short WAV names (<6 chars before `.wav`) crash `jt9`. Always name WAVs `YYMMDD_HHMMSS.wav`.
5. **`ft8sim.f90:129` circular `cshift` & CWD clutter:** `ft8sim` wraps shifts outside `[-0.5, +1.86] s` circularly, and all 3 simulators seed from `/dev/urandom` (`init_random_seed.c`) and dump `000000_*.wav` into CWD. `ftxr-oracle` extracts clean noiseless templates (`snrdb=99`) in a `TempDir` and performs linear shifting + deterministic `ChaCha8Rng` AWGN in Rust.
6. **CMake headless Release traps (`manpages/CMakeLists.txt:19`, `doc/CMakeLists.txt`):** Omitting `-DWSJT_SKIP_MANPAGES=ON` fails Release configure when `a2x` is absent; omitting `-DWSJT_GENERATE_DOCS=OFF` requires `asciidoctor`; omitting `-DWSJT_SKIP_MAP65=ON` requires `portaudio19-dev`.

### Cross-Stage Handoff Contract
- **To Stage 1 (77-Bit Codec, FEC & TX Modulator):**
  - Consumes `ftxr-core` workspace scaffolding, `ft8/params.rs`, `ft4/params.rs`, `protocol/rvec.rs`, `ModulateConfig`, `FtxrError`, and `ftxr-oracle`'s `ft8code`/`ft4code`/`jt9` runners for bit-exact differential fuzzing and TX-to-`jt9` loopback tests (nominal + late-TX).
- **To Stages 2–4 (FT8 MVP, FT8 Weak-Signal Parity, FT4 Parity):**
  - Consumes `DecoderEngine`, `DecodeConfig`, `QsoContext`, `ApTag`, `DecodedMessage`, `DecodeTelemetry`, the deterministic corpus generator (`ftxr-oracle generate`), the committed golden fixtures, and `scorecards/jt9-baseline-v1.json` + `ftxr-oracle compare` for all `-d 1`/`-d 3` FT8 and `-5` FT4 numerical parity gates.
  - Note that `ftxr-core::DecodeConfig` and `DecodedMessage::wsjtx_metadata()` are already shaped for Stage 5 IPC streaming and Stage 7 `digi-dx` `SlotDecodeReconciler` parity (`None`, `"a1"`..`"a6"`, `"? a1"`..`"? a6"`).
- **To Stage 5 (`ftxr-modem` CLI + Warm Worker):**
  - Builds directly on `crates/ftxr-modem` and `docs/ipc-protocol.md` already placed inside the extractable workspace layout.
- **To Stage 6 (Lift-and-Shift to `mpmbq2/ftxr-modem`):**
  - `crates/ftxr-core`, `crates/ftxr-modem`, `LICENSE`, `NOTICE`, `README.md`, `docs/ipc-protocol.md`, and `fixtures/committed/` are isolated from `tools/ftxr-oracle` and `.oracle-build`/`.oracle-cache` from day one, making extraction a clean copy + `Cargo.toml` member prune.
