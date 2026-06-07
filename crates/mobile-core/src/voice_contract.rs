use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Ios,
    Android,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    IosKeyboard,
    IosActionButton,
    AndroidKeyboard,
    AndroidBubble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageHint {
    Auto,
    En,
    Hi,
    Hinglish,
}

impl LanguageHint {
    pub const fn requires_roman_hinglish_guard(self) -> bool {
        matches!(self, Self::Auto | Self::Hinglish)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationStyle {
    Direct,
    Work,
    Casual,
    Email,
    Notes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorContext {
    pub before_text: String,
    pub after_text: String,
    pub selected_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetApp {
    pub label: String,
    pub bundle_id: Option<String>,
    pub field_hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileSessionRequest {
    pub client_request_id: String,
    pub device_id: String,
    pub platform: Platform,
    pub surface: Surface,
    pub language_hint: LanguageHint,
    pub style: DictationStyle,
    pub target_app: TargetApp,
    pub cursor_context: CursorContext,
    pub vocab_snapshot_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileSessionResponse {
    pub session_id: String,
    pub voice_ws_url: String,
    pub batch_url: String,
    pub session_token: String,
    pub expires_at: String,
    pub max_recording_seconds: u32,
    pub streaming_enabled: bool,
    pub current_vocab_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientVoiceEvent {
    SessionStart {
        client_request_id: String,
        audio_format: String,
        sample_rate: u32,
        channels: u8,
    },
    SessionStop,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerVoiceEvent {
    SessionReady {
        session_id: String,
    },
    SttInterim {
        text: String,
        is_final: bool,
    },
    SttFinal {
        text: String,
        confidence: Option<f32>,
    },
    PolishStarted {
        model: String,
    },
    PolishDelta {
        token: String,
    },
    GuardWarning {
        code: String,
    },
    Final {
        transcript: String,
        polished: String,
        request_id: String,
        latency_ms: u32,
    },
    Error {
        code: String,
        retryable: bool,
        message: String,
    },
}
