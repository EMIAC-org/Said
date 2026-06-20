//! Groq LLM client.
//!
//! Groq exposes an OpenAI-compatible Chat Completions endpoint with much lower
//! latency (~200–400 ms TTFT) because requests run on Groq's LPU hardware.
//!
//! API reference: https://console.groq.com/docs/openai
//! Endpoint: https://api.groq.com/openai/v1/chat/completions
//! Auth: Authorization: Bearer {groq_api_key}
//!
//! SSE format is identical to OpenAI: `data: {choices:[{delta:{content:"…"}}]}`
//!
//! Recommended models (fast, multilingual):
//!   - llama-3.3-70b-versatile  (best quality, ~300ms TTFT)
//!   - llama-3.1-8b-instant     (fastest, ~150ms TTFT)
//!   - gemma2-9b-it             (Google Gemma, good Hinglish)
//!   - mixtral-8x7b-32768       (32k context)

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, trace, warn};

pub use super::PolishResult;

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";

/// Default model — Llama 3.1 8B on Groq. Fast, disciplined, no hallucination.
/// Hindi→Roman is a simple structured task; 8B follows system prompts tightly.
/// Groq gives 4K RPM + sub-200ms latency — production-grade for voice polish.
pub const GROQ_MODEL_DEFAULT: &str = "llama-3.1-8b-instant";
/// Fast model — same as default on Groq (8B is already the fastest).
pub const GROQ_MODEL_FAST: &str = "llama-3.1-8b-instant";
/// Smart model — Llama 4 Scout 17B on Groq. Better quality for normal dictation.
pub const GROQ_MODEL_SMART: &str = "meta-llama/llama-4-scout-17b-16e-instruct";
/// High quality fallback for Option+N selected-text transforms.
pub const GROQ_MODEL_70B: &str = "llama-3.3-70b-versatile";

// ── SSE types (identical to OpenAI format) ────────────────────────────────────

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}
#[derive(Deserialize)]
struct StreamChoice {
    delta: Delta,
}
#[derive(Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

/// Non-streaming Groq call — for Pass 1 (substitutions/formatting).
/// Returns the full response text or an error.
pub async fn call_json(
    client: &Client,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let model = if model.is_empty() {
        GROQ_MODEL_DEFAULT
    } else {
        model
    };
    let body = serde_json::json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": 2048,
        "stream": false,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message },
        ]
    });

    let start = Instant::now();
    info!("[groq-json] POST {GROQ_ENDPOINT} model={model}");

    let resp = client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("groq request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        warn!(
            "[groq-json] HTTP {status}: {}",
            said_core::text::truncate_utf8(&body_text, 300)
        );
        return Err(format!("Groq API error {status}"));
    }

    let resp_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("groq json parse: {e}"))?;

    let ms = start.elapsed().as_millis();
    let content = resp_json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    info!("[groq-json] done in {ms}ms, {} chars", content.len());
    Ok(content)
}

/// Stream a polish operation through Groq's Chat Completions API.
///
/// `model` defaults to `GROQ_MODEL_DEFAULT` if empty.
/// Each token is sent on `token_tx` as it arrives.
/// Returns the final concatenated text + latency.
pub async fn stream_polish(
    client: &Client,
    groq_api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    if groq_api_key.is_empty() {
        return Err("Groq API key not set — add it in Settings → API Keys".to_string());
    }

    let model = if model.is_empty() {
        GROQ_MODEL_DEFAULT
    } else {
        model
    };
    let start = Instant::now();

    // Hard ceiling on generated tokens — polish output should never exceed
    // ~2x the input length. Caps runaway "let me also explain..." drift that
    // happens when the model slips out of cleaner-mode into responder-mode.
    // ~4 chars/token; user_message includes the transcript + reminder + fence.
    let estimated_input_tokens = user_message.len() / 4;
    let max_tokens = (estimated_input_tokens * 2 + 256).min(8192) as u32;

    // Stop sequences serve as a server-side defense against fence echo:
    // if the model ever starts to repeat the transcript fence (a known
    // leak shape), generation stops before tokens reach the client filter.
    // Capped at 4 (Groq API limit).
    let body = json!({
        "model":       model,
        "stream":      true,
        "temperature": 0.0,
        "top_p":       0.9,
        "max_tokens":  max_tokens,
        "stop": [
            "=== BEGIN TRANSCRIPT",
            "=== END TRANSCRIPT",
            "<transcript>",
            "</transcript>",
        ],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });

    info!("[groq] POST {GROQ_ENDPOINT} model={model}");

    let resp = client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {groq_api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Groq request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        warn!("[groq] HTTP {status}: {body_text}");
        return Err(format!(
            "Groq API error {status}: {}",
            said_core::text::truncate_utf8(&body_text, 400)
        ));
    }

    // Stream SSE response
    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new();
    let mut ttft_ms: Option<u64> = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Groq stream read error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();

            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }

            if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                for choice in chunk.choices {
                    if let Some(token) = choice.delta.content {
                        if !token.is_empty() {
                            if ttft_ms.is_none() {
                                let ms = start.elapsed().as_millis() as u64;
                                ttft_ms = Some(ms);
                                info!("[groq] first token in {ms}ms");
                            }
                            polished.push_str(&token);
                            trace!("[groq] token: {token:?}");
                            let _ = token_tx.send(token).await;
                        }
                    }
                }
            }
        }
    }

    let polish_ms = start.elapsed().as_millis() as u64;
    info!("[groq] done in {polish_ms}ms, {} chars", polished.len());

    Ok(PolishResult {
        polished,
        polish_ms,
    })
}
