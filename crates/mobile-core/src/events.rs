use serde::{Deserialize, Serialize};

use crate::voice_contract::{Platform, Surface};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileEventType {
    SetupStarted,
    SetupAccountReady,
    SetupPrivacyAcknowledged,
    SetupMicReady,
    SetupKeyboardEnabled,
    SetupFullAccessReady,
    SetupFirstDictationDone,
    PermissionMicDenied,
    PermissionMicRevoked,
    PermissionFullAccessMissing,
    PermissionFullAccessRevoked,
    SessionCreated,
    SessionReady,
    SessionStale,
    SessionCanceled,
    SessionExpired,
    SessionAppKilled,
    AudioStarted,
    AudioFirstFrameSent,
    AudioRouteChanged,
    AudioInterrupted,
    AudioStopped,
    AudioUploadFailed,
    GatewayWsConnected,
    GatewayWsFailed,
    GatewayBatchFallbackUsed,
    GatewayProviderTimeout,
    GatewayProviderError,
    InsertReady,
    InsertSucceeded,
    InsertFailed,
    InsertCopied,
    InsertSavedToHistory,
    InsertDuplicateSuppressed,
    CorrectionLearnSpellingTapped,
    CorrectionFeedbackSubmitted,
    CorrectionReviewQueued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedContext {
    pub host_app_label: Option<String>,
    pub field_hint: Option<String>,
    pub network_type: Option<String>,
    pub latency_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileEvent {
    pub event_id: String,
    pub schema: String,
    pub occurred_at: String,
    pub device_id: String,
    pub session_id: Option<String>,
    pub client_request_id: Option<String>,
    pub build: String,
    pub platform: Platform,
    pub surface: Surface,
    pub event_type: MobileEventType,
    pub redacted_context: RedactedContext,
}
