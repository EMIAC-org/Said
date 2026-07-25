//! DeepSeek OpenAI-compatible chat completions for server-runtime polish.

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::openai_compat_polish::{
    PolishCompletion, gateway_err, parse_chat_completion, read_polish_stream,
};

const DEEPSEEK_ENDPOINT: &str = "https://api.deepseek.com/chat/completions";
const MIN_COMPLETION_TOKENS: usize = 128;
const MAX_COMPLETION_TOKENS: usize = 1024;

pub async fn call_deepseek(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<PolishCompletion, (StatusCode, Json<Value>)> {
    let stream_tokens = token_tx.is_some();
    let body = deepseek_polish_body(
        model,
        system_prompt,
        user_message,
        completion_token_budget(user_message),
        stream_tokens,
    );

    tracing::info!("[runtime] POST {DEEPSEEK_ENDPOINT} model={model} thinking=disabled");

    let request_started = Instant::now();
    let resp = crate::HTTP_CLIENT
        .post(DEEPSEEK_ENDPOINT)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| gateway_err(&format!("server runtime DeepSeek request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] DeepSeek HTTP {status}: {}",
            said_core::text::truncate_utf8(&preview, 300)
        );
        return Err(gateway_err(&format!("DeepSeek returned {status}")));
    }

    if let Some(token_tx) = token_tx {
        return read_polish_stream(resp, token_tx, request_started, "deepseek", model).await;
    }

    let value: Value = resp.json().await.map_err(|e| {
        gateway_err(&format!(
            "server runtime DeepSeek response parse failed: {e}"
        ))
    })?;
    parse_chat_completion(&value)
        .ok_or_else(|| gateway_err("server runtime DeepSeek returned empty output"))
}

fn deepseek_polish_body(
    model: &str,
    system_prompt: &str,
    user_message: &str,
    max_tokens: u32,
    stream: bool,
) -> Value {
    json!({
        "model": model,
        "thinking": {"type": "disabled"},
        "max_tokens": max_tokens,
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
    let source = user_message
        .split_once("=== BEGIN CURRENT TRANSCRIPT ===")
        .and_then(|(_, tail)| tail.split_once("=== END CURRENT TRANSCRIPT ==="))
        .map(|(transcript, _)| transcript.trim())
        .filter(|transcript| !transcript.is_empty())
        .unwrap_or(user_message);
    let word_count = source.split_whitespace().count().max(1);
    (word_count * 2 + 64).clamp(MIN_COMPLETION_TOKENS, MAX_COMPLETION_TOKENS) as u32
}

#[cfg(test)]
mod tests {
    use super::{completion_token_budget, deepseek_polish_body};
    use said_core::polish::model::DEEPSEEK_POLISH_MODEL_V4_FLASH;

    #[test]
    fn body_pins_v4_flash_with_thinking_disabled() {
        let body =
            deepseek_polish_body(DEEPSEEK_POLISH_MODEL_V4_FLASH, "system", "user", 128, true);
        assert_eq!(body["model"], DEEPSEEK_POLISH_MODEL_V4_FLASH);
        assert_eq!(body["thinking"]["type"], "disabled");
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["stream"], true);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn completion_budget_is_bounded() {
        assert_eq!(completion_token_budget("Thik hai"), 128);
        assert_eq!(completion_token_budget(&"word ".repeat(1_000)), 1024);
    }
}
