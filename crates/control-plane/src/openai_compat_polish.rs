//! Shared OpenAI-compatible streaming polish helpers for control-plane providers.

use axum::{Json, http::StatusCode};
use futures_util::StreamExt;
use reqwest::Response;
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

pub fn gateway_err(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_GATEWAY, Json(json!({ "error": message })))
}

pub async fn read_polish_stream(
    resp: Response,
    token_tx: mpsc::Sender<String>,
    request_started: Instant,
    provider: &str,
    model: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    let mut stream = resp.bytes_stream();
    let mut pending = Vec::<u8>::new();
    let mut output = String::new();
    let mut chunk_count = 0usize;
    let mut ttft_ms: Option<i64> = None;
    let mut saw_done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| gateway_err(&format!("server runtime {provider} stream failed: {e}")))?;
        pending.extend_from_slice(&chunk);

        while let Some(newline_pos) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=newline_pos).collect::<Vec<_>>();
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            if line.is_empty() {
                continue;
            }
            let Ok(line) = std::str::from_utf8(&line) else {
                return Err(gateway_err(&format!(
                    "server runtime {provider} stream returned invalid UTF-8"
                )));
            };
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                saw_done = true;
                break;
            }
            let value = serde_json::from_str::<Value>(data).map_err(|e| {
                gateway_err(&format!(
                    "server runtime {provider} stream parse failed: {e}"
                ))
            })?;
            if value.get("error").is_some() {
                return Err(gateway_err(&format!(
                    "server runtime {provider} stream returned an error"
                )));
            }
            if let Some(delta) = value
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if !delta.is_empty() {
                    ttft_ms.get_or_insert_with(|| request_started.elapsed().as_millis() as i64);
                    chunk_count += 1;
                    output.push_str(delta);
                    let _ = token_tx.send(delta.to_string()).await;
                }
            }
        }

        if saw_done {
            break;
        }
    }

    tracing::info!(
        "[runtime] {provider} stream complete model={model} ttft_ms={ttft_ms:?} total_ms={} chunks={} output_chars={}",
        request_started.elapsed().as_millis(),
        chunk_count,
        output.chars().count()
    );

    let output = output.trim().to_string();
    if output.is_empty() {
        return Err(gateway_err(&format!(
            "server runtime {provider} stream returned empty output"
        )));
    }
    Ok(output)
}

pub fn parse_chat_completion(value: &Value) -> Option<String> {
    value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
