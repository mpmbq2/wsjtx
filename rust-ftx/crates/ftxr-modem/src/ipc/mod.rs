// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Warm stdio IPC v1 framing, message schema, and 4-role `--worker` daemon.

pub mod framing;
pub mod protocol;
pub mod worker;

pub use framing::{FTXR_IPC_VERSION, FTXR_MAGIC};
pub use worker::{run_worker, WorkerExit};
