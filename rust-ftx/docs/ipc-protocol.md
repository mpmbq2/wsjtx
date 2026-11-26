# `ftxr-modem` Warm Stdio IPC Protocol (v1)

`ftxr-modem --worker` implements a persistent, bidirectional, length-prefixed binary-framed RPC and streaming event protocol over standard input (`stdin`) and standard output (`stdout`). All diagnostic logs are written strictly to `stderr`; `stdout` is reserved exclusively for framed `FTXR` v1 packets.

---

## 1. Wire Framing (16-Byte Little-Endian Header)

Every message on `stdin` and `stdout` uses an exact 16-byte fixed little-endian header immediately followed by `json_len` bytes of UTF-8 JSON and `payload_len` bytes of optional binary PCM audio:

| Byte Offset | Size | Type | Field | Description |
| :--- | :--- | :--- | :--- | :--- |
| `0..4` | 4 | `[u8; 4]` | `magic` | ASCII `FTXR` (`0x46, 0x54, 0x58, 0x52`) |
| `4..6` | 2 | `u16` (LE) | `version` | Protocol version; MUST be `1` (`FTXR_IPC_VERSION`) |
| `6..8` | 2 | `u16` (LE) | `flags` | Bitmask (`0x0001`: binary payload present; `0x0002`: binary sample format hint `1=f32le`, `0=i16le`) |
| `8..12` | 4 | `u32` (LE) | `json_len` | UTF-8 JSON body byte length (`1 ..= 1 MiB`) |
| `12..16` | 4 | `u32` (LE) | `payload_len` | Binary payload byte length (`0 ..= 64 MiB`) |

- Total frame payload cap (`json_len + payload_len`) MUST not exceed `64 MiB`.
- If `magic != b"FTXR"` or size limits are violated, the worker considers framing desynchronized and exits with code **`3`**.
- Clean `stdin` EOF on an exact frame boundary exits gracefully with code **`0`**. Unexpected EOF mid-header or mid-body exits with code **`3`**.

---

## 2. Handshake & Worker Exit Codes

1. The client's **first** frame MUST be `{"op": "hello", "protocol_version": 1, ...}`.
2. If `protocol_version != 1`, the worker emits an `error` frame (`"code": "version_mismatch"`, `"fatal": true`), flushes `stdout`, and exits with code **`4`**.
3. If `protocol_version == 1`, the worker constructs/updates its `DecoderEngine` (`threads`), emits `{"op": "ready", ...}`, and enters the multiplexed command loop.
4. Any other malformed JSON or invalid request returns a non-fatal `{"op": "error", "fatal": false, ...}` frame and keeps the worker alive.

| Exit Code | Meaning |
| :--- | :--- |
| `0` | Clean `shutdown` command processed or graceful EOF on `stdin` |
| `2` | CLI argument / usage error before entering worker mode |
| `3` | Fatal frame desync (`FTXR` magic mismatch, truncated frame, oversized lengths) |
| `4` | Handshake failure or unsupported `protocol_version` |

---

## 3. Concurrency Model (4-Role Isolation)

`ftxr-modem --worker` guarantees sub-millisecond TX response latency even while all RX cores are saturated on a crowded Pass 3:

1. **Reader Thread (`stdin`):** Reads frames exclusively, routes `encode_tx` immediately to a dedicated `TXTHR` OS thread, updates active decode cancellation / deadline on `cancel`, handles `ping` inline or immediately, and serializes/submits `decode_slot` jobs to the RX coordinator (`RXCOORD`).
2. **TX Thread (`TXTHR`):** Runs `DecoderEngine::modulate` on a dedicated non-Rayon OS thread without locking RX FFT/scratch pools.
3. **RX Coordinator (`RXCOORD`) + Rayon Pool (`ftxr-rx-*`):** Executes `DecoderEngine::decode_i16_streaming` / `decode_streaming`, streaming deduplicated `decode_event` packets in canonical order immediately to the outbound channel.
4. **Deadline Watchdog (`DEADLINE`) + Single Writer Thread (`WRITER`):** A shared mutex+condvar watchdog triggers cooperative `AtomicBool` cancellation when `timeout_ms` expires (`status: "timeout"`); a single `BufWriter<StdoutLock>` writer thread drains a `crossbeam-channel` bounded/unbounded queue and flushes per frame so frames never interleave.

---

## 4. Client -> Worker Operations

### `hello`
```json
{
  "op": "hello",
  "id": "init-1",
  "protocol_version": 1,
  "client": "digi-dx",
  "client_name": "digi-dx",
  "client_version": "0.1.0",
  "threads": null,
  "enable_telemetry_default": false
}
```
- `threads`: optional `usize >= 1` (`null` / omitted uses the auto CPU-headroom heuristic).
- `enable_telemetry_default`: optional `bool` default applied to `decode_slot` when `enable_telemetry` is omitted.

### `set_context`
Updates persistent session QSO context across subsequent `decode_slot` calls:
```json
{
  "op": "set_context",
  "id": "ctx-1",
  "my_call": "N0AAA",
  "dx_call": "K9BBB",
  "dx_grid": "DM79",
  "rx_freq_hz": 1500.0,
  "qso_freq_hz": 1500.0,
  "tx_freq_hz": 1500.0,
  "freq_tol_hz": 20.0,
  "qso_progress": 2,
  "contest_mode": "none",
  "enable_ap": true,
  "ap_cq_only": false,
  "ap_width_hz": 75.0
}
```
- Both `rx_freq_hz` and alias `qso_freq_hz` are accepted.
- `contest_mode` accepts lowercase string names (`"none"`, `"na_vhf"`, `"eu_vhf"`, `"field_day"`, `"rtty_ru"`, `"ww_digi"`, `"fox"`, `"hound"`) or numeric `0..=6`.

### `session_reset`
Clears session `CallsignHashTable` entries (typically on band/frequency change):
```json
{ "op": "session_reset", "id": "rst-1" }
```

### `decode_slot` (with binary PCM payload)
```json
{
  "op": "decode_slot",
  "id": "dec-42",
  "slot_id": "210703_133430_p3",
  "mode": "ft8",
  "depth": 3,
  "freq_min_hz": 200.0,
  "freq_max_hz": 4000.0,
  "rx_freq_hz": 1500.0,
  "freq_tol_hz": 20.0,
  "sample_rate": 12000,
  "sample_format": "i16le",
  "early_pass": false,
  "qso_first": true,
  "enable_telemetry": false,
  "timeout_ms": 1500
}
```
- `sample_format`: `"i16le"` (2 bytes/sample) or `"f32le"` (4 bytes/sample). `sample_rate` MUST be `12000`.
- Binary payload byte length MUST be exact multiple of sample width (`2` for `i16le`, `4` for `f32le`).
- `qso_override`: optional inline `set_context` fields applying solely to this slot without mutating persistent context.
- Single active decode invariant: if a decode is already running when another `decode_slot` arrives, the worker cooperates with cancellation or rejects with non-fatal `"error"` (`code: "busy"`) depending on `replace_active` (default cancels prior active slot before running next or returns `"busy"` if `replace_active: false`; standard sequential slots return cleanly in order). Wait: default cancels superseded decode? Better: reject or serialize via queue cleanly — specifically, queue bounded/cancel active slot cleanly or execute on `RXCOORD`! Wait: returning `"busy"` immediately if an uncancelled slot is active while canceling smoothly via explicit `cancel` keeps semantics crystal clear, OR cancelling prior active slot if `cancel_previous: true`! Document exact behavior: explicit `cancel` aborts active decode; `decode_slot` runs sequentially on `RXCOORD` (queue depth bound e.g. 4, or immediate error if overloaded).

### `cancel`
Cancels an active or queued `decode_slot`:
```json
{ "op": "cancel", "id": "c-1", "target_id": "dec-42" }
```
- If `target_id` is omitted, cancels the currently active decode (if any).
- Responds immediately with `cancel_ack`; the cancelled decode terminates within `<50 ms` and emits `decode_done` with `"status": "cancelled"`.

### `encode_tx`
```json
{
  "op": "encode_tx",
  "id": "tx-1",
  "mode": "ft8",
  "message": "CQ N0AAA EM10",
  "af_hz": 1500.0,
  "sample_rate": 12000,
  "sample_format": "f32le",
  "start_delay_ms": 500,
  "offset_ms": 0,
  "pad_to_slot": false,
  "amplitude": 1.0
}
```
- Responds with `encode_result` + binary PCM payload (`f32le` in `[-1.0, 1.0]` scaled by `amplitude`, or `i16le`).

### `ping` & `shutdown`
```json
{ "op": "ping", "id": "p-1", "seq": 7 }
{ "op": "shutdown", "id": "bye-1" }
```

---

## 5. Worker -> Client Messages

- `ready`: `{"op":"ready","id":...,"protocol_version":1,"version":"0.1.0","threads":N,"modes":["ft8","ft4"],"capabilities":["streaming_decode","cancel","telemetry","concurrent_encode"]}`
- `ok`: `{"op":"ok","id":...,"entries_cleared":usize?}`
- `decode_event`:
```json
{
  "op": "decode_event",
  "id": "dec-42",
  "slot_id": "210703_133430_p3",
  "decode": {
    "mode": "ft8",
    "snr_db": -14,
    "snr_db_exact": -14.2,
    "dt_sec": 0.6,
    "af_hz": 1030.4,
    "af_hz_rounded": 1030,
    "message": "CQ LZ1JZ KN22",
    "ap_tag": "a1",
    "uncertain": false,
    "raw_metadata": "a1",
    "telemetry": null
  }
}
```
- `decode_done`:
```json
{
  "op": "decode_done",
  "id": "dec-42",
  "slot_id": "210703_133430_p3",
  "status": "completed",
  "decoded_count": 12,
  "elapsed_ms": 84.3
}
```
- `cancel_ack`: `{"op":"cancel_ack","id":...,"target_id":...,"cancelled":true}`
- `encode_result` (+ binary payload):
```json
{
  "op": "encode_result",
  "id": "tx-1",
  "mode": "ft8",
  "message": "CQ N0AAA EM10",
  "af_hz": 1500.0,
  "sample_rate": 12000,
  "sample_format": "f32le",
  "sample_count": 157680,
  "duration_ms": 13140.0,
  "elapsed_ms": 1.8
}
```
- `pong`: `{"op":"pong","id":...,"seq":7,"active_decode_id":null,"hash_entries":14}`
- `error`: `{"op":"error","id":...,"code":"invalid_request","message":"...","fatal":false}`
