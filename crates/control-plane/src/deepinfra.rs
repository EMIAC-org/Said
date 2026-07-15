//! DeepInfra OpenAI-compatible chat completions for server-runtime polish.

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::openai_compat_polish::{
    PolishCompletion, gateway_err, parse_chat_completion, read_polish_stream,
};

const DEEPINFRA_ENDPOINT: &str = "https://api.deepinfra.com/v1/openai/chat/completions";
const DEEPINFRA_SERVICE_TIER: &str = "priority";
const MIN_COMPLETION_TOKENS: usize = 128;
const MAX_COMPLETION_TOKENS: usize = 1024;

pub async fn call_deepinfra(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<PolishCompletion, (StatusCode, Json<Value>)> {
    let stream_tokens = token_tx.is_some();
    let max_tokens = completion_token_budget(user_message);
    let body = deepinfra_polish_body(
        model,
        system_prompt,
        user_message,
        max_tokens,
        stream_tokens,
    );

    tracing::info!("[runtime] POST {DEEPINFRA_ENDPOINT} model={model}");

    let client = &*crate::HTTP_CLIENT;
    let request_started = Instant::now();
    let resp = client
        .post(DEEPINFRA_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| gateway_err(&format!("server runtime DeepInfra request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] DeepInfra HTTP {status}: {}",
            said_core::text::truncate_utf8(&preview, 300)
        );
        return Err(gateway_err(&format!("DeepInfra returned {status}")));
    }

    if let Some(token_tx) = token_tx {
        return read_polish_stream(resp, token_tx, request_started, "deepinfra", model).await;
    }

    let value: Value = resp.json().await.map_err(|e| {
        gateway_err(&format!(
            "server runtime DeepInfra response parse failed: {e}"
        ))
    })?;
    parse_chat_completion(&value)
        .ok_or_else(|| gateway_err("server runtime DeepInfra returned empty output"))
}

fn deepinfra_polish_body(
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
        "stream": stream,
        "service_tier": DEEPINFRA_SERVICE_TIER,
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
    use super::{
        DEEPINFRA_SERVICE_TIER, completion_token_budget, current_transcript_block,
        deepinfra_polish_body,
    };
    use said_core::polish::model::DEEPINFRA_POLISH_MODEL_GEMMA_4_26B_A4B;

    #[test]
    fn extracts_current_transcript_for_budgeting() {
        let message = "before\n=== BEGIN CURRENT TRANSCRIPT ===\none two three\n=== END CURRENT TRANSCRIPT ===";
        assert_eq!(current_transcript_block(message), Some("one two three"));
    }

    #[test]
    fn completion_budget_is_bounded_for_short_and_long_dictation() {
        assert_eq!(completion_token_budget("Thik hai"), 128);
        assert_eq!(completion_token_budget(&"word ".repeat(1_000)), 1024);
    }

    #[test]
    fn body_pins_deepinfra_gemma_without_reasoning_parameters() {
        let body = deepinfra_polish_body(
            DEEPINFRA_POLISH_MODEL_GEMMA_4_26B_A4B,
            "system",
            "user",
            128,
            true,
        );
        assert_eq!(body["model"], DEEPINFRA_POLISH_MODEL_GEMMA_4_26B_A4B);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["service_tier"], DEEPINFRA_SERVICE_TIER);
        assert!(body.get("reasoning").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }
}
