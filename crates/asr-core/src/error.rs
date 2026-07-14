//! Error type shared across the ASR core.

use std::fmt;

/// A transcription failure. Kept deliberately small and `Clone` so it can cross
/// the worker IPC boundary as a string and be reasoned about by the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AsrError {
    /// The recording contained no decodable audio (empty / header-only WAV).
    EmptyAudio,
    /// The WAV bytes were malformed or used an unsupported encoding.
    BadWav(String),
    /// The whisper model file could not be loaded onto the target device.
    ModelLoad(String),
    /// whisper.cpp inference returned an error.
    Inference(String),
    /// Inference produced no usable text.
    NoTranscript,
}

impl fmt::Display for AsrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AsrError::EmptyAudio => write!(f, "recording audio is empty"),
            AsrError::BadWav(m) => write!(f, "invalid WAV audio: {m}"),
            AsrError::ModelLoad(m) => write!(f, "failed to load local speech model: {m}"),
            AsrError::Inference(m) => write!(f, "local speech inference failed: {m}"),
            AsrError::NoTranscript => write!(f, "local speech returned no usable transcript"),
        }
    }
}

impl std::error::Error for AsrError {}
