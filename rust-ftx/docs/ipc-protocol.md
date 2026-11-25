# `ftxr-modem` Stdio IPC Protocol (v1 — Stage 5 Specification Stub)

> **Status:** Placeholder stub created in Stage 0. The full wire-level schema, state machine, error codes, and streaming/cancellation semantics will be finalized and implemented in Stage 5 (`plans/stage-5-modem-cli-ipc-parallelism.md`).

## Planned Framing Summary

`ftxr-modem --worker` communicates over `stdin`/`stdout` using explicit 16-byte little-endian binary headers paired with UTF-8 JSON control payloads and optional raw binary PCM attachments (`f32le` or `i16le`):

```text
Offset  Size  Field
0       4     magic    = b"FTXR"
4       2     version  = 1 (u16 LE)
6       2     flags    = 0 (u16 LE)
8       4     json_len = byte length of UTF-8 JSON payload (u32 LE)
12      4     bin_len  = byte length of raw binary attachment (u32 LE)
16      json_len  JSON payload bytes
16+J    bin_len   Binary PCM payload bytes
```

- `stdout` is strictly reserved for framed protocol messages (zero stray log output).
- All diagnostic logging is written exclusively to `stderr`.
- Canonical request/response operations:
  - `hello` -> `ready`
  - `set_context` -> `ok`
  - `session_reset` -> `ok`
  - `decode_slot` -> `0..N decode_event` + `decode_done`
  - `cancel` (`job_id`) -> `cancel_ack` + `decode_done(status="cancelled")`
  - `encode_tx` (`start_delay_ms`, `offset_ms`) -> `encode_result`
  - `ping` -> `pong`
  - `shutdown` -> `ok`
  - `error`
- Multiplexing invariant: `encode_tx` executes on an isolated OS thread outside the RX Rayon pool so an in-flight Pass 3 deep decode can never stall next-slot TX modulation or mid-slot hot-swap.
