//! OpenAI-compatible `/audio/transcriptions` transport.
//!
//! Protocol (per DeepInfra's API reference, which mirrors OpenAI's):
//!   POST {base_url}/audio/transcriptions
//!   Authorization: Bearer {api_key}
//!   multipart/form-data:
//!     file            (binary, required)  — the audio clip
//!     model           (string, required)  — provider model id
//!     language        (string, optional)  — ISO-639-1 hint; omit = auto-detect
//!     response_format (string, optional)  — we ask for `verbose_json` to get
//!                                           the detected language + duration
//! Docs: <https://docs.deepinfra.com/api-reference/audio/openai-audio-transcriptions>

use std::time::Instant;

use crate::{HostedSttConfig, HostedSttError, HostedTranscription};

/// The `verbose_json` response body. Only `text` is guaranteed across
/// providers; everything else is best-effort enrichment.
#[derive(Debug, serde::Deserialize)]
struct VerboseJsonBody {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    duration: Option<f32>,
}

/// A reusable client for one hosted transcription provider.
///
/// Construct once and share: the inner `reqwest::Client` pools connections,
/// which saves a TLS handshake (~100–300ms) on every dictation after the first.
#[derive(Debug, Clone)]
pub struct HostedSttClient {
    http: reqwest::Client,
    cfg: HostedSttConfig,
}

impl HostedSttClient {
    pub fn new(cfg: HostedSttConfig) -> Result<Self, HostedSttError> {
        if cfg.api_key.trim().is_empty() {
            return Err(HostedSttError::MissingApiKey {
                provider: cfg.base_url.clone(),
                env_var: "the provider API key".into(),
            });
        }
        let http = reqwest::Client::builder()
            .connect_timeout(cfg.connect_timeout)
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| HostedSttError::InvalidResponse {
                detail: format!("HTTP client construction failed: {e}"),
            })?;
        Ok(Self { http, cfg })
    }

    /// The model this client transcribes with.
    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    /// Transcribe one WAV clip.
    ///
    /// `language_hint` pins the model to an ISO-639-1 language; `None` lets it
    /// run its own language identification on the audio.
    ///
    /// Transient failures (offline blip, timeout, throttle, 5xx) get exactly one
    /// immediate retry — enough to absorb a hiccup without doubling worst-case
    /// latency on a genuinely dead connection.
    pub async fn transcribe_wav(
        &self,
        wav: Vec<u8>,
        language_hint: Option<&str>,
    ) -> Result<HostedTranscription, HostedSttError> {
        match self.attempt(wav.clone(), language_hint).await {
            Err(err) if err.is_transient() => {
                tracing::warn!("[asr-cloud] transient failure, retrying once: {err}");
                self.attempt(wav, language_hint).await
            }
            outcome => outcome,
        }
    }

    async fn attempt(
        &self,
        wav: Vec<u8>,
        language_hint: Option<&str>,
    ) -> Result<HostedTranscription, HostedSttError> {
        let url = format!("{}/audio/transcriptions", self.cfg.base_url);
        let file = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| HostedSttError::InvalidResponse {
                detail: format!("multipart construction failed: {e}"),
            })?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("model", self.cfg.model.clone())
            .text("response_format", "verbose_json");
        if let Some(lang) = language_hint {
            form = form.text("language", lang.to_string());
        }

        let started = Instant::now();
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.cfg.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| self.classify_transport_error(&e))?;

        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(classify_status(status, &body));
        }

        let parsed: VerboseJsonBody =
            serde_json::from_str(&body).map_err(|e| HostedSttError::InvalidResponse {
                detail: format!("{e} (body: {})", truncate(&body, 200)),
            })?;

        Ok(HostedTranscription {
            text: parsed.text.trim().to_string(),
            language: parsed.language,
            audio_secs: parsed.duration,
            latency_ms: started.elapsed().as_millis() as u64,
            model: self.cfg.model.clone(),
        })
    }

    fn classify_transport_error(&self, e: &reqwest::Error) -> HostedSttError {
        // Connect first: reqwest flags a connect-phase timeout as BOTH
        // is_connect() and is_timeout(), and "check your internet" is the
        // actionable message there — "didn't respond within {budget}s" is only
        // true when the connection was established and the response stalled.
        if e.is_connect() {
            HostedSttError::Offline
        } else if e.is_timeout() {
            HostedSttError::Timeout {
                budget_secs: self.cfg.request_timeout.as_secs(),
            }
        } else {
            HostedSttError::InvalidResponse {
                detail: e.to_string(),
            }
        }
    }
}

/// Map a non-2xx status to the error a user (or build engineer) can act on.
fn classify_status(status: u16, body: &str) -> HostedSttError {
    match status {
        401 | 403 => HostedSttError::Auth { status },
        429 => HostedSttError::RateLimited,
        500..=599 => HostedSttError::Service { status },
        _ => HostedSttError::Rejected {
            status,
            detail: truncate(body, 300).to_string(),
        },
    }
}

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_json_parses_with_and_without_enrichment() {
        let full: VerboseJsonBody = serde_json::from_str(
            r#"{"text":" hello world ","language":"en","duration":3.2,"segments":[{"id":0}]}"#,
        )
        .unwrap();
        assert_eq!(full.text.trim(), "hello world");
        assert_eq!(full.language.as_deref(), Some("en"));
        assert_eq!(full.duration, Some(3.2));

        // A provider that returns the plain-json shape must still work.
        let minimal: VerboseJsonBody = serde_json::from_str(r#"{"text":"नमस्ते"}"#).unwrap();
        assert_eq!(minimal.text, "नमस्ते");
        assert!(minimal.language.is_none());
        assert!(minimal.duration.is_none());
    }

    #[test]
    fn statuses_map_to_the_right_error_class() {
        assert!(matches!(
            classify_status(401, ""),
            HostedSttError::Auth { status: 401 }
        ));
        assert!(matches!(
            classify_status(429, ""),
            HostedSttError::RateLimited
        ));
        assert!(matches!(
            classify_status(502, ""),
            HostedSttError::Service { status: 502 }
        ));
        assert!(matches!(
            classify_status(422, "bad audio"),
            HostedSttError::Rejected { status: 422, .. }
        ));
    }

    #[test]
    fn empty_api_key_is_rejected_at_construction() {
        let cfg = crate::deepinfra::config("   ".to_string());
        assert!(matches!(
            HostedSttClient::new(cfg),
            Err(HostedSttError::MissingApiKey { .. })
        ));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("नमस्ते दुनिया", 6), "नमस्ते");
        assert_eq!(truncate("short", 300), "short");
    }
}
