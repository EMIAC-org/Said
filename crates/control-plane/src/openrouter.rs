//! OpenRouter Chat Completions client for production server-runtime polish.

//! The production model is the Gemma 4 Nitro route. Nitro selects a healthy,
//! high-throughput upstream for each request; AirNote still owns one stable
//! provider contract: OpenRouter's OpenAI-compatible chat-completions API.

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::openai_compat_polish::{gateway_err, parse_chat_completion, read_polish_stream};

const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MIN_COMPLETION_TOKENS: usize = 128;
const MAX_COMPLETION_TOKENS: usize = 1024;

pub async fn call_openrouter(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let max_tokens = completion_token_budget(user_message);
    let stream_tokens = token_tx.is_some();
    let body = openrouter_polish_body(
        model,
        system_prompt,
        user_message,
        max_tokens,
        stream_tokens,
    );

    tracing::info!("[runtime] POST OpenRouter Nitro model={model} max_tokens={max_tokens}");
    let request_started = Instant::now();
    let resp = crate::HTTP_CLIENT
        .post(OPENROUTER_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| gateway_err(&format!("server runtime OpenRouter request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] OpenRouter HTTP {status}: {}",
            said_core::text::truncate_utf8(&preview, 300),
        );
        return Err(gateway_err(&format!("OpenRouter returned {status}")));
    }

    if let Some(token_tx) = token_tx {
        return read_polish_stream(resp, token_tx, request_started, "openrouter", model).await;
    }

    let value: Value = resp.json().await.map_err(|e| {
        gateway_err(&format!(
            "server runtime OpenRouter response parse failed: {e}"
        ))
    })?;
    parse_chat_completion(&value)
        .ok_or_else(|| gateway_err("server runtime OpenRouter returned empty output"))
}

fn openrouter_polish_body(
    model: &str,
    system_prompt: &str,
    user_message: &str,
    max_tokens: u32,
    stream: bool,
) -> Value {
    json!({
        "model": model,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        // Polish is a short rewriting task. Reasoning competes with visible
        // output for the same completion budget, so it stays off.
        "reasoning": { "enabled": false },
        "stream": stream,
        "stop": [
            "=== BEGIN TRANSCRIPT",
            "=== END TRANSCRIPT",
            "<transcript>",
            "</transcript>"
        ],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message }
        ]
    })
}

fn completion_token_budget(user_message: &str) -> u32 {
    let source = current_transcript_block(user_message).unwrap_or(user_message);
    let word_count = source.split_whitespace().count().max(1);
    (word_count * 2 + 64).clamp(MIN_COMPLETION_TOKENS, MAX_COMPLETION_TOKENS) as u32
}

fn current_transcript_block(user_message: &str) -> Option<&str> {
    let after_start = user_message
        .split_once("=== BEGIN CURRENT TRANSCRIPT ===")?
        .1;
    let transcript = after_start
        .split_once("=== END CURRENT TRANSCRIPT ===")?
        .0
        .trim();
    (!transcript.is_empty()).then_some(transcript)
}

#[cfg(test)]
mod tests {
    use super::{completion_token_budget, current_transcript_block, openrouter_polish_body};
    use said_core::polish::model::OPENROUTER_POLISH_MODEL_GEMMA_4_NITRO;

    #[test]
    fn extracts_current_transcript_for_budgeting() {
        let message = "before\n=== BEGIN CURRENT TRANSCRIPT ===\none two three\n=== END CURRENT TRANSCRIPT ===";
        assert_eq!(current_transcript_block(message), Some("one two three"));
    }

    #[test]
    fn completion_budget_has_small_floor_for_short_dictation() {
        assert_eq!(completion_token_budget("Thik hai"), 128);
    }

    #[test]
    fn body_pins_openrouter_nitro_gemma_with_reasoning_disabled() {
        let body = openrouter_polish_body(
            OPENROUTER_POLISH_MODEL_GEMMA_4_NITRO,
            "system",
            "user",
            128,
            true,
        );
        assert_eq!(body["model"], OPENROUTER_POLISH_MODEL_GEMMA_4_NITRO);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["reasoning"]["enabled"], false);
        assert!(body.get("provider").is_none());
    }

    #[test]
    fn completion_budget_is_capped_at_one_thousand_twenty_four_tokens() {
        assert_eq!(completion_token_budget(&"word ".repeat(1_000)), 1024);
    }
}
