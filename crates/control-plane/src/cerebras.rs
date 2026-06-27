//! Cerebras OpenAI-compatible chat completions for server-runtime polish.

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::openai_compat_polish::{gateway_err, parse_chat_completion, read_polish_stream};

const CEREBRAS_ENDPOINT: &str = "https://api.cerebras.ai/v1/chat/completions";

pub async fn call_cerebras(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let estimated_input_tokens = user_message.len() / 4;
    let mut max_tokens = (estimated_input_tokens * 2 + 256).min(8192) as u32;
    let stream_tokens = token_tx.is_some();
    let mut body = json!({
        "model": model,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        "stream": stream_tokens,
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
    });
    if model.contains("gpt-oss") {
        max_tokens = max_tokens.max(4096);
        body["max_tokens"] = json!(max_tokens);
        body["reasoning_effort"] = json!("low");
    }

    tracing::info!("[runtime] POST {CEREBRAS_ENDPOINT} model={model}");

    let client = &*crate::HTTP_CLIENT;
    let request_started = Instant::now();
    let resp = client
        .post(CEREBRAS_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| gateway_err(&format!("server runtime Cerebras request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] Cerebras HTTP {status}: {}",
            said_core::text::truncate_utf8(&preview, 300)
        );
        return Err(gateway_err(&format!("Cerebras returned {status}")));
    }

    if let Some(token_tx) = token_tx {
        return read_polish_stream(resp, token_tx, request_started, "cerebras", model).await;
    }

    let value: Value = resp.json().await.map_err(|e| {
        gateway_err(&format!(
            "server runtime Cerebras response parse failed: {e}"
        ))
    })?;
    parse_chat_completion(&value)
        .ok_or_else(|| gateway_err("server runtime Cerebras returned empty output"))
}
