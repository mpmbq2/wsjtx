// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! Authoritative JSON wire schemas for `FTXR` IPC v1 ops and responses.

use serde::{Deserialize, Serialize};

use ftxr_core::{ApTag, ContestMode, DecodeTelemetry, DecodedMessage, Mode, QsoContext};

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ContestField {
    Name(String),
    Code(u8),
}

impl ContestField {
    pub fn to_contest_mode(&self) -> Result<ContestMode, String> {
        match self {
            ContestField::Code(c) => match c {
                0 => Ok(ContestMode::None),
                1 => Ok(ContestMode::NaVhf),
                2 => Ok(ContestMode::EuVhf),
                3 => Ok(ContestMode::FieldDay),
                4 => Ok(ContestMode::RttyRoundup),
                5 => Ok(ContestMode::WwDigi),
                6 => Ok(ContestMode::Fox),
                7 => Ok(ContestMode::Hound),
                8 => Ok(ContestMode::ArrlDigi),
                _ => Err(format!("unknown numeric contest_mode {c}")),
            },
            ContestField::Name(s) => match s.trim().to_ascii_lowercase().as_str() {
                "none" | "" => Ok(ContestMode::None),
                "na_vhf" | "navhf" => Ok(ContestMode::NaVhf),
                "eu_vhf" | "euvhf" => Ok(ContestMode::EuVhf),
                "field_day" | "fieldday" | "fd" => Ok(ContestMode::FieldDay),
                "rtty_ru" | "rttyru" | "ru" | "rtty_roundup" => Ok(ContestMode::RttyRoundup),
                "ww_digi" | "wwdigi" => Ok(ContestMode::WwDigi),
                "fox" => Ok(ContestMode::Fox),
                "hound" => Ok(ContestMode::Hound),
                "arrl_digi" | "arrldigi" => Ok(ContestMode::ArrlDigi),
                other => Err(format!("unknown contest_mode '{other}'")),
            },
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContextPayload {
    pub my_call: Option<String>,
    #[serde(alias = "his_call")]
    pub dx_call: Option<String>,
    #[serde(alias = "his_grid")]
    pub dx_grid: Option<String>,
    #[serde(alias = "qso_freq_hz")]
    pub rx_freq_hz: Option<f32>,
    pub tx_freq_hz: Option<f32>,
    pub freq_tol_hz: Option<f32>,
    pub qso_progress: Option<u8>,
    #[serde(alias = "contest")]
    pub contest_mode: Option<ContestField>,
    #[serde(alias = "ap_enabled")]
    pub enable_ap: Option<bool>,
    pub ap_cq_only: Option<bool>,
    pub ap_width_hz: Option<f32>,
}

pub type AppliedQsoContext = (Option<QsoContext>, Option<f32>, Option<f32>);

impl ContextPayload {
    /// Applies non-None fields to an existing `QsoContext` (or builds one) cleanly.
    pub fn apply_to(
        &self,
        existing: Option<&QsoContext>,
        mode: Mode,
        default_rx_hz: f32,
    ) -> Result<AppliedQsoContext, String> {
        let mut qso = existing.cloned().unwrap_or(QsoContext {
            my_call: None,
            his_call: None,
            his_grid: None,
            qso_progress: 0,
            contest: ContestMode::None,
            tx_freq_hz: default_rx_hz,
            rx_freq_hz: default_rx_hz,
            ap_enabled: true,
            ap_cq_only: false,
            ap_width_hz: mode.default_ap_width_hz(),
        });

        if self.my_call.is_some() {
            qso.my_call = self.my_call.clone().filter(|s| !s.trim().is_empty());
        }
        if self.dx_call.is_some() {
            qso.his_call = self.dx_call.clone().filter(|s| !s.trim().is_empty());
        }
        if self.dx_grid.is_some() {
            qso.his_grid = self.dx_grid.clone().filter(|s| !s.trim().is_empty());
        }
        if let Some(rx) = self.rx_freq_hz {
            qso.rx_freq_hz = rx;
        }
        if let Some(tx) = self.tx_freq_hz {
            qso.tx_freq_hz = tx;
        }
        if let Some(p) = self.qso_progress {
            qso.qso_progress = p;
        }
        if let Some(ref c) = self.contest_mode {
            qso.contest = c.to_contest_mode()?;
        }
        if let Some(ap) = self.enable_ap {
            qso.ap_enabled = ap;
        }
        if let Some(cq_only) = self.ap_cq_only {
            qso.ap_cq_only = cq_only;
        }
        if let Some(w) = self.ap_width_hz {
            qso.ap_width_hz = w;
        }

        let has_any = qso.my_call.is_some()
            || qso.his_call.is_some()
            || qso.his_grid.is_some()
            || qso.qso_progress != 0
            || qso.contest != ContestMode::None
            || !qso.ap_enabled
            || qso.ap_cq_only;
        let out_qso = if has_any || existing.is_some() {
            Some(qso)
        } else {
            None
        };
        Ok((out_qso, self.rx_freq_hz, self.freq_tol_hz))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum InboundOp {
    Hello {
        #[serde(default)]
        id: Option<String>,
        protocol_version: u16,
        #[serde(default)]
        client: Option<String>,
        #[serde(default)]
        client_name: Option<String>,
        #[serde(default)]
        client_version: Option<String>,
        #[serde(default)]
        threads: Option<usize>,
        #[serde(default)]
        enable_telemetry_default: Option<bool>,
    },
    SetContext {
        #[serde(default)]
        id: Option<String>,
        #[serde(flatten)]
        ctx: ContextPayload,
    },
    SessionReset {
        #[serde(default)]
        id: Option<String>,
    },
    DecodeSlot {
        id: String,
        #[serde(default)]
        slot_id: Option<String>,
        mode: Mode,
        #[serde(default)]
        depth: Option<u8>,
        #[serde(default)]
        freq_min_hz: Option<f32>,
        #[serde(default)]
        freq_max_hz: Option<f32>,
        #[serde(default)]
        rx_freq_hz: Option<f32>,
        #[serde(default)]
        freq_tol_hz: Option<f32>,
        #[serde(default)]
        sample_rate: Option<u32>,
        #[serde(default)]
        sample_format: Option<String>,
        #[serde(default)]
        early_pass: Option<bool>,
        #[serde(default)]
        qso_first: Option<bool>,
        #[serde(default)]
        enable_telemetry: Option<bool>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        utc_hhmmss: Option<String>,
        #[serde(default)]
        qso_override: Option<ContextPayload>,
    },
    Cancel {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        target_id: Option<String>,
    },
    EncodeTx {
        id: String,
        mode: Mode,
        message: String,
        af_hz: f32,
        #[serde(default)]
        sample_rate: Option<u32>,
        #[serde(default)]
        sample_format: Option<String>,
        #[serde(default)]
        start_delay_ms: Option<u32>,
        #[serde(default)]
        offset_ms: Option<u64>,
        #[serde(default)]
        pad_to_slot: Option<bool>,
        #[serde(default)]
        amplitude: Option<f32>,
    },
    Ping {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        seq: Option<u64>,
    },
    Shutdown {
        #[serde(default)]
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WireDecode {
    pub mode: Mode,
    pub snr_db: i8,
    pub snr_db_exact: f32,
    pub dt_sec: f32,
    pub af_hz: f32,
    pub af_hz_rounded: i32,
    pub message: String,
    pub ap_tag: Option<String>,
    pub uncertain: bool,
    pub raw_metadata: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<DecodeTelemetry>,
}

impl From<DecodedMessage> for WireDecode {
    fn from(dec: DecodedMessage) -> Self {
        let mut dt_sec = dec.dt_sec;
        if dt_sec == 0.0 {
            dt_sec = 0.0;
        }
        let raw_metadata = dec.wsjtx_metadata();
        let (ap_tag, uncertain) = match dec.ap_tag {
            ApTag::None => (None, false),
            ApTag::Ap { kind, uncertain } => (Some(format!("a{kind}")), uncertain),
        };
        let sync_score = dec.telemetry.as_ref().map(|t| t.sync_score);
        let quality = dec.telemetry.as_ref().map(|t| t.quality);
        Self {
            mode: dec.mode,
            snr_db: dec.snr_db,
            snr_db_exact: dec.snr_raw,
            dt_sec,
            af_hz: dec.af_hz,
            af_hz_rounded: dec.af_hz.round() as i32,
            message: dec.message,
            ap_tag,
            uncertain,
            raw_metadata,
            sync_score,
            quality,
            telemetry: dec.telemetry,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OutboundOp {
    Ready {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        protocol_version: u16,
        version: &'static str,
        threads: usize,
        modes: [&'static str; 2],
        capabilities: [&'static str; 4],
    },
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entries_cleared: Option<usize>,
    },
    DecodeEvent {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        slot_id: Option<String>,
        decode: WireDecode,
    },
    DecodeDone {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        slot_id: Option<String>,
        status: &'static str,
        decoded_count: usize,
        elapsed_ms: f64,
    },
    CancelAck {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        cancelled: bool,
    },
    EncodeResult {
        id: String,
        mode: Mode,
        message: String,
        af_hz: f32,
        sample_rate: u32,
        sample_format: &'static str,
        sample_count: usize,
        duration_ms: f64,
        elapsed_ms: f64,
    },
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        active_decode_id: Option<String>,
        hash_entries: usize,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        code: &'static str,
        message: String,
        fatal: bool,
    },
}

/// Extracts optional `"id"` field from malformed/version-mismatched JSON when full deserialization fails.
pub fn extract_loose_id(json: &[u8]) -> Option<String> {
    let val: serde_json::Value = serde_json::from_slice(json).ok()?;
    val.get("id")?.as_str().map(str::to_string)
}
