use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptOrigin {
    DictationLocal,
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
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub origin: TranscriptOrigin,
}
