// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Standalone `ftxr-modem` CLI and persistent warm stdio IPC worker binary (Stage 0 stub).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ftxr_core::Mode;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n",
    "Copyright (C) 2026 ftxr-modem Contributors\n",
    "Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).\n",
    "License GPLv3: GNU General Public License version 3 only <https://www.gnu.org/licenses/gpl-3.0.html>.\n",
    "This is free software: you are free to change and redistribute it. There is NO WARRANTY, to the extent permitted by law."
);

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliMode {
    Ft8,
    Ft4,
}

impl From<CliMode> for Mode {
    fn from(value: CliMode) -> Self {
        match value {
            CliMode::Ft8 => Mode::Ft8,
            CliMode::Ft4 => Mode::Ft4,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "ftxr-modem",
    version = env!("CARGO_PKG_VERSION"),
    long_version = LONG_VERSION,
    about = "Pure-Rust FT8 & FT4 standalone modem CLI and warm stdio IPC worker"
)]
struct Cli {
    /// Run as a persistent warm framed-stdio IPC worker (implemented in Stage 5).
    #[arg(long)]
    worker: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Decode one or more 12 kHz mono PCM WAV files (implemented in Stage 5).
    Decode {
        /// Digital mode to decode.
        #[arg(long, value_enum, default_value_t = CliMode::Ft8)]
        mode: CliMode,
        /// Decode depth (1=Fast, 2=Normal, 3=Deep).
        #[arg(short, long, default_value_t = 3)]
        depth: u8,
        /// Input WAV file path(s).
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
    },
    /// Encode and GFSK-modulate a message into a WAV file (implemented in Stage 5).
    Encode {
        /// Digital mode to encode.
        #[arg(long, value_enum, default_value_t = CliMode::Ft8)]
        mode: CliMode,
        /// Base audio frequency in Hz.
        #[arg(long, default_value_t = 1500.0)]
        af_hz: f32,
        /// Output sample rate in Hz.
        #[arg(long, default_value_t = 12_000)]
        sample_rate: u32,
        /// Output WAV file path.
        #[arg(short, long)]
        out: PathBuf,
        /// FT8/FT4 message text to encode.
        message: String,
    },
}

fn run(cli: Cli) -> ExitCode {
    if cli.worker {
        eprintln!("ftxr-modem --worker is not yet implemented (scheduled for Stage 5)");
        return ExitCode::from(2);
    }

    match cli.command {
        Some(Commands::Decode { mode, depth, inputs }) => {
            let mode: Mode = mode.into();
            eprintln!(
                "ftxr-modem decode (mode={mode:?}, depth={depth}, files={}) is not yet implemented (scheduled for Stage 5)",
                inputs.len()
            );
            ExitCode::from(2)
        }
        Some(Commands::Encode {
            mode,
            af_hz,
            sample_rate,
            out,
            message,
        }) => {
            let mode: Mode = mode.into();
            eprintln!(
                "ftxr-modem encode (mode={mode:?}, af_hz={af_hz}, sr={sample_rate}, out={}, msg={message:?}) is not yet implemented (scheduled for Stage 5)",
                out.display()
            );
            ExitCode::from(2)
        }
        None => {
            eprintln!("Usage: ftxr-modem [--worker | decode ... | encode ...] (try --help or --version)");
            ExitCode::from(2)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    run(cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_worker_decode_and_encode() {
        let worker = Cli::try_parse_from(["ftxr-modem", "--worker"]).unwrap();
        assert!(worker.worker);
        assert_eq!(run(worker), ExitCode::from(2));

        let decode = Cli::try_parse_from([
            "ftxr-modem",
            "decode",
            "--mode",
            "ft8",
            "-d",
            "3",
            "000000_000001.wav",
        ])
        .unwrap();
        assert!(!decode.worker);
        assert_eq!(run(decode), ExitCode::from(2));

        let encode = Cli::try_parse_from([
            "ftxr-modem",
            "encode",
            "--mode",
            "ft4",
            "--out",
            "out.wav",
            "CQ N0AAA EM10",
        ])
        .unwrap();
        assert_eq!(run(encode), ExitCode::from(2));
    }

    #[test]
    fn long_version_contains_gplv3_and_wsjtx_attribution() {
        assert!(LONG_VERSION.contains("GPLv3"));
        assert!(LONG_VERSION.contains("WSJT-X"));
        assert!(LONG_VERSION.contains("NO WARRANTY"));
    }
}
