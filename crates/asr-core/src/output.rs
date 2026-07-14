//! The result of a successful transcription.

/// One completed local transcription plus timing breakdown.
///
/// `queue_wait_ms` and `total_ms` are filled in by the caller (the app-side
/// router) since they span the dispatch/IPC boundary; the engine itself only
/// knows `load_ms` and `inference_ms`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocalAsrOutput {
    pub transcript: String,
    /// Model file name, e.g. `ggml-oriserve-hinglish-fp16.bin`.
    pub model: String,
    /// Normalized whisper language code actually used (`en` / `hi`).
    pub language: String,
    /// Wall time from dispatch to result (caller-filled).
    pub total_ms: u64,
    /// Model (re)load time; 0 when the warm model was reused.
    pub load_ms: u64,
    /// whisper.cpp inference time.
    pub inference_ms: u64,
    /// Time the request spent queued before the engine picked it up (caller-filled).
    pub queue_wait_ms: u64,
}
