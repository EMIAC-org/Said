//! Direct Gemini LLM client via the OpenAI-compatible endpoint.
//!
//! Uses `https://generativelanguage.googleapis.com/v1beta/openai/chat/completions`
//! with `stream: true` and standard OpenAI SSE chunk format.
//!
//! This is intentionally structured to mirror `gateway.rs` so both can be
//! compared side-by-side in the UI.

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{info, trace, warn};

pub const GEMINI_DIRECT_MODEL: &str = "gemini-3.1-flash-lite-preview";

const GEMINI_OAI_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions";

// ── Response chunk types (OpenAI-compat) ─────────────────────────────────────

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

/// Stream tokens directly from Gemini via the OpenAI-compatible endpoint.
///
/// Drop-in replacement for `gateway::stream_polish` — same signature.
pub async fn stream_polish(
    client: &Client,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    info!(
        "[gemini_direct] ┌──── REQUEST → {} ────────────────────────────",
        model
    );
    info!("[gemini_direct] │ SYSTEM ({} chars)", system_prompt.len());
    info!("[gemini_direct] │ USER   ({} chars)", user_message.len());
    info!("[gemini_direct] └────────────────────────────────────────────");

    let body = json!({
        "model":  model,
        "stream": true,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });

    let start = std::time::Instant::now();

    let resp = client
        .post(GEMINI_OAI_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("gemini_direct request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Gemini error {status}: {}",
            said_core::text::truncate_utf8(&body_text, 400)
        ));
    }

    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream read error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        while let Some(newline_pos) = buf.find('\n') {
            let line = buf[..newline_pos].trim().to_string();
            buf = buf[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

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
                            trace!("[gemini_direct] token: {:?}", token);
                            let _ = token_tx.send(token).await;
                        }
                    }
                }
                Err(e) => {
                    warn!("[gemini_direct] failed to parse SSE chunk {data:?}: {e}");
                }
            }
        }
    }

    let polish_ms = start.elapsed().as_millis() as u64;
    info!(
        "[gemini_direct] finished in {polish_ms}ms, {} chars",
        polished.len()
    );

    Ok(PolishResult {
        polished,
        polish_ms,
    })
}
