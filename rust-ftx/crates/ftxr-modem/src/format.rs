// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Output formatting (`jt9`, `json`, `jsonl`) and UTC extraction compatible with `digi-dx` and WSJT-X.

use std::path::Path;

use clap::ValueEnum;
use serde::Serialize;

use ftxr_core::{DecodeTelemetry, DecodedMessage, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Jt9,
    Json,
    Jsonl,
}

#[derive(Debug, Serialize)]
pub struct JsonDecodeItem<'a> {
    pub utc: &'a str,
    pub mode: Mode,
    pub snr_db: i8,
    pub snr_db_exact: f32,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub af_hz_rounded: i32,
    pub message: &'a str,
    pub ap_tag: Option<String>,
    pub uncertain: bool,
    pub raw_metadata: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<&'a DecodeTelemetry>,
}

impl<'a> JsonDecodeItem<'a> {
    pub fn from_decoded(utc: &'a str, dec: &'a DecodedMessage) -> Self {
        let mut dt_sec = dec.dt_sec;
        if dt_sec == 0.0 {
            dt_sec = 0.0;
        }
        let (ap_tag, uncertain) = match dec.ap_tag {
            ftxr_core::ApTag::None => (None, false),
            ftxr_core::ApTag::Ap { kind, uncertain } => (Some(format!("a{kind}")), uncertain),
        };
        Self {
            utc,
            mode: dec.mode,
            snr_db: dec.snr_db,
            snr_db_exact: dec.snr_raw,
            dt_sec,
            af_hz: dec.af_hz,
            af_hz_rounded: dec.af_hz.round() as i32,
            message: &dec.message,
            ap_tag,
            uncertain,
            raw_metadata: dec.wsjtx_metadata(),
            telemetry: dec.telemetry.as_ref(),
        }
    }
}

/// Derives a 6-digit `HHMMSS` UTC tag from an optional override or WSJT-X filename (`YYMMDD_HHMMSS.wav`).
pub fn resolve_utc_tag(explicit: Option<&str>, path: Option<&Path>) -> String {
    if let Some(u) = explicit {
        let digits: String = u.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 6 {
            return digits[digits.len() - 6..].to_string();
        }
        if !digits.is_empty() {
            return format!("{digits:0>6}");
        }
    }
    if let Some(p) = path {
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            // Match `YYMMDD_HHMMSS` or trailing `_HHMMSS`
            if let Some((_, tail)) = stem.rsplit_once('_') {
                if tail.len() == 6 && tail.chars().all(|c| c.is_ascii_digit()) {
                    return tail.to_string();
                }
            }
            if stem.len() == 6 && stem.chars().all(|c| c.is_ascii_digit()) {
                return stem.to_string();
            }
        }
    }
    "000000".to_string()
}

/// Formats a single `DecodedMessage` in classic WSJT-X `jt9` text output format compatible
/// with `digi-dx`'s `parse_jt9_line`.
pub fn format_jt9_line(utc: &str, dec: &DecodedMessage) -> String {
    let mut dt = dec.dt_sec;
    if dt == 0.0 || dt.abs() < 0.05 {
        dt = 0.0;
    }
    let af = dec.af_hz.round() as i32;
    let marker = dec.mode.jt9_mode_marker();
    match dec.wsjtx_metadata() {
        Some(meta) => {
            if let Some(stripped) = meta.strip_prefix("? ") {
                format!(
                    "{utc:6} {:>3} {:>4.1} {:>4} {marker}  {:<36} ? {stripped}",
                    dec.snr_db, dt, af, dec.message
                )
            } else {
                format!(
                    "{utc:6} {:>3} {:>4.1} {:>4} {marker}  {:<36}   {meta}",
                    dec.snr_db, dt, af, dec.message
                )
            }
        }
        None => format!(
            "{utc:6} {:>3} {:>4.1} {:>4} {marker}  {:<36}",
            dec.snr_db, dt, af, dec.message
        ),
    }
}

pub fn format_decode_finished(nsynced: usize, ndecoded: usize) -> String {
    format!("<DecodeFinished>   {nsynced}   {ndecoded}        0")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ftxr_core::ApTag;

    #[test]
    fn formats_jt9_lines_and_resolves_utc() {
        assert_eq!(
            resolve_utc_tag(None, Some(Path::new("/tmp/210703_133430.wav"))),
            "133430"
        );
        assert_eq!(resolve_utc_tag(Some("12:34:56"), None), "123456");

        let d1 = DecodedMessage {
            mode: Mode::Ft8,
            snr_db: -4,
            snr_raw: -4.2,
            dt_sec: 0.7,
            af_hz: 683.2,
            message: "CQ TA6CQ KN70".to_string(),
            ap_tag: ApTag::None,
            telemetry: None,
        };
        let s1 = format_jt9_line("133430", &d1);
        assert!(s1.starts_with("133430  -4  0.7  683 ~  CQ TA6CQ KN70"));

        let d2 = DecodedMessage {
            mode: Mode::Ft4,
            snr_db: -18,
            snr_raw: -18.1,
            dt_sec: 0.2,
            af_hz: 1500.0,
            message: "N0AAA K9BBB RR73".to_string(),
            ap_tag: ApTag::Ap {
                kind: 6,
                uncertain: true,
            },
            telemetry: None,
        };
        let s2 = format_jt9_line("000002", &d2);
        assert!(s2.contains(" +  N0AAA K9BBB RR73"));
        assert!(s2.ends_with("? a6"));

        let d3 = DecodedMessage {
            mode: Mode::Ft8,
            snr_db: -14,
            snr_raw: -14.0,
            dt_sec: 0.6,
            af_hz: 1030.0,
            message: "CQ LZ1JZ KN22".to_string(),
            ap_tag: ApTag::Ap {
                kind: 1,
                uncertain: false,
            },
            telemetry: None,
        };
        let s3 = format_jt9_line("110200", &d3);
        assert!(s3.ends_with("   a1"));
    }
}
