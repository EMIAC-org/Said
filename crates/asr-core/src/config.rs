//! Resolved per-request dictation settings.
//!
//! This is a plain, serializable data record. It is *produced* by the app
//! (`meeting_engine::resolve_dictation_local_asr_config`, which reads the model
//! path, VAD model, and env knobs) and *consumed* by the whisper engine —
//! either in-process or after crossing the worker IPC boundary. Keeping it here
//! lets both sides share one definition; the app re-exports it so existing
//! `crate::meeting_engine::DictationLocalAsrConfig` references keep working.

use std::path::PathBuf;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DictationLocalAsrConfig {
    /// Absolute path to the ggml whisper model file.
    pub model: PathBuf,
    /// Preferred language hint (`en` / `hi` / `hinglish`); normalized by the engine.
    pub language: String,
    /// `whisper_full_params.n_max_text_ctx` (-1 = model default).
    pub max_context_tokens: i32,
    /// Optional initial prompt biasing.
    pub prompt: Option<String>,
    /// Suppress non-speech tokens (`-sns`).
    pub suppress_non_speech: bool,
    /// Disable temperature fallback (keeps a single deterministic pass).
    pub no_fallback: bool,
    /// No-speech probability gate, if enabled.
    pub no_speech_threshold: Option<f32>,
    /// Log-probability gate, if enabled.
    pub logprob_threshold: Option<f32>,
    /// Entropy gate, if enabled.
    pub entropy_threshold: Option<f32>,
    /// Silero VAD model path; `None` disables VAD.
    pub vad_model: Option<PathBuf>,
    pub vad_threshold: f32,
    pub vad_speech_pad_ms: i32,
    pub vad_min_silence_ms: i32,
    /// Romanize Devanagari output to Hinglish.
    pub romanize: bool,
}
