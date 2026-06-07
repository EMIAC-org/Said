//! LLM polish client.
//!
//! Talks to an OpenAI-compatible Chat Completions endpoint (Groq by default:
//! `https://api.groq.com/openai/v1/chat/completions`, `Authorization: Bearer`).
//! The SSE format is the OpenAI shape: `data: {choices:[{delta:{content}}]}`.
//! Streaming yields tokens over an mpsc channel for low-latency delta events;
//! `polish_once` is the non-streaming variant used by the batch path.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

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

pub struct PolishOutcome {
    pub polished: String,
    pub polish_ms: u64,
}

/// Stream a polish through the LLM, sending each token on `token_tx` as it
/// arrives. Returns the full concatenated text + latency.
pub async fn stream_polish(
    http: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishOutcome, String> {
    if api_key.trim().is_empty() {
        return Err("LLM API key not configured".into());
    }

    // Cap generated tokens to ~2x input — polish output should never balloon.
    let estimated_input_tokens = user_message.len() / 4;
    let max_tokens = (estimated_input_tokens * 2 + 256).min(8192) as u32;

    let body = json!({
        "model": model,
        "stream": true,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        "stop": ["=== BEGIN TRANSCRIPT", "=== END TRANSCRIPT"],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });

    let start = Instant::now();
    let resp = http
        .post(base_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "LLM error {status}: {}",
            body_text.chars().take(300).collect::<String>()
        ));
    }

    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("LLM stream read error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();

            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                for choice in chunk.choices {
                    if let Some(token) = choice.delta.content {
                        if !token.is_empty() {
                            polished.push_str(&token);
                            let _ = token_tx.send(token).await;
                        }
                    }
                }
            }
        }
    }

    Ok(PolishOutcome {
        polished,
        polish_ms: start.elapsed().as_millis() as u64,
    })
}

/// Non-streaming polish — used by the batch dictation path.
pub async fn polish_once(
    http: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    if api_key.trim().is_empty() {
        return Err("LLM API key not configured".into());
    }
    let body = json!({
        "model": model,
        "stream": false,
        "temperature": 0.0,
        "max_tokens": 2048,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });

    let resp = http
        .post(base_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "LLM error {status}: {}",
            body_text.chars().take(300).collect::<String>()
        ));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("LLM json parse: {e}"))?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(content)
}
