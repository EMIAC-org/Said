//! Streaming LLM client for the gateway's `/v1/chat/completions` endpoint.
//!
//! Uses SSE (`stream: true`) and yields tokens via an async channel.
//! Tauri consumes these tokens for live preview and inserts the final result once.

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

const CHAT_URL: &str = "https://gateway.outreachdeal.com/v1/chat/completions";

// ── Response chunk types ──────────────────────────────────────────────────────

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

// ── Polish result ─────────────────────────────────────────────────────────────

pub use super::PolishResult;

/// Stream tokens from the gateway and return when the stream closes.
///
/// Each token is sent on `token_tx` as it arrives.
/// Returns the final concatenated text + latency.
pub async fn stream_polish(
    client: &Client,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    // ── Debug: dump the full prompt being sent to the LLM ─────────────────────
    info!(
        "[llm] ┌──── REQUEST → {} ────────────────────────────────────────────",
        model
    );
    info!("[llm] │ SYSTEM PROMPT ({} chars):", system_prompt.len());
    for line in system_prompt.lines() {
        info!("[llm] │   {line}");
    }
    info!("[llm] │ USER MESSAGE ({} chars):", user_message.len());
    for line in user_message.lines() {
        info!("[llm] │   {line}");
    }
    info!("[llm] └──────────────────────────────────────────────────────────────");

    let body = json!({
        "model": model,
        "stream": true,
        "messages": [
            { "role": "system",  "content": system_prompt },
            { "role": "user",    "content": user_message  },
        ]
    });

    let start = std::time::Instant::now();

    let resp = client
        .post(CHAT_URL)
        .header("X-API-Key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("gateway request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Gateway error {status}: {}",
            said_core::text::truncate_utf8(&body_text, 300)
        ));
    }

    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new(); // SSE line buffer

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream read error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        // SSE lines are separated by \n; double-\n separates events
        while let Some(newline_pos) = buf.find('\n') {
            let line = buf[..newline_pos].trim().to_string();
            buf = buf[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            // Strip "data: " prefix
            let data = match line.strip_prefix("data: ") {
                Some(d) => d.trim(),
                None => continue,
            };

            if data == "[DONE]" {
                break;
            }

            match serde_json::from_str::<StreamChunk>(data) {
                Ok(chunk) => {
                    if let Some(token) = chunk
                        .choices
                        .into_iter()
                        .next()
                        .and_then(|c| c.delta.content)
                    {
                        if !token.is_empty() {
                            polished.push_str(&token);
                            trace!("[llm] token: {:?}", token);
                            // Send token; ignore error (receiver may have dropped)
                            let _ = token_tx.send(token).await;
                        }
                    }
                }
                Err(e) => {
                    warn!("[llm] failed to parse SSE chunk {data:?}: {e}");
                }
            }
        }
    }

    let polish_ms = start.elapsed().as_millis() as u64;
    debug!("[llm] finished in {polish_ms}ms, {} chars", polished.len());

    Ok(PolishResult {
        polished,
        polish_ms,
    })
}

/// Non-streaming variant for test/debug use.
pub async fn polish_blocking(
    client: &Client,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system",  "content": system_prompt },
            { "role": "user",    "content": user_message  },
        ]
    });

    let resp = client
        .post(CHAT_URL)
        .header("X-API-Key", api_key)
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("gateway request failed: {e}"))?;

    #[derive(Deserialize)]
    struct ChatResp {
        choices: Vec<ChatChoice>,
    }
    #[derive(Deserialize)]
    struct ChatChoice {
        message: ChatMsg,
    }
    #[derive(Deserialize)]
    struct ChatMsg {
        content: String,
    }

    let data: ChatResp = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse response: {e}"))?;

    data.choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "no completion in response".into())
}
