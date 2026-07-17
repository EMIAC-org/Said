use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptOrigin {
    DictationLocal,
    /// Dictation transcribed by a hosted (cloud) STT provider — the Windows path.
    DictationHosted,
    MeetingLocal,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TranscriptMeta {
    #[serde(default)]
    pub enriched_transcript: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub mean_word_confidence: f64,
    #[serde(default)]
    pub low_confidence_count: usize,
    #[serde(default)]
    pub word_count: usize,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub model: String,
    /// Stable provider identifier for telemetry and cost attribution.
    /// Examples: `local_whisper`, `local_nemotron`, `deepinfra`.
    #[serde(default)]
    pub provider: String,
    /// Concrete execution path for this transcript, such as `local_batch` or
    /// `http_batch`. This is per-run metadata, not a selected preference.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub origin: TranscriptOrigin,
}
