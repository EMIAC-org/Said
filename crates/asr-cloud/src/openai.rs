//! OpenAI completed-recording speech-to-text transport.
//!
//! This uses `POST /v1/audio/transcriptions`, not OpenAI's asynchronous Batch
//! API. AirNote uploads one WAV after push-to-talk ends and waits for its
//! transcription before the existing polish pipeline runs.

use std::{fmt, sync::Arc, time::Instant};

use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio::time::{Duration, sleep, timeout};

use crate::{CloudSttError, CloudTranscription, classify_http_status};

pub const API_KEY_ENV: &str = "OPENAI_API_KEY";
pub const MODEL: &str = "gpt-4o-mini-transcribe";
pub const ENDPOINT: &str = "https://api.openai.com/v1/audio/transcriptions";

// AirNote's current cloud-STT product scope is Hindi/Hinglish. Sending an
// explicit hint prevents auto-detection from choosing Urdu for Hindi speech.
const LANGUAGE: &str = "hi";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct OpenAiClient {
    api_key: Arc<str>,
    http: reqwest::Client,
}

impl fmt::Debug for OpenAiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiClient")
            .field("model", &MODEL)
            .finish_non_exhaustive()
    }
}

impl OpenAiClient {
    pub fn new(api_key: String) -> Result<Self, CloudSttError> {
        if api_key.trim().is_empty() {
            return Err(CloudSttError::MissingApiKey {
                provider: "OpenAI".into(),
                env_var: API_KEY_ENV.into(),
            });
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| CloudSttError::InvalidResponse {
                detail: format!("couldn't initialize the OpenAI client: {error}"),
            })?;
        Ok(Self {
            api_key: Arc::from(api_key),
            http,
        })
    }

    pub fn model(&self) -> &'static str {
        MODEL
    }

    pub async fn transcribe_wav(&self, wav: &[u8]) -> Result<CloudTranscription, CloudSttError> {
        if wav.is_empty() {
            return Err(CloudSttError::Rejected {
                status: 422,
                detail: "the recording contained no audio".into(),
            });
        }

        let started = Instant::now();
        let mut result = self.transcribe_once(wav).await;
        if result.as_ref().is_err_and(CloudSttError::is_transient) {
            sleep(RETRY_DELAY).await;
            result = self.transcribe_once(wav).await;
        }
        let response = result?;

        Ok(CloudTranscription {
            text: response.text.trim().to_string(),
            // The documented JSON response is text + usage. AirNote sends a
            // deliberate Hindi hint rather than inferring a language locally.
            language: Some(LANGUAGE.to_string()),
            audio_secs: None,
            latency_ms: started.elapsed().as_millis() as u64,
            model: MODEL.to_string(),
        })
    }

    async fn transcribe_once(&self, wav: &[u8]) -> Result<OpenAiResponse, CloudSttError> {
        timeout(REQUEST_TIMEOUT, async {
            let audio = Part::bytes(wav.to_vec())
                .file_name("dictation.wav")
                .mime_str("audio/wav")
                .map_err(|error| CloudSttError::InvalidResponse {
                    detail: format!("invalid audio upload: {error}"),
                })?;
            let form = Form::new()
                .part("file", audio)
                .text("model", MODEL)
                .text("response_format", "json")
                .text("language", LANGUAGE);
            let response = self
                .http
                .post(ENDPOINT)
                .bearer_auth(self.api_key.as_ref())
                .multipart(form)
                .send()
                .await
                .map_err(request_error)?;
            let status = response.status();
            let body = response.bytes().await.map_err(request_error)?;
            if !status.is_success() {
                return Err(classify_http_status(
                    status.as_u16(),
                    &provider_error_detail(&body),
                ));
            }
            serde_json::from_slice(&body).map_err(|error| CloudSttError::InvalidResponse {
                detail: format!("invalid OpenAI transcription response: {error}"),
            })
        })
        .await
        .map_err(|_| CloudSttError::Timeout {
            budget_secs: REQUEST_TIMEOUT.as_secs(),
        })?
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    text: String,
}

fn request_error(error: reqwest::Error) -> CloudSttError {
    if error.is_timeout() {
        CloudSttError::Timeout {
            budget_secs: REQUEST_TIMEOUT.as_secs(),
        }
    } else if error.is_connect() {
        CloudSttError::Offline
    } else {
        CloudSttError::InvalidResponse {
            detail: format!("OpenAI request failed: {error}"),
        }
    }
}

fn provider_error_detail(body: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return String::from_utf8_lossy(body).trim().to_string();
    };
    let detail = value
        .get("error")
        .or_else(|| value.get("detail"))
        .or_else(|| value.get("message"))
        .unwrap_or(&value);
    detail
        .as_str()
        .or_else(|| detail.get("message").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_exposes_the_api_key() {
        let client = OpenAiClient::new("secret-token".into()).unwrap();
        let debug = format!("{client:?}");
        assert!(debug.contains(MODEL));
        assert!(!debug.contains("secret-token"));
    }

    #[test]
    fn uses_hindi_for_every_openai_transcription() {
        assert_eq!(LANGUAGE, "hi");
    }

    #[test]
    fn extracts_structured_provider_errors() {
        assert_eq!(
            provider_error_detail(br#"{"error":{"message":"bad audio"}}"#),
            "bad audio"
        );
    }
}
