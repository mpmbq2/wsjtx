# Stage 1 Plan — 77-Bit Protocol Codec, FEC Encoder & GFSK TX Modulator (FT8 + FT4)

## 1. Header & Scope

- **Stage:** 1 of 7 — 77-Bit Message Codec, FEC Encoder & GFSK TX Modulator (FT8 + FT4)
- **Workspace Location:** `/home/antigravity/mpmbq2/wsjtx/rust-ftx/` (development branch inside `wsjtx`)
- **Primary Crates:**
  - `crates/ftxr-core` (GPLv3 pure-Rust library — `src/protocol/*`, `src/modulate/*`, `src/lib.rs`)
  - `tools/ftxr-oracle` (dev-only differential fuzzing & `jt9` loopback verification subcommands)
- **Prerequisites (from Stage 0):**
  - Headless WSJT-X oracle binaries compiled and verified: `ft8code`, `ft4code`, `jt9` (classic `-8` and `-5`), `ft8sim`, `ft4sim`.
  - Cargo workspace (`rust-ftx/Cargo.toml`) scaffolded with `crates/ftxr-core`, `crates/ftxr-modem`, `tools/ftxr-oracle`, GPLv3 `LICENSE`, and `NOTICE`.
  - Public skeleton types (`Mode`, `FtxrError`, `DecoderEngine`) in `crates/ftxr-core/src/lib.rs`.
- **Deliverables:**
  1. Complete pure-Rust 77-bit WSJT-X message encoder/decoder (`pack77`/`unpack77`, all `i3`/`n3` message types, `fmtmsg`, `split77`, `chkcall`, `callok`, `stdcall`, `pack28`/`unpack28`, `ihashcall`).
  2. Thread-safe, session-scoped `CallsignHashTable` (`Send + Sync`, zero global/static mutable state) with both mutable/transactional and read-only parallel lookup modes for Stage 5 Rayon compatibility.
  3. Exact CRC-14 (`0x2757` / `0x6757` over 96-bit `77 + 5 zero-pad + 14` layout) and LDPC(174,91) systematic encoder (both with-CRC and `nocrc` entry points) plus FT4's 77-bit `rvec` scrambler/descrambler.
  4. FT8 (79-symbol, 8-GFSK, `BT=2.0`, 12.64 s active) and FT4 (103 data/sync + 2 ramp = 105-symbol, 4-GFSK, `BT=1.0`, 5.04 s active) tone mappers and continuous-phase GFSK waveform synthesizers (real `f32` TX PCM + complex `Complex32` analytic reference for Stages 2 & 4 subtraction) supporting arbitrary sample rates (12 kHz / 48 kHz), configurable nominal start delay (`500 ms` FT8 / `300 ms` FT4 / `0 ms` raw), and UTC-aligned late-TX / mid-slot hot-swap `offset_ms` with click-free raised-cosine fade-in.
  5. Automated oracle test suite in `tools/ftxr-oracle`: full `ft8code -T` (49 vectors) & `ft4code -t` (47 vectors) parity, 5,000+ differential fuzzed messages per mode against `ft8code`/`ft4code`, and `jt9 -8`/`jt9 -5` WAV loopback tests covering nominal starts, FT8 1–5 s & FT4 0.5–1.5 s late starts, mid-slot hot-swaps, and multi-slot hash-table resolution.

### Exit Gate Checklist

- [ ] **100% bit-exact parity** on 77-bit payloads (`msgbits`), 14-bit CRC, 83 LDPC parity bits, and channel tone sequences (`79` FT8 / `105` FT4) against `ft8code` and `ft4code` across all built-in test vectors (`-T`/`-t`) and ≥5,000 fuzzed messages per mode covering every valid `i3`/`n3` type.
- [ ] **100% round-trip pack → unpack fidelity** on all canonical valid messages (including `<...>` compound/nonstandard callsign hash-table resolution workflows in both TX and RX modes).
- [ ] **Zero global/static mutable state** in `crates/ftxr-core` (`CallsignHashTable` is explicitly passed and `Send + Sync`; parallel thread-safety tests pass under stress).
- [ ] **Waveform numerical parity:** Complex and real GFSK waveforms at 12 kHz and 48 kHz match WSJT-X `gen_ft8wave`/`gen_ft4wave` to `< 1e-4` RMS error (and `< 1e-6` at 12 kHz).
- [ ] **100% decode rate by classic `jt9 -8` and `jt9 -5`** on Rust-synthesized FT8 and FT4 waveforms across nominal starts (`delay_ms = 500/300/0`), late starts (FT8: `1, 2, 3, 4, 5 s`; FT4: `0.5, 1.0, 1.5 s`), and mid-slot hot-swaps, with recovered AF within **±2 Hz** and DT within **±0.1 s** of theoretical expected DT.
- [ ] **Zero non-GPLv3-compatible dependencies** and zero `wsjtx`/C/Fortran/FFTW/`digi-dx` coupling in `crates/ftxr-core`.

---

## 2. Source Audit & Technical Reference

All constants, bit layouts, indexing rules, and edge-case behaviors below were verified directly from upstream WSJT-X and read-only `digi-dx` sources.

### 2.1 Audited Reference Files

| Component | Exact Source File(s) | Key Subroutines / Symbols |
| :--- | :--- | :--- |
| 77-bit codec & hash table | `wsjtx/lib/77bit/packjt77.f90` (lines 1–1735, 3452–3500) | `ihashcall`, `ihashcall_from_n8`, `save_hash_call`, `hash10`, `hash12`, `hash22`, `pack77`, `unpack77`, `pack28`, `unpack28`, `split77`, `pack77_01`, `pack77_03`, `pack77_06`, `pack77_1`, `pack77_3`, `pack77_4`, `pack77_5`, `packtext77`, `unpacktext77`, `to_grid4`, `to_grid6`, `to_grid`, `add_call_to_recent_calls`, `callok` |
| Callsign & message helpers | `wsjtx/lib/chkcall.f90`, `wsjtx/lib/fmtmsg.f90`, `wsjtx/lib/qra/q65/q65_set_list.f90` (L66–98) | `chkcall`, `fmtmsg`, `stdcall` |
| CRC-14 | `wsjtx/lib/crc14.cpp`, `wsjtx/lib/crc.f90`, `wsjtx/lib/ft8/get_crc14.f90`, `wsjtx/lib/ft8/chkcrc14a.f90`, `wsjtx/lib/ft8/decode174_91.f90` (L75–80) | `crc14`, `crc14_check`, `get_crc14`, `chkcrc14a` |
| LDPC(174,91) encoder | `wsjtx/lib/ft8/encode174_91.f90`, `wsjtx/lib/ft8/encode174_91_nocrc.f90`, `wsjtx/lib/ft8/ldpc_174_91_c_generator.f90` | `encode174_91`, `encode174_91_nocrc`, `g(1:83)` |
| FT8 framing & GFSK TX | `wsjtx/lib/ft8/ft8_params.f90`, `wsjtx/lib/ft8/genft8.f90`, `wsjtx/lib/ft8/gen_ft8wave.f90`, `wsjtx/lib/ft8/ft8code.f90`, `wsjtx/lib/ft8/ft8_testmsg.f90`, `wsjtx/lib/ft8/ft8sim.f90` | `genft8`, `get_ft8_tones_from_77bits`, `gen_ft8wave`, `icos7`, `graymap` |
| FT4 framing & GFSK TX | `wsjtx/lib/ft4/ft4_params.f90`, `wsjtx/lib/ft4/genft4.f90`, `wsjtx/lib/ft4/gen_ft4wave.f90`, `wsjtx/lib/ft4/ft4code.f90`, `wsjtx/lib/ft4/ft4_testmsg.f90`, `wsjtx/lib/ft4/ft4sim.f90` | `genft4`, `get_ft4_tones_from_77bits`, `gen_ft4wave`, `rvec`, `icos4a..d` |
| GFSK pulse & TX timing | `wsjtx/lib/ft2/gfsk_pulse.f90`, `wsjtx/Modulator/Modulator.cpp`, `wsjtx/widgets/mainwindow.cpp` (L7945–7998, L12641–12685) | `gfsk_pulse`, `Modulator::start` (`delay_ms = 500/300`), `Modulator::readData` |
| `jt9` DT & WAV framing | `wsjtx/lib/jt9.f90` (L281–335), `wsjtx/lib/ft8_decode.f90` (L215), `wsjtx/lib/ft4_decode.f90` (L240–252, L430–455), `wsjtx/lib/ft4/subtractft4.f90` (L22), `wsjtx/lib/ft8/subtractft8.f90` (L45, L68) | `NMAX`, `xdt` formulas, `sync4d` ramp alignment |
| `digi-dx` TX contract (read-only) | `digi-dx/src/modem/ft8_lib.rs`, `digi-dx/src/qso/mod.rs` (L52–53, L2217–2365), `digi-dx/src/domain/ft8.rs` (L251–266) | `modulate_pcm_from_offset`, `MAX_LATE_TX_START_MS`, hot-swap `Plan::HotSwap`, `validate_tx_message` |

> **Scope note on `packjt77var`:** Lines 1737–3449 of `packjt77.f90` belong to WSJT-X 3.0's multithreaded `lib/ft8var/` (`-M`) variant. Per `ROADMAP.md` §4 Decision 3, Stage 1 ports the **classic `packjt77`** implementation (lines 1–1735 and `callok` lines 3452–3500) used by classic `jt9 -8`, `jt9 -5`, `ft8code`, and `ft4code`.

---

### 2.2 77-Bit Codec Bit Layouts & Verified Quirks

#### Callsign Hashing (`ihashcall`) & Hash Table Semantics
- **Alphabet (38 chars):** `HASH_ALPHABET = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/"` (indices `0..=37`).
- **Alignment (CRITICAL):** `ihashcall(c0, m)` operates on **11 characters, LEFT-justified and RIGHT-padded with spaces** (`c0(1:11)`). Trailing spaces map to `0`, so every trailing space multiplies `n8` by `38`:
  $$n_8 = \sum_{i=0}^{10} \text{idx}(c_0[i]) \cdot 38^{10-i} \quad (< 38^{11} = 238{,}572{,}050{,}223{,}552{,}512 < 2^{58})$$
- **Hash multiplier:** `HASH_MULT = 47_055_833_459_u64` (verified from `hash_factor = (2419, 62655, 10, 0)` in `ihashcall_from_n8`: $10 \cdot 2^{32} + 62655 \cdot 2^{16} + 2419 = 47{,}055{,}833{,}459$).
- **Formula for $m \in \{10, 12, 22\}$:**
  $$\text{ihashcall}(c_0, m) = \left(47{,}055{,}833{,}459 \cdot n_8 \bmod 2^{64}\right) \gg (64 - m)$$
  In Rust: `((47_055_833_459_u64.wrapping_mul(n8)) >> (64 - m)) as u32`.
- **`save_hash_call` exact rules (`packjt77.f90:150-184`):**
  1. Reject if input starts with `' '` or `"<...>"`.
  2. Strip leading `'<'` and truncate at `'>'` (if present).
  3. Reject if trimmed length `< 3`.
  4. Compute `n10`, `n12`, `n22` (11-char left-justified space-padded).
  5. **Non-obvious `mycall13` exclusion:** `calls10[n10]` and `calls12[n12]` are updated **only if `cw != mycall13`** (prevents `mycall13` from clobbering another station's 10/12-bit slot; `mycall13` resolution is handled explicitly via `hashmy10`/`hashmy12` overrides in `unpack77`). By contrast, `calls22` **does** store `mycall13`.
  6. `calls22` capacity is `MAXHASH = 1000` (FIFO/MRU stack): if `n22` already exists in `ihash22[0..nzhash]`, update `calls22` in-place; otherwise shift entries down by 1 (evicting index 999 when full) and insert `(n22, cw)` at index 0.
  7. Lookups return `format!("<{call}>")` on hit, or `"<...>"` on miss.

#### 28-Bit Callsign / Token Encoding (`pack28` / `unpack28`)
The $2^{28} = 268{,}435{,}456$ address space is partitioned exactly into three contiguous ranges:

| Range of `n28` | Size | Meaning |
| :--- | :--- | :--- |
| `0` | 1 | `"DE"` |
| `1` | 1 | `"QRZ"` |
| `2` | 1 | `"CQ"` |
| `3 ..= 1_002` | 1,000 | `"CQ_000"` .. `"CQ_999"` (`3 + nqsy`, 3 digits) |
| `1_003 ..= 532_443` | 531,441 | `"CQ_a"` .. `"CQ_aaaa"` (`1003 + base27(adjustr(suffix, 4))`, `' '=0, 'A'=1..'Z'=26`) |
| `532_444 ..= 2_063_591` | 1,531,148 | Reserved token space (`NTOKENS = 2_063_592`; see Gotcha #4 below) |
| `2_063_592 ..= 6_257_895` | `4_194_304` ($2^{22}$) | 22-bit hashed callsign: `NTOKENS + n22` (`MAX22 = 4_194_304`) |
| `6_257_896 ..= 268_435_455` | `262_177_560` | Standard callsign: `NTOKENS + MAX22 + mixed_radix(c1..c6)` |

- **Standard callsign mixed-radix encoding (`37 × 36 × 10 × 27 × 27 × 27 = 262,177,560`):**
  - Find rightmost digit `iarea` (1-based) in positions `2..=n` (if none, `iarea = 1`).
  - Standard iff `2 <= iarea <= 3`, `nplet >= 1` (at least 1 letter before `iarea`), `npdig < iarea - 1` (not all digits before `iarea`), and `nslet <= 3` (at most 3 letters after `iarea`, and `n - iarea == nslet`). Otherwise falls back to 22-bit hash `NTOKENS + n22`.
  - Align callsign to 6 chars: if `iarea == 2`, `' ' + c[0..5]`; if `iarea == 3`, `c[0..6]`.
  - Alphabets: `A1 = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"` (37), `A2 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"` (36), `A3 = "0123456789"` (10), `A4 = " ABCDEFGHIJKLMNOPQRSTUVWXYZ"` (27).
  - $n_{28} = 6{,}257{,}896 + 7{,}085{,}880\,i_1 + 196{,}830\,i_2 + 19{,}683\,i_3 + 729\,i_4 + 27\,i_5 + i_6$.
  - **Note on `3DA0`/`3X` lines (`packjt77.f90:728-731`):** Dead legacy JT65 lines in WSJT-X; `callsign` is unconditionally overwritten at lines 819–820 and `3DA0*`/`3XA*` have `iarea != 2,3` so they hash to `NTOKENS + n22`. Do not emit special remapping.
  - **Unpack validation (`unpack28` + `callok`):** Standard callsigns must pass `callok(trim(c13))` (length ≥ 3, does NOT start with `'Q'` — even `"QU1RK"` is rejected by `callok`, rightmost digit at 1-based pos 2 or 3, ≥ 1 letter in prefix, suffix all letters) and contain no internal space; failure sets `c13 = "QU1RK"` and `success = false`.

#### `split77` Preprocessing (`packjt77.f90:919-967`)
1. Replace NUL (`\0`) with `' '`, collapse leading and repeated spaces, uppercase ASCII `a..z`, split into up to 19 words (each truncated to 13 chars).
2. **Directed CQ merge rule (L958–964):** If `nwords >= 3`, `w[0] == "CQ"`, and `chkcall(w[2])` is `true`, merge `w[0]` and `w[1]` into `w[0] = "CQ_" + w[1][..min(len,10)]`, shift `w[2..]` left by 1, and decrement `nwords`. (e.g. `"CQ DX K1ABC FN42"` → `["CQ_DX", "K1ABC", "FN42"]`; `"CQ K1ABC FN42"` stays 3 words because `chkcall("FN42")` is false).

#### Complete `i3`/`n3` Message Table & Exact `pack77` Dispatch Precedence

`pack77` checks message types in the following **strict order** (verified `packjt77.f90:196-265`):
- If input begins with `"$DX"` and `dxbase` is set, substitute `$DX` with `dxbase`.
- If `hint == Some(PackHint::Telemetry)` (`i3=0, n3=5`), jump directly to `0.5`.
- Run `split77`. If collapsed message starts with `"CQ "`, `"DE "`, or `"QRZ "`, skip `0.1`, `0.3`/`0.4`, and `0.5` and jump to `0.6`.
- Otherwise try: **`0.1`** → **`0.3`/`0.4`** → (if `nwords == 1`, **`0.5`**) → **`0.6`** → **`1`/`2`** → **`3`** → **`4`** → **`5`** → fallback **`0.0`**.

| `i3` | `n3` | Name | Exact Bit Layout (MSB-first, 77 bits total) | Key Packing / Unpacking Notes |
| :---: | :---: | :--- | :--- | :--- |
| `0` | `0` | Free Text | `text(71) \| 000(3) \| 000(3)` | 13 chars **right-justified** (`adjustr`), base-42 over `" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?"` ($42^{13} < 2^{71}$); fits in `u128`. Unpack `trim_start`; empty string fails (`unpk77_success = false`). |
| `0` | `1` | DXpedition | `n28a(28) \| n28b(28) \| n10(10) \| n5(5) \| 001(3) \| 000(3)` | 5 words: `CALL1 RR73; CALL2 <FOX> RPT`. `n5 = ((rpt + 30) / 2).clamp(0, 31)` (truncating toward 0); unpack `irpt = 2*n5 - 30` (`-30..=+32` even). Unpack fails if `n28a <= 2 || n28b <= 2`. `n10` overrides: RX `dxcall13`, TX `mycall13`. |
| `0` | `2` | (Deprecated) | — | Never packed; `unpack77` always returns `unpk77_success = false`. |
| `0` | `3`/`4` | ARRL Field Day | `n28a(28) \| n28b(28) \| ir(1) \| intx(4) \| nclass(3) \| isec(7) \| n3(3) \| 000(3)` | `ntx` in `1..=16` → `n3=3, intx=ntx-1`; `17..=32` → `n3=4, intx=ntx-17`. `nclass` = `'A'..='H'` (`0..=7`). `isec` = 1-based index into 86-entry `ARRL_SECTIONS` (`1..=86`). Unpack fails if `isec < 1 || isec > 86 || n28a <= 2 || n28b <= 2`. |
| `0` | `5` | Telemetry | `t0(23) \| t1(24) \| t2(24) \| 101(3) \| 000(3)` | 1 word, right-justified into 18 hex chars (`3z6`), valid iff `t0 < 2^23` (max `7FFFFFFFFFFFFFFFFF`). If `t0 >= 2^23`, falls straight to `0.0` Free Text. Unpack formats `3z6.6` uppercase and strips leading `'0'`s. |
| `0` | `6` | WSPR (50-bit) | Type 1: `n28(28)\|g4(15)\|dbm(5)\|00(2)\|0(21)\|110\|000`<br>Type 2: `n28(28)\|pfx(16)\|dbm(5)\|1(1)\|0(21)\|110\|000`<br>Type 3: `n22(22)\|g6(25)\|010(3)\|0(21)\|110\|000` | Checked **before** Type 1! Type 1 (`CALL GRID DBM`), Type 2 (`PFX/CALL DBM` or `CALL/SFX DBM`, `nzzz=46656`), Type 3 only with `PackHint::Wspr` (`<CALL> GRID4/6`). Note WSPR Type 3 `igrid6` uses **base 25** (`24,24` = 4-char grid), unlike Type 5's base 24! `dbm` quantized via `round(0.3 * clamp(dbm,0,60))`. |
| `1` | — | Standard Msg | `n28a(28)\|ipa(1)\|n28b(28)\|ipb(1)\|ir(1)\|igrid4(15)\|001(3)` | Optional `/R` suffix via `ipa`/`ipb`. `MAXGRID4 = 32400`. Valid grid4: `0..=32399`. Special reports: `32401`=none, `32402`=`RRR`, `32403`=`RR73` (legacy decode), `32404`=`73`, `32405..` = SNR (`irpt = isnr + 35`, wrapping `-50..=-31` by `+101` to `86..=105`). **Gotcha #1:** `"RR73"` packs as Grid4 `32373`, NOT `32403`! |
| `2` | — | EU VHF `/P` | `n28a(28)\|ipa(1)\|n28b(28)\|ipb(1)\|ir(1)\|igrid4(15)\|010(3)` | Identical to Type 1, selected when either call ends in `/P`; `ipa`/`ipb` denote `/P` instead of `/R`. |
| `3` | — | ARRL RTTY | `itu(1)\|n28a(28)\|n28b(28)\|ir(1)\|irpt(3)\|nexch(13)\|011(3)` | `[TU;] CALL1 CALL2 [R] 5x9 <MULT\|SERIAL>`. `irpt = 0..=7` → `529..=599`. `nexch`: `1..=7999` serial (`0001..7999`), `8001..=8171` = `8000 + imult` (1-based index into 171-entry `RTTY_MULTS`). |
| `4` | — | Nonstandard | `n12(12)\|n58(58)\|iflip(1)\|nrpt(2)\|icq(1)\|100(3)` | `n58` = 11-char **RIGHT-justified** (`adjustr`) base-38 over `HASH_ALPHABET`. `iflip=0`: `call1` hashed, `call2` literal (`save_hash_call(call2)` on unpack); `iflip=1`: `call1` literal, `call2` hashed. `nrpt`: `0`=none, `1`=`RRR`, `2`=`RR73`, `3`=`73`. **Gotcha #2:** When `icq=1` (`CQ <nonstd>`), `n12 = ihashcall(w[1], 12)` (NOT 0!). |
| `5` | — | EU VHF Hash | `n12(12)\|n22(22)\|ir(1)\|irpt(3)\|iserial(11)\|igrid6(25)\|101(3)` | `<CALL1> <CALL2> [R] 5xNNNN GRID6`. `nx` in `520001..=594095`; `irpt = nx/10000 - 52`; `iserial = (nx % 10000).min(2047)`. `igrid6` uses **base 24** (`0..=18_662_399`). |

#### Critical Non-Obvious Codec Gotchas Verified from Source

1. **`RR73` packs as Grid4 `32373`, NOT `MAXGRID4 + 3 = 32403` (`packjt77.f90:1256-1297`):** Although `pack77_1` sets `irpt = 3` at line 1258, lines 1286–1294 subsequently check `is_grid4("RR73")`, which evaluates `true` (`'R' ∈ 'A'..'R'`, `'7','3' ∈ '0'..'9'`), overwriting `igrid4` with $17\cdot 1800 + 17\cdot 100 + 70 + 3 = 32373$. Both `ft8code` and `ft4_decode.f90:81` (`mrr73` AP mask = `0x7E75 = 32373`) rely on this! Meanwhile, `unpack77` accepts **both** `32373` (via `to_grid4`) and `32403` (via `irpt == 3`) as `"RR73"`.
2. **Type 4 `CQ <nonstd>` writes `ihashcall(call, 12)` into `n12`, NOT `0` (`packjt77.f90:1430-1432`):** Line 1430 sets `n12 = 0`, and line 1432 immediately calls `save_hash_call(w(2), n10, n12, n22)`, which overwrites the `inout` parameter `n12` with `ihashcall(w(2), 12)`. Failing to populate `n12` here breaks bit-exact parity on `"CQ KH1/KH7Z"` and `"CQ YW18FIFA"`.
3. **Opposite padding between `ihashcall` and Type 4 `n58`:** `ihashcall` pads the 11-char callsign on the **right** (left-justified), whereas Type 4 `n58` pads on the **left** (`adjustr`, right-justified).
4. **`unpack28` gap `532_444 ..= 2_063_591` (`packjt77.f90:852-887`):** Falls out of `if (n28 < NTOKENS)`, subtracts `NTOKENS` (yielding a negative `n22`), misses in `hash22`, and returns `"<...>"` with `success = true`. Preserve exact behavior for bit-level parity (and note `unpack77` line 698 still rejects `"CQ <"`).
5. **Type 1/2 hash-table asymmetry (`packjt77.f90:559`):** During `unpack77`, ONLY the "from" callsign (`call_2`) is saved to the hash table (not `call_1`), matching WSJT-X's rule that only transmitted callsigns are trusted hashes. Similarly, Type 4 `unpack77` only calls `save_hash_call` when `iflip == 0` (`call_2`).
6. **Three distinct callsign predicates:** `chkcall` (used by `split77`/`pack77_*`; extracts basecall from compounds, allows `QU1RK`, requires suffix 1–3 letters), `pack28` standard-call check (checks `iarea in 2..=3`, `nplet >= 1`, `npdig < iarea - 1`, `nslet <= 3`), and `callok` (used by `unpack28`; rejects any `'Q'` prefix including `QU1RK`, allows 0-letter suffix like `"KA1"`). All three must be ported faithfully without unifying them.

---

### 2.3 CRC-14 & LDPC(174,91) Exact Bit Layout

#### CRC-14 Specification (`lib/crc14.cpp`, `lib/ft8/encode174_91.f90`, `lib/ft8/decode174_91.f90`, `lib/ft8/get_crc14.f90`)
- **Truncated polynomial:** `0x2757` (full 15-bit generator polynomial with implicit $x^{14}$ term: `0x6757` = `0b110_0111_0101_0111`).
- **Exact 96-bit (12-byte) augmented bit layout:**
  ```
  Bit index (0-based):  0 ..= 76      77 ..= 81     82 ..= 95
                        [ 77 msg bits | 5 ZERO bits | 14 CRC bits ]
  Byte index (MSB-1st): bytes[0..9] (all 8 bits) + bytes[9] bits 7..3  = msg[0..77]
                        bytes[9] bits 2..0 + bytes[10] bits 7..6       = 0b00000 (5 zero pad bits)
                        bytes[10] bits 5..0 + bytes[11] bits 7..0      = 0 (encode) or crc14 (verify)
  ```
- **Encode (`compute_crc14`):** Place `msg77[0..77]` into bits `0..77`, zero bits `77..96` (19 trailing zeros = 5 zero-pad bits + 14 CRC slot zeros), divide MSB-first by `0x6757` over GF(2). Remainder (14 bits) is appended directly as `message91[77..91]` (MSB-first).
- **Verify (`verify_crc14`):** Either recompute `compute_crc14(&msg91[0..77]) == u14_from_bits(&msg91[77..91])` (`chkcrc14a.f90`), or place `msg91[0..77]` in bits `0..77`, `00000` in bits `77..82`, `msg91[77..91]` in bits `82..96` and check remainder `== 0` (`decode174_91.f90:75-78`, `crc14_check`). Both are mathematically identical.

#### LDPC(174,91) Generator Matrix (`lib/ft8/ldpc_174_91_c_generator.f90`)
- Dimensions: $N = 174$, $K = 91$ ($77 + 14$), $M = N - K = 83$.
- `g(1:83)` stores 83 rows × 23 hex digits (92 bits/row; LSB of the 23rd hex digit is always `0` and unused).
- Parity bit $i \in \{0,\dots,82\}$:
  $$p_i = \left(\sum_{j=0}^{90} m_j \cdot G_{i,j}\right) \bmod 2$$
- Systematic codeword: `cw[0..91] = msg91[0..91]`, `cw[91..174] = p[0..83]`.
- **FT4 Scrambling (`rvec`, `lib/ft4/genft4.f90:33-35, 64-65`):**
  - 77-bit constant:
    ```
    01001010010111101000100110110
    10010110000100010100111100101
    0101011011111000101
    ```
  - **Order is critical:** `scrambled77 = msg77 ^ rvec` **BEFORE** CRC-14 and LDPC(174,91) encoding! On RX (`ft4_decode.f90:430`), CRC-14 is verified on `scrambled77`, and `rvec` XOR is removed *after* LDPC+CRC passes, right before `unpack77`.

---

### 2.4 Tone Mapping & GFSK Waveform Formulas (FT8 vs FT4)

| Parameter | FT8 (`genft8.f90` / `gen_ft8wave.f90`) | FT4 (`genft4.f90` / `gen_ft4wave.f90`) |
| :--- | :--- | :--- |
| Modulation | 8-GFSK (`hmod = 1.0`, `BT = 2.0`) | 4-GFSK (`hmod = 1.0`, `BT = 1.0`) |
| Symbol duration $T_s$ | `0.160 s` (`1920` @ 12 kHz, `7680` @ 48 kHz) | `0.048 s` (`576` @ 12 kHz, `2304` @ 48 kHz) |
| Tone spacing $\Delta f = 1/T_s$ | `6.25 Hz` (BW = `50.0 Hz`) | `20.8333... Hz` (`125/6 Hz`, BW = `83.33 Hz`) |
| Channel symbols | `79` (`S7 D29 S7 D29 S7`) | `103` data/sync + `2` ramp = **`105`** (`R S4 D29 S4 D29 S4 D29 S4 R`) |
| Active waveform duration | `79 * 0.160 = 12.64 s` (`151,680` @ 12 kHz, `606,720` @ 48 kHz) | `105 * 0.048 = 5.04 s` (`60,480` @ 12 kHz, `241,920` @ 48 kHz) |
| Costas sync arrays | `ICOS7 = [3, 1, 4, 0, 6, 5, 2]` (×3) | `ICOS4A = [0,1,3,2]`, `B = [1,0,2,3]`, `C = [2,3,1,0]`, `D = [3,2,0,1]` |
| Gray map (MSB-first bits → tone) | `[0, 1, 3, 2, 5, 6, 4, 7]` (non-standard!) | `[0, 1, 3, 2]` |
| `dphi` buffer length | `(79 + 2) * nsps = 81 * nsps` | `(103 + 2) * nsps = 105 * nsps` |
| Boundary symbol extension | **YES:** dummy `itone[0]` prepended (`dphi[0..2*nsps] += peak*itone[0]*pulse[nsps..3*nsps]`), dummy `itone[78]` appended (`dphi[79*nsps..81*nsps] += peak*itone[78]*pulse[0..2*nsps]`) | **NO:** zero-padded outside the 103 symbol pulses (`R` ramp symbols smoothly transition from/to base carrier `f0`) |
| Phase integration slice | `j = nsps .. nsps + 79*nsps` (skips outer `nsps` on each side; `79*nsps` output samples) | `j = 0 .. 105*nsps` (integrates full buffer; `105*nsps` output samples) |
| Raised-cosine envelope ramp | `nramp = round(nsps / 8.0)` (`20 ms`: 240 @ 12 kHz, 960 @ 48 kHz) on first & last `nramp` samples | `nramp = nsps` (`48 ms`: 576 @ 12 kHz, 2304 @ 48 kHz) across full first & last ramp symbols |
| Nominal TX start `delay_ms` (`Modulator.cpp:76,79`) | `500 ms` (or `0 ms` raw / `digi-dx` mode) | `300 ms` (or `0 ms` raw) |
| Slot length (`jt9` `NMAX` @ 12 kHz) | `15.0 s` (`180,000` samples) | `7.5 s` UTC slot (`jt9 -5` reads `NMAX = 21*3456 = 72,576` = `6.048 s`) |
| Expected `jt9` reported `DT` | `delay_ms / 1000.0 - 0.500` (`0.0 s` @ 500 ms; `-0.5 s` @ 0 ms) | `(delay_ms / 1000.0 + 0.048) - 0.500` (`-0.15 s` @ 300 ms; `0.0 s` @ 452 ms; `-0.45 s` @ 0 ms) |

#### Exact GFSK Math
1. **Gaussian frequency-smoothing pulse (`lib/ft2/gfsk_pulse.f90`):**
   $$c = \pi \sqrt{\frac{2}{\ln 2}}, \qquad g(b, t) = \frac{1}{2}\Bigl(\operatorname{erf}\!\bigl(c\,b\,(t + 0.5)\bigr) - \operatorname{erf}\!\bigl(c\,b\,(t - 0.5)\bigr)\Bigr)$$
   Sampled at `idx = 0 .. 3*nsps` using WSJT-X's 1-based grid:
   $$t[\text{idx}] = \frac{(\text{idx} + 1) - 1.5 \cdot \text{nsps}}{\text{nsps}}$$
2. **Instantaneous phase increment (`dphi`):**
   $$\Delta\phi_{\text{peak}} = \frac{2\pi}{\text{nsps}}, \qquad \Delta\phi_0 = \frac{2\pi f_0}{f_s}$$
   $$\Delta\phi[k] = \Delta\phi_0 + \sum_{j=0}^{N_{\text{sym}}-1} \Delta\phi_{\text{peak}} \cdot \text{itone}[j] \cdot \text{pulse}[k - j \cdot \text{nsps}] \quad (+ \text{ FT8 edge dummies})$$
3. **Phase accumulation (`f64` wrapped to $[0, 2\pi)$):**
   Initialize $\phi = 0.0$. For each output sample $k$:
   - Real output (`icmplx = 0`): `wave[k] = phi.sin() as f32`
   - Complex output (`icmplx = 1`): `cwave[k] = Complex32::new(phi.cos() as f32, phi.sin() as f32)`
   - Advance: `phi = (phi + dphi[j]).rem_euclid(TAU)`
4. **Raised-cosine envelope shaping:**
   - Ramp-up ($i = 0 \dots n_{\text{ramp}}-1$): $w_{\text{up}}[i] = \frac{1}{2}\bigl(1 - \cos(\pi i / n_{\text{ramp}})\bigr)$
   - Ramp-down ($i = 0 \dots n_{\text{ramp}}-1$, applied at `nwave - nramp + i`): $w_{\text{dn}}[i] = \frac{1}{2}\bigl(1 + \cos(\pi i / n_{\text{ramp}})\bigr)$
5. **UTC-aligned `delay_ms` & `offset_ms` (Late-TX & Mid-Slot Hot-Swap):**
   Matches both WSJT-X `Modulator.cpp:94-104` and `digi-dx/src/modem/ft8_lib.rs:153-175`:
   - If `offset_ms < delay_ms`: prepend `round((delay_ms - offset_ms) * fs / 1000.0)` samples of `0.0`, followed by the full active waveform (`wave[0..]`).
   - If `offset_ms == delay_ms`: output the full active waveform (`wave[0..]`) directly.
   - If `offset_ms > delay_ms`: compute `skip = round((offset_ms - delay_ms) * fs / 1000.0)`.
     - If `skip >= wave.len()`, return empty `Vec::new()`.
     - Otherwise slice `tail = wave[skip..]`, and apply a click-free raised-cosine fade-in $w_{\text{up}}[i] = \frac{1}{2}(1 - \cos(\pi i / n_{\text{fade}}))$ over `n_fade = late_fade_samples.min(tail.len())` where `late_fade_samples = (nsps / 8).max(1)` for FT8 (`20 ms`, matching `digi-dx`) and `(nsps / 4).max(1)` for FT4 (`12 ms`, 1/4 symbol, guaranteeing `tail[0] == 0.0` with minimal symbol energy loss).

---

## 3. Architecture & Concrete Rust Design

### 3.1 Workspace & File Layout

```text
rust-ftx/
├── Cargo.toml
├── crates/ftxr-core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                 # Public API re-exports, Mode, ModulateConfig, FtxrError
│       ├── protocol/
│       │   ├── mod.rs             # Public protocol facade & re-exports
│       │   ├── types.rs           # MessageType, PackHint, UnpackMode, UnpackContext, PackedMessage, UnpackedMessage, tables
│       │   ├── hashtable.rs       # Thread-safe CallsignHashTable (RwLock-backed, no globals) + ihashcall
│       │   ├── pack77.rs          # fmtmsg, split77, chkcall, stdcall, pack28, pack77 & pack77_* encoders
│       │   ├── unpack77.rs        # callok, unpack28, unpack77, unpacktext77, grid decoders
│       │   ├── crc14.rs           # CRC-14 (0x2757/0x6757) 96-bit layout, const 256-entry table, verify
│       │   ├── ldpc.rs            # LDPC(174,91) const [u128; 83] generator, encode174_91, encode174_91_nocrc
│       │   └── rvec.rs            # FT4 77-bit scrambling vector & XOR helpers
│       └── modulate/
│           ├── mod.rs             # ModulateConfig, ModulatedWaveform, modulate_message, modulate_tones, modulate_slot_pcm
│           ├── gfsk.rs            # gfsk_pulse (libm::erf), pulse table, cosine ramps, delay/offset/fade engine
│           ├── ft8.rs             # FT8 constants, Costas, Gray map, tone encoder, real & complex GFSK wave gen
│           └── ft4.rs             # FT4 constants, Costas A-D, Gray map, tone encoder, real & complex GFSK wave gen
└── tools/ftxr-oracle/
    ├── Cargo.toml
    └── src/
        ├── main.rs                # Add `codec-diff` and `tx-loopback` CLI subcommands
        ├── codec_oracle.rs        # ft8code / ft4code runner, parser, deterministic fuzzer
        └── tx_loopback.rs         # WAV writer + jt9 -8/-5 loopback runner (nominal, late-TX, hot-swap, hash-chain)
```

### 3.2 Dependencies & GPLv3 License Compatibility

All dependencies are strictly pure-Rust and GPLv3-compatible (`MIT OR Apache-2.0`):

| Crate | Target | Version | License | Purpose |
| :--- | :--- | :--- | :--- | :--- |
| `libm` | `ftxr-core` | `0.2` | `MIT OR Apache-2.0` | Pure-Rust IEEE-754 `libm::erf` for `gfsk_pulse` (zero C FFI) |
| `num-complex` | `ftxr-core` | `0.4` | `MIT OR Apache-2.0` | `Complex32` analytic GFSK reference waveforms (reused in Stages 2–4) |
| `serde` | `ftxr-core` | `1` (`derive`) | `MIT OR Apache-2.0` | `Mode`, `ModulateConfig`, protocol types |
| `thiserror` | `ftxr-core` | `2` | `MIT OR Apache-2.0` | `FtxrError` error enum |
| `rand`, `rand_chacha` | `ftxr-oracle` | `0.8`, `0.3` | `MIT OR Apache-2.0` | Seeded deterministic message fuzzing (dev-only) |
| `hound` | `ftxr-oracle` | `3.5` | `Apache-2.0` | 12 kHz 16-bit mono WAV export for `jt9` loopback (dev-only) |
| `serde_json` | `ftxr-oracle` | `1` | `MIT OR Apache-2.0` | JSON oracle diff scorecards (dev-only) |

### 3.3 Concrete Public & Internal Rust Signatures

#### `crates/ftxr-core/src/lib.rs` (additions / refinements to Stage 0 stubs)
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ft8,
    Ft4,
}

#[derive(Debug, thiserror::Error)]
pub enum FtxrError {
    #[error("invalid message: {0}")]
    InvalidMessage(String),
    #[error("failed to unpack 77-bit payload")]
    UnpackFailed,
    #[error("invalid audio frequency {af_hz} Hz (expected {min_hz}..={max_hz} Hz)")]
    InvalidFrequency { af_hz: f32, min_hz: f32, max_hz: f32 },
    #[error("invalid sample rate {sample_rate} Hz (minimum 8000 Hz)")]
    InvalidSampleRate { sample_rate: u32 },
    #[error("invalid tone value {tone} for mode {mode:?}")]
    InvalidTone { mode: Mode, tone: u8 },
    // ... existing Stage 0 / future RX variants preserved
}
```

#### `crates/ftxr-core/src/protocol/types.rs`
```rust
pub const MAX_MSG_LEN: usize = 37;
pub const PAYLOAD_BITS: usize = 77;
pub const CRC14_BITS: usize = 14;
pub const LDPC_K: usize = 91;
pub const LDPC_M: usize = 83;
pub const LDPC_N: usize = 174;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    FreeText,       // i3=0, n3=0
    Dxpedition,     // i3=0, n3=1
    FieldDayLow,    // i3=0, n3=3 (1..=16 TX)
    FieldDayHigh,   // i3=0, n3=4 (17..=32 TX)
    Telemetry,      // i3=0, n3=5
    Wspr,           // i3=0, n3=6
    Standard,       // i3=1
    EuVhfPortable,  // i3=2 (/P)
    ArrlRtty,       // i3=3
    Nonstandard,    // i3=4
    EuVhfHashed,    // i3=5
}

impl MessageType {
    pub fn i3_n3(self) -> (u8, u8) { /* exact (i3, n3) pair */ }
    pub fn from_i3_n3(i3: u8, n3: u8) -> Option<Self> { /* None for 0.2, 0.7, i3>=6 */ }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackHint {
    Telemetry, // i3_hint=0, n3_hint=5
    Wspr,      // i3_hint=0, n3_hint=6 (enables WSPR Type 3)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackMode {
    Tx, // nrx = 0 (TX verification / msgsent generation)
    Rx, // nrx = 1 (received frame decoding)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnpackContext {
    pub my_call: Option<String>, // corresponds to mycall13
    pub dx_call: Option<String>, // corresponds to dxcall13
    pub dx_base: Option<String>, // corresponds to dxbase (used for $DX substitution in pack77)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedMessage {
    pub bits: [u8; PAYLOAD_BITS], // 0 or 1 per bit, MSB-first
    pub i3: u8,
    pub n3: u8,
    pub msg_type: MessageType,
    pub msgsent: String,          // unpacked confirmation string (nrx = 0)
}

/// Deferred hash-table mutations emitted by `unpack77_readonly` so Stage 5
/// parallel Rayon workers never mutate shared state out of order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingHashUpdates {
    pub save_calls: Vec<String>,   // calls to pass to CallsignHashTable::save_call
    pub recent_calls: Vec<String>, // calls to pass to CallsignHashTable::add_recent
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackedMessage {
    pub text: String,
    pub i3: u8,
    pub n3: u8,
    pub msg_type: MessageType,
    pub pending_hashes: PendingHashUpdates,
}

pub const ARRL_SECTIONS: [&str; 86] = [ /* exact packjt77.f90:300-308 */ ];
pub const RTTY_MULTS: [&str; 171] = [ /* exact packjt77.f90:310-327 */ ];
```

#### `crates/ftxr-core/src/protocol/hashtable.rs` (Zero Globals, Thread-Safe)
```rust
use std::collections::VecDeque;
use std::sync::RwLock;

pub const MAX_HASH22: usize = 1000;
pub const MAX_RECENT_CALLS: usize = 10;
pub const HASH_ALPHABET: &[u8; 38] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";
pub const HASH_MULT: u64 = 47_055_833_459;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallsignHashes {
    pub h10: u16, // 0..1024
    pub h12: u16, // 0..4096
    pub h22: u32, // 0..4_194_304
}

/// Compute 10-, 12-, or 22-bit WSJT-X callsign hash (exact port of `ihashcall`).
/// Input is left-justified, right-padded with spaces to 11 chars.
pub fn ihashcall(call: &str, bits: u8) -> u32;
pub fn compute_all_hashes(call: &str) -> CallsignHashes;

#[derive(Debug, Clone)]
struct HashTableInner {
    calls10: Box<[Option<String>; 1024]>,
    calls12: Box<[Option<String>; 4096]>,
    /// Front = most recent; max length `MAX_HASH22` (1000), exact FIFO/in-place update semantics.
    calls22: VecDeque<(u32, String)>,
    recent_calls: VecDeque<String>,
}

/// Thread-safe, session-scoped callsign hash table (`Send + Sync`, no global state).
/// Owned by `DecoderEngine` (one per session) and cleared on `reset_session`.
#[derive(Debug)]
pub struct CallsignHashTable {
    inner: RwLock<HashTableInner>,
}

impl CallsignHashTable {
    pub fn new() -> Self;
    pub fn clear(&self);
    /// Exact port of `save_hash_call`: strips `<...>`, requires len >= 3,
    /// skips updating `calls10`/`calls12` when `clean == my_call`, updates `calls22`.
    pub fn save_call(&self, raw_call: &str, my_call: Option<&str>) -> Option<CallsignHashes>;
    pub fn lookup_10(&self, h10: u16) -> Option<String>; // returns bare call without <>
    pub fn lookup_12(&self, h12: u16) -> Option<String>;
    pub fn lookup_22(&self, h22: u32) -> Option<String>;
    /// Format as `"<CALL>"` or `"<...>"` matching WSJT-X `hash10`/`hash12`/`hash22`.
    pub fn format_10(&self, h10: u16) -> String;
    pub fn format_12(&self, h12: u16) -> String;
    pub fn format_22(&self, h22: u32) -> String;
    pub fn add_recent(&self, call: &str);
    pub fn recent_calls(&self) -> Vec<String>;
    /// Apply deferred updates in deterministic order (used by Stage 5 parallel commit).
    pub fn apply_updates(&self, updates: &PendingHashUpdates, my_call: Option<&str>);
}
```
> **Memory & Concurrency Design:** `CallsignHashTable` uses `Box<[Option<String>; 1024]>` + `Box<[Option<String>; 4096]>` (~120 KB heap, tiny stack footprint) protected by `std::sync::RwLock`. During Stage 5 Rayon parallel candidate decoding, worker threads invoke `unpack77_readonly` (acquiring only concurrent shared read locks) and collect `PendingHashUpdates`, which the main thread commits deterministically via `apply_updates` in candidate order. Single-threaded callers (Stages 1–4 and TX) use `pack77` / `unpack77`, which apply updates immediately. Zero global `static mut`, zero `lazy_static`, zero lock poisoning hazards across sessions.

#### `crates/ftxr-core/src/protocol/mod.rs`, `crc14.rs`, `ldpc.rs`, `rvec.rs`
```rust
// protocol/pack77.rs & unpack77.rs
pub fn fmtmsg(msg: &str) -> String;
pub fn split77(msg: &str) -> (String, Vec<String>);
pub fn chkcall(word: &str) -> Option<String>; // returns Some(basecall) if valid standard/compound
pub fn callok(call: &str) -> bool;            // exact unpack28 validator
pub fn stdcall(call: &str) -> bool;           // exact ft8apset validator (consumed by Stage 3)

pub fn pack28(token: &str, hash_table: Option<&CallsignHashTable>, my_call: Option<&str>) -> Option<u32>;
pub fn unpack28(n28: u32, hash_table: &CallsignHashTable) -> (String, bool);

pub fn pack77(
    msg: &str,
    hint: Option<PackHint>,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<PackedMessage, FtxrError>;

/// Unpack 77 bits and immediately commit any learned hashes/recent calls into `hash_table`.
pub fn unpack77(
    bits: &[u8; PAYLOAD_BITS],
    mode: UnpackMode,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<UnpackedMessage, FtxrError>;

/// Pure read-only unpack for concurrent Rayon workers (Stage 5); caller commits `pending_hashes`.
pub fn unpack77_readonly(
    bits: &[u8; PAYLOAD_BITS],
    mode: UnpackMode,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<UnpackedMessage, FtxrError>;

// protocol/crc14.rs
pub const CRC14_POLY: u16 = 0x2757;
pub const CRC14_FULL_POLY: u16 = 0x6757;
pub fn pack_msg77_to_bytes12(msg77: &[u8; 77]) -> [u8; 12];
pub fn crc14_bytes12(bytes: &[u8; 12]) -> u16;
pub fn compute_crc14(msg77: &[u8; 77]) -> u16;
pub fn verify_crc14(msg91: &[u8; 91]) -> bool;

// protocol/ldpc.rs
pub const LDPC_GEN_ROWS: [u128; 83] = [ /* const-parsed from ldpc_174_91_c_generator.f90 */ ];
pub fn encode174_91(msg77: &[u8; 77]) -> [u8; 174];
pub fn encode174_91_nocrc(msg91: &[u8; 91]) -> [u8; 174];
pub fn generator_bit(row: usize, col: usize) -> u8; // helper for Stage 3 OSD

// protocol/rvec.rs
pub const FT4_RVEC: [u8; 77] = [ /* exact 77-bit FT4 scrambler */ ];
pub fn scramble_ft4(msg77: &[u8; 77]) -> [u8; 77];
pub fn descramble_ft4(scrambled77: &[u8; 77]) -> [u8; 77];
```

#### `crates/ftxr-core/src/modulate/`
```rust
// modulate/mod.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModulateConfig {
    pub mode: Mode,
    /// Base audio tone frequency in Hz (typical passband 200.0..=3000.0; validated 50.0..=4500.0).
    pub af_hz: f32,
    /// Output PCM sample rate in Hz (e.g., 12_000 or 48_000; minimum 8_000).
    pub sample_rate: u32,
    /// Nominal TX start delay from UTC slot boundary in milliseconds.
    /// Defaults: 500 ms (FT8), 300 ms (FT4). Set to 0 for raw active waveform (digi-dx compat).
    pub delay_ms: u32,
    /// Elapsed ms from UTC slot boundary at which TX starts (0 = nominal slot start; > delay_ms = late TX / hot-swap).
    pub offset_ms: u32,
    /// Peak output amplitude scaling in (0.0, 1.0] (default 1.0).
    pub amplitude: f32,
}

impl ModulateConfig {
    pub fn default_for(mode: Mode, af_hz: f32, sample_rate: u32) -> Self; // delay_ms = 500 (FT8) / 300 (FT4), offset_ms = 0, amp = 1.0
    pub fn raw_active(mode: Mode, af_hz: f32, sample_rate: u32) -> Self;  // delay_ms = 0, offset_ms = 0, amp = 1.0
}

#[derive(Debug, Clone)]
pub enum EncodedTones {
    Ft8([u8; 79]),
    Ft4([u8; 103]), // 103 data+sync symbols (2 ramp symbols synthesized by gen_ft4_wave)
}

#[derive(Debug, Clone)]
pub struct ModulatedWaveform {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub mode: Mode,
    pub msgsent: String,
    pub tones: EncodedTones,
    pub i3: u8,
    pub n3: u8,
}

pub fn modulate_message(
    msg: &str,
    config: &ModulateConfig,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<ModulatedWaveform, FtxrError>;

pub fn modulate_tones(tones: &EncodedTones, config: &ModulateConfig) -> Result<Vec<f32>, FtxrError>;

/// Convenience for offline testing / full-slot WAV generation (pads/truncates to exact slot length).
pub fn modulate_slot_pcm(
    msg: &str,
    config: &ModulateConfig,
    slot_samples: usize,
    ctx: &UnpackContext,
    hash_table: &CallsignHashTable,
) -> Result<(Vec<f32>, ModulatedWaveform), FtxrError>;

// modulate/ft8.rs (reused directly by Stage 2 subtractft8)
pub fn encode_ft8_tones_from_77bits(msg77: &[u8; 77]) -> [u8; 79];
pub fn gen_ft8_wave_real(tones: &[u8; 79], af_hz: f64, sample_rate: u32) -> Vec<f32>;
pub fn gen_ft8_wave_complex(tones: &[u8; 79], af_hz: f64, sample_rate: u32) -> Vec<num_complex::Complex32>;

// modulate/ft4.rs (reused directly by Stage 4 subtractft4)
pub fn encode_ft4_tones_from_77bits(msg77_unscrambled: &[u8; 77]) -> [u8; 103];
pub fn gen_ft4_wave_real(tones: &[u8; 103], af_hz: f64, sample_rate: u32) -> Vec<f32>;
pub fn gen_ft4_wave_complex(tones: &[u8; 103], af_hz: f64, sample_rate: u32) -> Vec<num_complex::Complex32>;
```

---

## 4. Ordered Implementation Tasks

Execute tasks strictly in sequence; each task ends with passing unit/oracle checks before proceeding.

### Task 1.1 — Add Dependencies & Core Protocol Types (`crates/ftxr-core`)
- **Files:** `crates/ftxr-core/Cargo.toml`, `crates/ftxr-core/src/lib.rs`, `crates/ftxr-core/src/protocol/types.rs`, `crates/ftxr-core/src/protocol/mod.rs`.
- **Actions:**
  - Add `libm = "0.2"` and `num-complex = "0.4"` to `ftxr-core/Cargo.toml`.
  - Implement `MessageType`, `PackHint`, `UnpackMode`, `UnpackContext`, `PackedMessage`, `UnpackedMessage`, `PendingHashUpdates`, `ARRL_SECTIONS` (86 entries verbatim), and `RTTY_MULTS` (171 entries verbatim).
  - Add protocol/modulate error variants to `FtxrError`.
- **Acceptance criteria:** `cargo check -p ftxr-core` succeeds; unit test verifies `ARRL_SECTIONS.len() == 86`, `RTTY_MULTS.len() == 171`, no duplicate entries, and `MessageType::from_i3_n3` rejects `0.2`, `0.7`, `6.*`, `7.*`.

### Task 1.2 — Implement `ihashcall` & Thread-Safe `CallsignHashTable`
- **Files:** `crates/ftxr-core/src/protocol/hashtable.rs`.
- **Actions:**
  - Implement `ihashcall(call, bits)` using 11-char left-justified space-padded base-38 encoding and `47_055_833_459_u64.wrapping_mul(n8) >> (64 - bits)`.
  - Implement `CallsignHashTable` with `RwLock<HashTableInner>`, enforcing `my_call` exclusion on 10/12-bit tables, in-place update + 1000-entry FIFO eviction on `calls22`, `<...>` stripping, minimum length 3, and 10-entry `recent_calls` ring.
  - Implement `apply_updates` for deterministic Stage 5 Rayon commits.
- **Acceptance criteria:** Unit tests verify known hashes (`K1ABC`, `W9XYZ`, `PJ4/K1ABC`, `KH1/KH7Z`, `YW18FIFA`, `G4ABC/P`), `my_call` collision exclusion on 10/12-bit slots, 1001st entry eviction in `calls22`, and a 16-thread concurrent read/write stress test with zero deadlocks/panics.

### Task 1.3 — Implement Callsign Predicates, `fmtmsg`, `split77`, `pack28` & `unpack28`
- **Files:** `crates/ftxr-core/src/protocol/pack77.rs`, `crates/ftxr-core/src/protocol/unpack77.rs`.
- **Actions:**
  - Port `fmtmsg`, `split77` (including the `CQ <dir> <call>` → `CQ_<dir>` merge rule), `chkcall`, `stdcall`, and `callok` verbatim.
  - Port `pack28` and `unpack28` across all three address ranges (`DE`/`QRZ`/`CQ`/`CQ_nnn`/`CQ_aaaa`, 22-bit `<...>` hashes, standard mixed-radix calls), including `"QU1RK"` failure handling and internal-space rejection.
- **Acceptance criteria:** Unit tests verify boundary `n28` values (`0, 1, 2, 3, 1002, 1003, 532443, 532444, 2063591, 2063592, 6257895, 6257896, 268435455`), round-trip on 1/2-letter prefixes, 1/2/3-letter suffixes, `CQ_000..CQ_999`, `CQ_A..CQ_ZZZZ`, `chkcall` vs `callok` edge cases (`QU1RK`, `FN42`, `KA1`, `PJ4/K1ABC`, `K1ABC/R`, `K1ABC/P`, `3DA0XYZ`).

### Task 1.4 — Implement Complete `pack77` & `unpack77` (All `i3`/`n3` Types)
- **Files:** `crates/ftxr-core/src/protocol/pack77.rs`, `crates/ftxr-core/src/protocol/unpack77.rs`.
- **Actions:**
  - Implement exact dispatch precedence: `$DX` substitution → Telemetry hint → `split77` → `0.1` → `0.3`/`0.4` → `0.5` → `0.6` (WSPR Types 1/2/3) → `1`/`2` → `3` → `4` → `5` → `0.0` (`u128` base-42).
  - Preserve every verified WSJT-X quirk: `RR73` packed as Grid4 `32373` (and both `32373`/`32403` unpacked as `RR73`), Type 4 `icq=1` populating `n12 = ihashcall(w[1], 12)`, right-justified `n58`, `nrx` (`Tx` vs `Rx`) override rules for `my_call`/`dx_call`, base-25 WSPR grid vs base-24 Type 5 grid, and `"CQ <"` unpack rejection.
  - Implement both `unpack77_readonly` (returns `PendingHashUpdates`) and `unpack77` (applies updates).
- **Acceptance criteria:** All 49 `ft8_testmsg.f90` messages and all 47 `ft4_testmsg.f90` messages pack and unpack with 100% round-trip fidelity (matching exact `ft8code -T` / `ft4code -t` `msgsent` and `i3.n3`).

### Task 1.5 — Implement CRC-14, LDPC(174,91) Encoder & FT4 `rvec` Scrambler
- **Files:** `crates/ftxr-core/src/protocol/crc14.rs`, `crates/ftxr-core/src/protocol/ldpc.rs`, `crates/ftxr-core/src/protocol/rvec.rs`.
- **Actions:**
  - Implement `crc14.rs` with both a bit-by-bit reference and a `const`-generated 256-entry lookup table over the exact 12-byte (96-bit: `77 + 5 zero-pad + 14`) layout.
  - Embed all 83 hex rows from `ldpc_174_91_c_generator.f90` into `LDPC_GEN_ROWS: [u128; 83]` (const-parsed at compile time with `>> 1` alignment) and implement `encode174_91`, `encode174_91_nocrc`, and `generator_bit`.
  - Implement `rvec.rs` with the exact 77-bit `FT4_RVEC` array and XOR helpers.
- **Acceptance criteria:** Bit-by-bit vs table CRC-14 match across 10,000 random 77-bit payloads; every encoded 174-bit codeword satisfies all 83 LDPC parity-check equations (`H * cw^T == 0 mod 2`, cross-checked against `ldpc_174_91_c_parity.f90`); single-bit flip in any of the 91 info+CRC bits causes `verify_crc14` to fail; `encode_ft8_tones` / `encode_ft4_tones` match `ft8code` / `ft4code` bit-for-bit on hand-verified golden vectors.

### Task 1.6 — Implement Tone Mappers & Continuous-Phase GFSK Waveform Engine (FT8 + FT4)
- **Files:** `crates/ftxr-core/src/modulate/gfsk.rs`, `ft8.rs`, `ft4.rs`, `mod.rs`.
- **Actions:**
  - Implement `gfsk_pulse(bt, t)` using `libm::erf` and `build_gfsk_pulse_table(bt, nsps)` on WSJT-X's exact `((idx + 1) - 1.5 * nsps) / nsps` grid.
  - Implement `ft8.rs`: `ICOS7`, `FT8_GRAYMAP`, `encode_ft8_tones_from_77bits`, `gen_ft8_wave_real`, `gen_ft8_wave_complex` (with dummy boundary extensions, `j = nsps..80*nsps` integration, `nramp = round(nsps/8)` cosine ramp).
  - Implement `ft4.rs`: `ICOS4A..D`, `FT4_GRAYMAP`, `encode_ft4_tones_from_77bits` (applies `rvec` scramble before LDPC!), `gen_ft4_wave_real`, `gen_ft4_wave_complex` (zero-padded `105*nsps` `dphi`, full `j = 0..105*nsps` integration, `nramp = nsps` full-symbol cosine ramps).
  - Implement `ModulateConfig` validation (`sample_rate >= 8000`, `af_hz` in passband, `0.0 < amplitude <= 1.0`), `delay_ms` silence prefix, `offset_ms` late-TX slicing with click-free raised-cosine fade-in, and `modulate_slot_pcm`.
- **Acceptance criteria:** Unit tests verify exact sample counts (`151,680`/`606,720` FT8; `60,480`/`241,920` FT4 at 12/48 kHz), `delay_ms` silence lengths (`6,000`/`24,000` for 500 ms FT8; `3,600`/`14,400` for 300 ms FT4), `offset_ms` truncation lengths (exact parity with `digi-dx/src/modem/ft8_lib.rs:278-298`: 5000 ms offset at 48 kHz with `delay_ms = 0` yields `606,720 - 240,000 = 366,720` samples and `pcm[0].abs() < 1e-6`), past-end `offset_ms` returns empty `Vec`, peak amplitude `<= amplitude`, continuity (no sample-to-sample phase jumps > `dphi_max`), and `|cwave[k]| == env[k]` for complex waveforms.

### Task 1.7 — Build Oracle Differential Fuzzer & `jt9` Loopback Suite (`tools/ftxr-oracle`)
- **Files:** `tools/ftxr-oracle/src/codec_oracle.rs`, `tools/ftxr-oracle/src/tx_loopback.rs`, `tools/ftxr-oracle/src/main.rs`.
- **Actions:**
  - Add `ftxr-oracle codec-diff` subcommand: runs `ft8code -T`, `ft4code -t`, and seeded fuzzing (5,000+ messages per mode across every `i3`/`n3` type + adversarial edge cases) comparing Rust vs `ft8code`/`ft4code` outputs (`i3`, `n3`, `msgsent`, 77-bit payload, 14-bit CRC, 83 parity bits, and 79/105 tones).
  - Add `ftxr-oracle tx-loopback` subcommand: synthesizes 12 kHz 16-bit mono WAV files (`180,000` samples for FT8, `72,576` for FT4) covering nominal starts, late starts, hot-swaps, frequency sweeps, and multi-file hash-table chains, runs `jt9 -8 -d 3 -c b -x b` and `jt9 -5 -d 3 -c b -x b`, parses decoded lines, and asserts 100% decode rate + AF/DT tolerances.
  - Add waveform numerical diff check comparing Rust `gen_ft8_wave_complex` / `gen_ft4_wave_complex` against noiseless `ft8sim` / `ft4sim` (`snr = 99`) WAV output.
- **Acceptance criteria:** Both `ftxr-oracle codec-diff` and `ftxr-oracle tx-loopback` exit `0` with 100% pass rate and emit JSON summary reports.

---

## 5. Verification, Oracle Testing & Numerical Gates

### 5.1 Differential Codec Fuzzing vs `ft8code` / `ft4code`

Command: `cargo run --release -p ftxr-oracle -- codec-diff --seed 42 --count 5000`

| Test Category | Oracle Invocation | Assertion / Gate |
| :--- | :--- | :--- |
| Upstream golden suites | `ft8code -T` (49 msgs, stateful hash table), `ft4code -t` (47 msgs, stateful hash table) | **100% exact match** on `i3.n3`, `msgsent`, `Err` (`*`) flag across every vector (including `ft4_testmsg` deprecated `0.2` fallback to `0.0`). |
| Per-type seeded fuzzing (≥5,000 msgs × 2 modes) | `ft8code "<msg>"` and `ft4code "<msg>"` (clean hash table per msg; apply `fmtmsg` identically to `ft8code`/`ft4code`) | **100% bit-exact match** on: 77-bit unscrambled payload, 77-bit FT4 scrambled payload, 14-bit CRC, 83 LDPC parity bits, 79 FT8 tones / 105 FT4 tones, `i3.n3`, and `msgsent`. |
| Type coverage quota (minimum per type) | Standard `1` (incl. `CQ`, `CQ DX`, `CQ 000..999`, `CQ AAAA`, `DE`, `QRZ`, `/R`, grids, `-50..+49`, `R-50..R+49`, `RRR`, `RR73`, `73`, no-3rd-word, `<hashed>`), EU VHF `2` (`/P`), DXpedition `0.1`, Field Day `0.3` & `0.4` (all 86 sections, `1..32`, `A..H`, with/without `R`), Telemetry `0.5` (incl. leading zeros, max `7FFF...`, overflow `8000...` → `0.0`), WSPR `0.6` (Types 1 & 2), RTTY `3` (all 171 mults + serials `0001..7999`, `TU;`, `R`), Nonstandard `4` (`CQ <nonstd>`, `<hash> <nonstd>`, `<nonstd> <hash>`, `RRR`/`RR73`/`73`), EU VHF Hash `5` (6-char grids, serials `1..4095` clamp), Free text `0.0` (all 42 chars + illegal chars). | Zero mismatches (`0 / 10,000+`). |
| Adversarial edge cases | `"K1ABC W9XYZ RR73"` (verify `igrid4 == 32373`), `"CQ KH1/KH7Z"` (verify `n12 == ihashcall("KH1/KH7Z", 12)`), `"K1ABC FN42 30"` (WSPR `0.6`), `"K1ABC FN42 73"` (WSPR `0.6` clamp `60`), `"CQ ABCD"` (free text `0.0`), `"CQ DX KH1/KH7Z"` (free text `0.0`), `"3DA0XYZ W9XYZ -10"`, `"  cq   k1abc   fn42  "`, `"0"` (all-zero telemetry). | 100% exact match with `ft8code`/`ft4code`. |

### 5.2 Waveform Numerical Verification vs `ft8sim` / `ft4sim`

- Generate noiseless reference WAVs with `ft8sim "CQ K1ABC FN42" 1500.0 0.0 0.0 0.0 1 99` (`imag(c)` = `sin(phi)` at `dt = 0.0` → starts at sample `6000`) and `ft4sim "CQ K1ABC FN42" 1500.0 0.0 0.0 0.0 1 99` (`real(c)` = `cos(phi)` at `dt = 0.0` → starts at sample `5424`).
- Compare normalized active samples against Rust `gen_ft8_wave_complex` (`.im`) and `gen_ft4_wave_complex` (`.re`).
- **Pass gate:** Maximum per-sample difference `≤ 2e-4` (accounting for 16-bit integer WAV quantization `1/32767 ≈ 3.1e-5` and Fortran `f32` vs Rust `f64` phase accumulation) and correlation coefficient `> 0.999999`.

### 5.3 TX-to-`jt9` Loopback Tests (Nominal, Late-TX, Hot-Swap, Hash-Chain)

Command: `cargo run --release -p ftxr-oracle -- tx-loopback`

Standardized oracle CLI flags (per `ROADMAP.md` Stage 0, eliminating `K1ABC`/`W9XYZ` default AP bias):
- FT8: `jt9 -8 -d 3 -c b -x b <wav_file(s)>` (WAV: 12 kHz mono `i16`, `180,000` samples, amplitude `16,000`)
- FT4: `jt9 -5 -d 3 -c b -x b <wav_file(s)>` (WAV: 12 kHz mono `i16`, `72,576` samples, amplitude `16,000`)
- Also test with small AWGN (`SNR = +10 dB` and `-10 dB`) in addition to noiseless PCM, because `jt9` baseline/SNR estimation can behave differently on pure-zero background noise vs realistic noise floor!

| Suite | Mode | Cases | Expected `jt9` Result |
| :--- | :--- | :--- | :--- |
| **Nominal multi-type** | FT8 (`delay_ms=500`, `0`) | 10 representative types (`0.0`, `0.1`, `0.3`, `0.4`, `0.5`, `1`, `2`, `3`, `4`, `5`) across `af_hz ∈ {500, 1000, 1500, 2200, 2700}` (and 48 kHz decimated 4× to 12 kHz) | 100% decode; exact message text; `|af - af_hz| ≤ 2 Hz`; `|dt - (delay_ms/1000 - 0.5)| ≤ 0.1 s` |
| **Nominal multi-type** | FT4 (`delay_ms=300`, `452`, `0`) | Same 10 representative types across `af_hz ∈ {500, 1000, 1500, 2200, 2700}` | 100% decode; exact message text; `|af - af_hz| ≤ 2 Hz`; `|dt - (delay_ms/1000 + 0.048 - 0.5)| ≤ 0.1 s` |
| **Late-start TX** | FT8 (`delay_ms=500` & `0`) | `offset_ms ∈ {1000, 2000, 3000, 4000, 5000}` placed at `slot[offset_samples..]` (leading silence `0..offset_ms`) | **100% decode** for all 1–5 s offsets; `|af - af_hz| ≤ 2 Hz`; `|dt - dt_expected| ≤ 0.1 s` (UTC-aligned!) |
| **Late-start TX** | FT4 (`delay_ms=300` & `0`) | `offset_ms ∈ {500, 1000, 1500}` placed at `slot[offset_samples..]` (leading silence `0..offset_ms`) | **100% decode** for all 0.5–1.5 s offsets; `|af - af_hz| ≤ 2 Hz`; `|dt - dt_expected| ≤ 0.1 s` (UTC-aligned!) |
| **Mid-slot hot-swap** | FT8 | Slot starts with `"CQ N1MPM FN33"` (`t = 0..T_swap`), then splices `"K1ABC N1MPM -08"` generated with `offset_ms = T_swap` for `T_swap ∈ {1500, 3000, 5000}` ms (both same AF `1500 Hz` and shifted AF `1500 → 1800 Hz`) | **100% decode of Message B** (`"K1ABC N1MPM -08"`) with `|af - af_B| ≤ 2 Hz`, `|dt - dt_expected| ≤ 0.1 s`; zero discontinuity click at splice (`tail[0] == 0.0`) |
| **Mid-slot hot-swap** | FT4 | Slot starts with `"CQ N1MPM FN33"` (`t = 0..T_swap`), then splices `"K1ABC N1MPM -08"` with `offset_ms = T_swap` for `T_swap ∈ {600, 1000, 1500}` ms (same AF & shifted AF) | **100% decode of Message B** with `|af - af_B| ≤ 2 Hz`, `|dt - dt_expected| ≤ 0.1 s` |
| **Hash-table chain** | FT8 & FT4 | Multi-WAV `jt9` run: `file1.wav` = `"CQ PJ4/K1ABC"` (Type 4), `file2.wav` = `"<PJ4/K1ABC> W9XYZ -11"` (Type 1, 22-bit hash), `file3.wav` = `"<W9XYZ> PJ4/K1ABC RR73"` (Type 4, 12-bit hash) | `jt9` resolves `<PJ4/K1ABC>` in `file2` and `<W9XYZ>` in `file3` (no `<...>`), verifying bit-exact hash compatibility across both 12-bit and 22-bit hashes! |

---

## 6. Risks, Gotchas & Cross-Stage Handoff

### 6.1 Stage 1 Risks & Mitigations

| Risk | Impact | Mitigation |
| :--- | :--- | :--- |
| **Subtle `pack77_*` fallback edge cases** (e.g., `pack77_4` with two nonstandard/compound calls without `<>`, or `pack77_3` uninitialized `nserial`) | Divergence on malformed/unusual messages in differential fuzzing | Test malformed/ambiguous inputs explicitly in `codec-diff`; keep Rust strictly safe (never UB) while matching `ft8code`/`ft4code` on every valid and documented fallback case. |
| **Non-integer `nsps` at non-standard sample rates** (e.g. FT4 at 44,100 Hz: `44100 * 0.048 = 2116.8`) | Slight symbol-rate error (`0.009%`) over 105 symbols (`<0.5 ms` drift over 5 s) | Round `nsps` to nearest integer (`2117`); validate exact integer `nsps` at 8, 12, 16, 24, 48 kHz (and 44.1 kHz for FT8 where `44100 * 0.160 = 7056` is exact). |
| **Noiseless WAV into `jt9`** (`sbase` spectrum baseline division on pure digital zero noise floor) | Occasional SNR clamp or candidate suppression in `jt9` on noiseless inputs | Run `tx-loopback` at both noiseless AND realistic AWGN (`SNR = +10 dB`, `-10 dB`) so both pure-waveform and real-channel decode paths are verified. |
| **FT4 `DT` expectation mismatch** (`delay_ms = 300 ms` + `48 ms` ramp yields `DT = -0.15 s` in `jt9 -5`, not `0.0 s`) | False test failure in loopback harness | Parameterize loopback assertions by exact theoretical formula `dt_expected = delay_ms/1000.0 + ramp_lead_sec - 0.5`. |

### 6.2 Explicit Cross-Stage Handoff Contracts

#### Consumed from Stage 0
- Workspace layout (`rust-ftx/Cargo.toml`), `Mode`, `FtxrError`, GPLv3 `LICENSE`/`NOTICE`.
- Compiled WSJT-X oracle binaries (`ft8code`, `ft4code`, `jt9`, `ft8sim`, `ft4sim`) and `ftxr-oracle` CLI skeleton.

#### Exposed to Later Stages
- **To Stage 2 (FT8 Decoder MVP):**
  - `protocol::verify_crc14(&[u8; 91]) -> bool` (used inside `decode174_91` BP codeword check).
  - `protocol::unpack77` / `unpack77_readonly` + `CallsignHashTable` (converts decoded 77 bits to text and learns callsign hashes).
  - `modulate::ft8::encode_ft8_tones_from_77bits` + `modulate::ft8::gen_ft8_wave_complex` (directly drives `subtractft8` signal reconstruction without duplicating GFSK math!).
- **To Stage 3 (FT8 Weak-Signal Parity — OSD + AP `a1`–`a6`):**
  - `protocol::ldpc::encode174_91_nocrc(&[u8; 91]) -> [u8; 174]` and `protocol::ldpc::generator_bit` (builds OSD's systematic generator matrix `gen(k, N)` in `osd174_91`).
  - `protocol::pack77`, `protocol::stdcall`, `protocol::ihashcall`, and `CallsignHashTable::save_call` (drives `ft8apset` to build `apsym(58)` and `aph10(10)` for `a1`–`a6` hypotheses).
- **To Stage 4 (FT4 Decoder Parity):**
  - `protocol::rvec::{FT4_RVEC, descramble_ft4, scramble_ft4}` (FT4 AP bit scrambling and post-LDPC payload descrambling).
  - `protocol::ldpc::encode174_91` (builds FT4's `apbits(174)` in `ft4_decode`).
  - `modulate::ft4::encode_ft4_tones_from_77bits` + `modulate::ft4::gen_ft4_wave_complex` (directly drives `subtractft4` signal reconstruction).
- **To Stage 5 (`ftxr-modem` Binary, Parallelism & Warm IPC Worker):**
  - `CallsignHashTable` (`Send + Sync`, `clear()` for `reset_session` IPC command) + `unpack77_readonly` + `apply_updates` (enables lock-free-contended Rayon parallel candidate decoding with 100% deterministic hash-table state).
  - `modulate::modulate_message` with `ModulateConfig` (serves standalone CLI encode and non-blocking concurrent TX modulate IPC requests while Pass 3 RX runs).
- **To Stage 7 (`digi-dx` Integration Blueprint):**
  - `ModulateConfig { delay_ms: 0, offset_ms, .. }` directly mirrors `digi-dx`'s `Ft8LibTxModulator::modulate_pcm_from_offset` for FT8 late-TX / hot-swap, and extends seamlessly to FT4 (`delay_ms: 300` or `0`, `offset_ms` up to 1.5 s) once `digi-dx`'s FT4 slot constants are generalized.
