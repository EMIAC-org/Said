pub mod deepinfra;
pub mod devanagari_recovery;
pub mod gateway;
pub mod gemini_direct;
pub mod groq;
pub mod openai_codex;
pub mod polish_dispatch;
pub mod prompt;
pub mod script;
pub mod stream_safety;

use serde::{Deserialize, Serialize};

/// Shared result type returned by all LLM streaming clients.
pub struct PolishResult {
    pub polished: String,
    pub polish_ms: u64,
}

const STRUCTURED_ERROR_PREFIX: &str = "AIRNOTE_LLM_ERROR_JSON:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmErrorDetails {
    pub message: String,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub retryable: Option<bool>,
    #[serde(default)]
    pub diagnostic: Option<String>,
}

pub fn encode_llm_error(details: &LlmErrorDetails) -> String {
    match serde_json::to_string(details) {
        Ok(payload) => format!("{STRUCTURED_ERROR_PREFIX}{payload}"),
        Err(_) => details.message.clone(),
    }
}

pub fn decode_llm_error(err: &str) -> Option<LlmErrorDetails> {
    let payload = err.strip_prefix(STRUCTURED_ERROR_PREFIX)?;
    serde_json::from_str(payload).ok()
}

pub fn llm_error_message(err: &str) -> String {
    decode_llm_error(err)
        .map(|details| details.message)
        .unwrap_or_else(|| err.to_string())
}
