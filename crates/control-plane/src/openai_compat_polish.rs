//! Shared OpenAI-compatible streaming polish helpers for control-plane providers.

use axum::{Json, http::StatusCode};
use futures_util::StreamExt;
use reqwest::Response;
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Default)]
pub struct ProviderUsage {
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub cost_usd: Option<f64>,
    pub cost_source: Option<String>,
    pub generation_id: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct PolishCompletion {
    pub text: String,
    pub usage: ProviderUsage,
}

impl PolishCompletion {
    pub fn without_usage(text: String) -> Self {
        Self {
            text,
            usage: ProviderUsage::default(),
        }
    }
}

pub fn gateway_err(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_GATEWAY, Json(json!({ "error": message })))
}

pub async fn read_polish_stream(
    resp: Response,
    token_tx: mpsc::Sender<String>,
    request_started: Instant,
    provider: &str,
    model: &str,
) -> Result<PolishCompletion, (StatusCode, Json<Value>)> {
    let generation_id = resp
        .headers()
        .get("x-generation-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut stream = resp.bytes_stream();
    let mut pending = Vec::<u8>::new();
    let mut output = String::new();
    let mut chunk_count = 0usize;
    let mut ttft_ms: Option<i64> = None;
    let mut saw_done = false;
    let mut usage = ProviderUsage {
        generation_id,
        ..ProviderUsage::default()
    };

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
            if value.get("usage").is_some() {
                usage = usage_from_value(&value, usage.generation_id.clone());
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
    Ok(PolishCompletion {
        text: output,
        usage,
    })
}

pub fn parse_chat_completion(value: &Value) -> Option<PolishCompletion> {
    let text = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)?;
    Some(PolishCompletion {
        text,
        usage: usage_from_value(value, None),
    })
}

fn usage_from_value(value: &Value, generation_id: Option<String>) -> ProviderUsage {
    let raw = value.get("usage").cloned().unwrap_or(Value::Null);
    let input_tokens = raw
        .get("prompt_tokens")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let output_tokens = raw
        .get("completion_tokens")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let cost_usd = raw
        .get("cost")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    ProviderUsage {
        input_tokens,
        output_tokens,
        cost_usd,
        cost_source: cost_usd.map(|_| "provider_reported".to_string()),
        generation_id: generation_id
            .or_else(|| value.get("id").and_then(Value::as_str).map(str::to_string)),
        raw,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_chat_completion;

    #[test]
    fn parses_text_tokens_and_provider_cost() {
        let completion = parse_chat_completion(&json!({
            "id": "gen-1",
            "choices": [{"message": {"content": "Hello"}}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 8, "cost": 0.0000172}
        }))
        .expect("completion");
        assert_eq!(completion.text, "Hello");
        assert_eq!(completion.usage.input_tokens, Some(120));
        assert_eq!(completion.usage.output_tokens, Some(8));
        assert_eq!(completion.usage.generation_id.as_deref(), Some("gen-1"));
        assert_eq!(
            completion.usage.cost_source.as_deref(),
            Some("provider_reported")
        );
    }
}
