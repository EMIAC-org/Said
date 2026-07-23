//! Hosted batch speech-to-text transport.
//!
//! This crate knows nothing about platform selection, recorder lifecycle, or
//! build-time API-key ownership.

mod deepinfra;
mod openai;

pub use deepinfra::{API_KEY_ENV, DeepInfraClient, ENDPOINT, LANGUAGE, MODEL};
pub use openai::{
    API_KEY_ENV as OPENAI_API_KEY_ENV, ENDPOINT as OPENAI_ENDPOINT, MODEL as OPENAI_MODEL,
    OpenAiClient,
};

/// A completed cloud transcription.
#[derive(Debug, Clone)]
pub struct CloudTranscription {
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

/// Why a cloud transcription failed.
///
/// `Display` renders each variant as a complete, user-facing sentence — the
/// desktop shows these verbatim in the dictation error UI, so they must say
/// what happened *and* what to do about it.
#[derive(Debug, Clone)]
pub enum CloudSttError {
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

impl CloudSttError {
    /// Transient failures are worth one immediate retry (a blip, a throttle, a
    /// bad gateway); the rest are deterministic and retrying just adds latency.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            CloudSttError::Offline
                | CloudSttError::Timeout { .. }
                | CloudSttError::RateLimited
                | CloudSttError::Service { .. }
        )
    }
}

impl std::fmt::Display for CloudSttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloudSttError::MissingApiKey { provider, env_var } => write!(
                f,
                "Speech service unavailable — no {provider} key in this build (bake {env_var} at build time, or set it as an env var)."
            ),
            CloudSttError::Offline => write!(
                f,
                "Couldn't reach the speech service — check your internet connection and try again."
            ),
            CloudSttError::Timeout { budget_secs } => write!(
                f,
                "The speech service didn't respond within {budget_secs}s — check your connection and try again."
            ),
            CloudSttError::Auth { status } => write!(
                f,
                "The speech service rejected this build's key (HTTP {status}) — rebuild with a valid key."
            ),
            CloudSttError::RateLimited => {
                write!(f, "The speech service is busy — try again in a moment.")
            }
            CloudSttError::Service { status } => write!(
                f,
                "The speech service hit an internal error (HTTP {status}) — try again."
            ),
            CloudSttError::Rejected { status, detail } => write!(
                f,
                "The speech service rejected the audio (HTTP {status}): {detail}"
            ),
            CloudSttError::InvalidResponse { detail } => write!(
                f,
                "The speech service returned an unreadable response: {detail}"
            ),
        }
    }
}

impl std::error::Error for CloudSttError {}

pub(crate) fn classify_http_status(status: u16, detail: &str) -> CloudSttError {
    match status {
        401 | 403 => CloudSttError::Auth { status },
        429 => CloudSttError::RateLimited,
        500..=599 => CloudSttError::Service { status },
        _ => CloudSttError::Rejected {
            status,
            detail: detail.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_render_as_actionable_sentences() {
        let cases: Vec<CloudSttError> = vec![
            CloudSttError::MissingApiKey {
                provider: "DeepInfra".into(),
                env_var: "DEEPINFRA_API_KEY".into(),
            },
            CloudSttError::Offline,
            CloudSttError::Timeout { budget_secs: 75 },
            CloudSttError::Auth { status: 401 },
            CloudSttError::RateLimited,
            CloudSttError::Service { status: 502 },
            CloudSttError::Rejected {
                status: 422,
                detail: "audio too short".into(),
            },
            CloudSttError::InvalidResponse {
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
        assert!(CloudSttError::Offline.is_transient());
        assert!(CloudSttError::Timeout { budget_secs: 1 }.is_transient());
        assert!(CloudSttError::RateLimited.is_transient());
        assert!(CloudSttError::Service { status: 500 }.is_transient());
        assert!(!CloudSttError::Auth { status: 401 }.is_transient());
        assert!(
            !CloudSttError::Rejected {
                status: 422,
                detail: String::new()
            }
            .is_transient()
        );
        assert!(
            !CloudSttError::MissingApiKey {
                provider: String::new(),
                env_var: String::new()
            }
            .is_transient()
        );
    }
}
