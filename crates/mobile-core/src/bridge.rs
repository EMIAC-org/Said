use serde::{Deserialize, Serialize};

use crate::voice_contract::{DictationStyle, LanguageHint, Surface};

pub const IOS_BRIDGE_SESSION_SCHEMA: &str = "airnote.ios.bridge.session.v1";
pub const IOS_BRIDGE_COMMAND_SCHEMA: &str = "airnote.ios.bridge.command.v1";
pub const IOS_BRIDGE_RESULT_SCHEMA: &str = "airnote.ios.bridge.result.v1";
pub const IOS_BRIDGE_ACK_SCHEMA: &str = "airnote.ios.bridge.ack.v1";
pub const IOS_BRIDGE_HEALTH_SCHEMA: &str = "airnote.ios.bridge.health.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSessionState {
    NotConfigured,
    NeedsFullAccess,
    NeedsMainAppSession,
    SessionStartRequested,
    Ready,
    Recording,
    Processing,
    InsertReady,
    Inserted,
    Error,
    StaleSession,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeSession {
    pub schema: String,
    pub session_id: String,
    pub device_id: String,
    pub state: BridgeSessionState,
    pub started_at: String,
    pub expires_at: String,
    pub heartbeat_at: String,
    pub language_hint: LanguageHint,
    pub style: DictationStyle,
    pub surface: Surface,
    pub gateway_region: String,
    pub result_seq: u64,
    pub command_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardContext {
    pub before_text: String,
    pub after_text: String,
    pub selected_text: String,
    pub host_app_label: String,
    pub field_hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeCommandKind {
    StartSession,
    StartRecording,
    StopRecording,
    CancelRecording,
    RequestInsert,
    ClearState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeCommand {
    pub schema: String,
    pub command_id: String,
    pub command_seq: u64,
    pub kind: BridgeCommandKind,
    pub created_at: String,
    pub keyboard_context: KeyboardContext,
    pub language_hint: LanguageHint,
    pub style: DictationStyle,
    pub client_request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeResultState {
    Partial,
    Final,
    Error,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertPolicy {
    InsertAtCursor,
    ReplaceSelectedText,
    CopyOnly,
    SaveToHistory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeResult {
    pub schema: String,
    pub result_seq: u64,
    pub session_id: String,
    pub client_request_id: String,
    pub request_id: String,
    pub state: BridgeResultState,
    pub transcript: String,
    pub polished: String,
    pub language: LanguageHint,
    pub style: DictationStyle,
    pub latency_ms: u32,
    pub created_at: String,
    pub expires_at: String,
    pub insert_policy: InsertPolicy,
    pub learning_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Inserted,
    Copied,
    SavedToHistory,
    Canceled,
    Failed,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeAck {
    pub schema: String,
    pub result_seq: u64,
    pub session_id: String,
    pub client_request_id: String,
    pub outcome: TerminalOutcome,
    pub acknowledged_at: String,
}

#[must_use]
pub const fn is_newer_sequence(last_seen: u64, candidate: u64) -> bool {
    candidate > last_seen
}

#[must_use]
pub fn is_terminal_outcome(outcome: TerminalOutcome) -> bool {
    matches!(
        outcome,
        TerminalOutcome::Inserted
            | TerminalOutcome::Copied
            | TerminalOutcome::SavedToHistory
            | TerminalOutcome::Canceled
            | TerminalOutcome::Failed
            | TerminalOutcome::Expired
    )
}
