//! OpenRouter Nitro streaming client for the local emergency polish fallback.

//! Normal signed-in dictation uses the control plane. This module keeps the
//! fallback faithful to that same production provider and request contract.

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, trace, warn};

pub use super::PolishResult;

const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MIN_COMPLETION_TOKENS: usize = 128;
const MAX_COMPLETION_TOKENS: usize = 1024;

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

pub async fn stream_polish(
    client: &Client,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    if api_key.is_empty() {
        return Err("OpenRouter API key not set — configure OPENROUTER_API_KEY".to_string());
    }
    let model = if model.is_empty() {
        said_core::polish::model::OPENROUTER_POLISH_MODEL_GEMMA_4_NITRO
    } else {
        model
    };
    let max_tokens = completion_token_budget(user_message);
    let body = json!({
        "model": model,
        "stream": true,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        "reasoning": { "enabled": false },
        "stop": ["=== BEGIN TRANSCRIPT", "=== END TRANSCRIPT", "<transcript>", "</transcript>"],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message }
        ]
    });

    info!("[openrouter] POST Nitro model={model} max_tokens={max_tokens}");
    let start = Instant::now();
    let resp = client
        .post(OPENROUTER_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("OpenRouter request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        warn!("[openrouter] HTTP {status}: {body_text}");
        return Err(format!(
            "OpenRouter API error {status}: {}",
            said_core::text::truncate_utf8(&body_text, 400)
        ));
    }

    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("OpenRouter stream read error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }
            if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                for choice in chunk.choices {
                    if let Some(token) = choice.delta.content.filter(|token| !token.is_empty()) {
                        if polished.is_empty() {
                            info!(
                                "[openrouter] first token in {}ms",
                                start.elapsed().as_millis()
                            );
                        }
                        polished.push_str(&token);
                        trace!("[openrouter] token: {token:?}");
                        let _ = token_tx.send(token).await;
                    }
                }
            }
        }
    }
    Ok(PolishResult {
        polished,
        polish_ms: start.elapsed().as_millis() as u64,
    })
}

fn completion_token_budget(user_message: &str) -> u32 {
    let source = current_transcript_block(user_message).unwrap_or(user_message);
    let words = source.split_whitespace().count().max(1);
    (words * 2 + 64).clamp(MIN_COMPLETION_TOKENS, MAX_COMPLETION_TOKENS) as u32
}

fn current_transcript_block(user_message: &str) -> Option<&str> {
    let content = user_message
        .split_once("=== BEGIN CURRENT TRANSCRIPT ===")?
        .1;
    let content = content
        .split_once("=== END CURRENT TRANSCRIPT ===")?
        .0
        .trim();
    (!content.is_empty()).then_some(content)
}

#[cfg(test)]
mod tests {
    use super::completion_token_budget;

    #[test]
    fn completion_budget_has_small_floor_for_short_dictation() {
        assert_eq!(completion_token_budget("Thik hai"), 128);
    }

    #[test]
    fn completion_budget_is_capped_at_one_thousand_twenty_four_tokens() {
        assert_eq!(completion_token_budget(&"word ".repeat(1_000)), 1024);
    }
}
