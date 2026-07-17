//! DeepInfra batch speech-to-text transport.

use std::{fmt, sync::Arc, time::Instant};

use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio::time::{Duration, sleep, timeout};

use crate::{CloudSttError, CloudTranscription, classify_http_status};

pub const API_KEY_ENV: &str = "DEEPINFRA_API_KEY";
pub const MODEL: &str = "openai/whisper-large-v3-turbo";
pub const ENDPOINT: &str = "https://api.deepinfra.com/v1/inference/openai/whisper-large-v3-turbo";
pub const LANGUAGE: &str = "hi";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct DeepInfraClient {
    api_key: Arc<str>,
    http: reqwest::Client,
}

impl fmt::Debug for DeepInfraClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeepInfraClient")
            .field("model", &MODEL)
            .finish_non_exhaustive()
    }
}

impl DeepInfraClient {
    pub fn new(api_key: String) -> Result<Self, CloudSttError> {
        if api_key.trim().is_empty() {
            return Err(CloudSttError::MissingApiKey {
                provider: "DeepInfra".into(),
                env_var: API_KEY_ENV.into(),
            });
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| CloudSttError::InvalidResponse {
                detail: format!("couldn't initialize the DeepInfra client: {error}"),
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
            language: response
                .language
                .filter(|language| !language.trim().is_empty()),
            audio_secs: response
                .input_length_ms
                .map(|duration_ms| duration_ms / 1_000.0),
            latency_ms: started.elapsed().as_millis() as u64,
            model: MODEL.to_string(),
        })
    }

    async fn transcribe_once(&self, wav: &[u8]) -> Result<DeepInfraResponse, CloudSttError> {
        timeout(REQUEST_TIMEOUT, async {
            let audio = Part::bytes(wav.to_vec())
                .file_name("dictation.wav")
                .mime_str("audio/wav")
                .map_err(|error| CloudSttError::InvalidResponse {
                    detail: format!("invalid audio upload: {error}"),
                })?;
            let response = self
                .http
                .post(ENDPOINT)
                .bearer_auth(self.api_key.as_ref())
                .multipart(
                    Form::new()
                        .part("audio", audio)
                        .text("language", LANGUAGE)
                        .text("task", "transcribe"),
                )
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
                detail: format!("invalid DeepInfra transcription response: {error}"),
            })
        })
        .await
        .map_err(|_| CloudSttError::Timeout {
            budget_secs: REQUEST_TIMEOUT.as_secs(),
        })?
    }
}

#[derive(Debug, Deserialize)]
struct DeepInfraResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    input_length_ms: Option<f32>,
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
            detail: format!("DeepInfra request failed: {error}"),
        }
    }
}

fn provider_error_detail(body: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return String::from_utf8_lossy(body).trim().to_string();
    };
    value
        .get("error")
        .or_else(|| value.get("detail"))
        .or_else(|| value.get("message"))
        .map(|detail| {
            detail
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| detail.to_string())
        })
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_exposes_the_api_key() {
        let client = DeepInfraClient::new("secret-token".into()).unwrap();
        let debug = format!("{client:?}");
        assert!(debug.contains(MODEL));
        assert!(!debug.contains("secret-token"));
    }

    #[test]
    fn extracts_structured_provider_errors() {
        assert_eq!(
            provider_error_detail(br#"{"error":"bad audio"}"#),
            "bad audio"
        );
    }
}
