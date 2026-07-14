//! Hosted (cloud) speech-to-text.
//!
//! The cloud counterpart of `asr-core`: where asr-core runs whisper on-device,
//! this crate transcribes by calling a hosted OpenAI-compatible
//! `POST {base_url}/audio/transcriptions` endpoint. DeepInfra, OpenAI, Groq and
//! Fireworks all implement that protocol, so a provider is a [`HostedSttConfig`]
//! preset (base URL + model + key) — never a new client implementation.
//!
//! Providers:
//! * [`deepinfra`] — `openai/whisper-large-v3-turbo` on DeepInfra
//!   (<https://docs.deepinfra.com/api-reference/audio/openai-audio-transcriptions>)
//!
//! This crate is transport-only. It knows nothing about platforms, dictation
//! flows, or where the API key comes from — callers resolve the key and decide
//! when hosted transcription is the right provider.

use std::time::Duration;

mod client;
pub mod deepinfra;

pub use client::HostedSttClient;

/// Everything needed to reach one hosted transcription provider.
#[derive(Debug, Clone)]
pub struct HostedSttConfig {
    /// API root that exposes `/audio/transcriptions`, e.g. `https://api.deepinfra.com/v1`.
    pub base_url: String,
    /// Provider model identifier, e.g. `openai/whisper-large-v3-turbo`.
    pub model: String,
    /// Bearer token for the `Authorization` header.
    pub api_key: String,
    /// TCP connect budget. Kept short so an offline machine fails fast with an
    /// actionable message instead of hanging the dictation pipeline.
    pub connect_timeout: Duration,
    /// Whole-request budget (upload + inference + download). Sized for
    /// multi-minute dictations on slow uplinks; typical clips finish in 1–4s.
    pub request_timeout: Duration,
}

/// A completed hosted transcription.
#[derive(Debug, Clone)]
pub struct HostedTranscription {
    /// Transcript text as returned by the model (whitespace-trimmed).
    pub text: String,
    /// ISO-639-1 language the model detected in the audio, when reported.
    pub language: Option<String>,
    /// Audio duration in seconds, when reported.
    pub audio_secs: Option<f32>,
    /// Wall-clock time for the API call, including the upload.
    pub latency_ms: u64,
    /// Model that produced the transcript (from the request config).
    pub model: String,
}

/// Why a hosted transcription failed.
///
/// `Display` renders each variant as a complete, user-facing sentence — the
/// desktop shows these verbatim in the dictation error UI, so they must say
/// what happened *and* what to do about it.
#[derive(Debug, Clone)]
pub enum HostedSttError {
    /// The config carried no API key. End-users cannot fix this (keys are
    /// baked at build time) — the message targets whoever ships the build.
    MissingApiKey { provider: String, env_var: String },
    /// TCP/TLS connection never established — machine offline or DNS failure.
    Offline,
    /// The request exceeded its time budget.
    Timeout { budget_secs: u64 },
    /// 401/403 — the baked key was rejected.
    Auth { status: u16 },
    /// 429 — provider throttled us.
    RateLimited,
    /// 5xx — provider-side failure.
    Service { status: u16 },
    /// Any other non-success status (e.g. 422 audio validation).
    Rejected { status: u16, detail: String },
    /// 200 OK but the body wasn't the documented JSON shape.
    InvalidResponse { detail: String },
}

impl HostedSttError {
    /// Transient failures are worth one immediate retry (a blip, a throttle, a
    /// bad gateway); the rest are deterministic and retrying just adds latency.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            HostedSttError::Offline
                | HostedSttError::Timeout { .. }
                | HostedSttError::RateLimited
                | HostedSttError::Service { .. }
        )
    }
}

impl std::fmt::Display for HostedSttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostedSttError::MissingApiKey { provider, env_var } => write!(
                f,
                "Speech service unavailable — no {provider} key in this build (bake {env_var} at build time, or set it as an env var)."
            ),
            HostedSttError::Offline => write!(
                f,
                "Couldn't reach the speech service — check your internet connection and try again."
            ),
            HostedSttError::Timeout { budget_secs } => write!(
                f,
                "The speech service didn't respond within {budget_secs}s — check your connection and try again."
            ),
            HostedSttError::Auth { status } => write!(
                f,
                "The speech service rejected this build's key (HTTP {status}) — rebuild with a valid key."
            ),
            HostedSttError::RateLimited => {
                write!(f, "The speech service is busy — try again in a moment.")
            }
            HostedSttError::Service { status } => write!(
                f,
                "The speech service hit an internal error (HTTP {status}) — try again."
            ),
            HostedSttError::Rejected { status, detail } => write!(
                f,
                "The speech service rejected the audio (HTTP {status}): {detail}"
            ),
            HostedSttError::InvalidResponse { detail } => write!(
                f,
                "The speech service returned an unreadable response: {detail}"
            ),
        }
    }
}

impl std::error::Error for HostedSttError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_render_as_actionable_sentences() {
        let cases: Vec<HostedSttError> = vec![
            HostedSttError::MissingApiKey {
                provider: "DeepInfra".into(),
                env_var: "DEEPINFRA_API_KEY".into(),
            },
            HostedSttError::Offline,
            HostedSttError::Timeout { budget_secs: 75 },
            HostedSttError::Auth { status: 401 },
            HostedSttError::RateLimited,
            HostedSttError::Service { status: 502 },
            HostedSttError::Rejected {
                status: 422,
                detail: "audio too short".into(),
            },
            HostedSttError::InvalidResponse {
                detail: "expected JSON".into(),
            },
        ];
        for err in cases {
            let msg = err.to_string();
            // Every message must be a full sentence a non-engineer can act on.
            assert!(msg.len() > 20, "too terse: {msg}");
            assert!(
                msg.ends_with('.') || msg.contains(':'),
                "not a sentence: {msg}"
            );
        }
    }

    #[test]
    fn only_blips_and_throttles_are_transient() {
        assert!(HostedSttError::Offline.is_transient());
        assert!(HostedSttError::Timeout { budget_secs: 1 }.is_transient());
        assert!(HostedSttError::RateLimited.is_transient());
        assert!(HostedSttError::Service { status: 500 }.is_transient());
        assert!(!HostedSttError::Auth { status: 401 }.is_transient());
        assert!(
            !HostedSttError::Rejected {
                status: 422,
                detail: String::new()
            }
            .is_transient()
        );
        assert!(
            !HostedSttError::MissingApiKey {
                provider: String::new(),
                env_var: String::new()
            }
            .is_transient()
        );
    }
}
