//! Cerebras OpenAI-compatible chat completions for server-runtime polish.

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::openai_compat_polish::{gateway_err, parse_chat_completion, read_polish_stream};

const CEREBRAS_ENDPOINT: &str = "https://api.cerebras.ai/v1/chat/completions";
const MIN_COMPLETION_TOKENS: usize = 128;
const MAX_COMPLETION_TOKENS: usize = 4096;

pub async fn call_cerebras(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let max_completion_tokens = cerebras_completion_token_budget(user_message);
    let stream_tokens = token_tx.is_some();
    let mut body = json!({
        "model": model,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_completion_tokens": max_completion_tokens,
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
        body["reasoning_effort"] = json!("low");
    }

    tracing::info!(
        "[runtime] POST {CEREBRAS_ENDPOINT} model={model} max_completion_tokens={max_completion_tokens}"
    );

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

    let status = resp.status();
    let headers = resp.headers().clone();
    if !status.is_success() {
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] Cerebras HTTP {status}: {}\n{}",
            said_core::text::truncate_utf8(&preview, 300),
            rate_limit_headers_diagnostic(&headers)
        );
        return Err(gateway_err(&cerebras_error_message(status, &headers)));
    }
    log_rate_limit_headers("success", &headers);

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

fn cerebras_completion_token_budget(user_message: &str) -> u32 {
    let source = current_transcript_block(user_message).unwrap_or(user_message);
    let word_count = source.split_whitespace().count().max(1);
    (word_count * 2 + 64).clamp(MIN_COMPLETION_TOKENS, MAX_COMPLETION_TOKENS) as u32
}

fn current_transcript_block(user_message: &str) -> Option<&str> {
    let start = "=== BEGIN CURRENT TRANSCRIPT ===";
    let end = "=== END CURRENT TRANSCRIPT ===";
    let after_start = user_message.split_once(start)?.1;
    let transcript = after_start.split_once(end)?.0.trim();
    (!transcript.is_empty()).then_some(transcript)
}

fn rate_limit_headers_diagnostic(headers: &reqwest::header::HeaderMap) -> String {
    const NAMES: &[&str] = &[
        "x-ratelimit-limit-requests-day",
        "x-ratelimit-limit-tokens-minute",
        "x-ratelimit-remaining-requests-day",
        "x-ratelimit-remaining-tokens-minute",
        "x-ratelimit-reset-requests-day",
        "x-ratelimit-reset-tokens-minute",
        "retry-after",
    ];

    let mut lines = Vec::with_capacity(NAMES.len());
    for name in NAMES {
        let value = header_value(headers, name).unwrap_or_else(|| "<missing>".to_string());
        lines.push(format!("{name}: {value}"));
    }
    lines.join("\n")
}

fn cerebras_error_message(status: StatusCode, headers: &reqwest::header::HeaderMap) -> String {
    let remaining_tokens = header_value(headers, "x-ratelimit-remaining-tokens-minute");
    let reset_tokens = header_value(headers, "x-ratelimit-reset-tokens-minute");
    let remaining_requests = header_value(headers, "x-ratelimit-remaining-requests-day");
    let reset_requests = header_value(headers, "x-ratelimit-reset-requests-day");
    let retry_after = header_value(headers, "retry-after");

    let mut parts = vec![format!("Cerebras returned {status}")];
    if let Some(value) = remaining_tokens {
        parts.push(format!("x-ratelimit-remaining-tokens-minute={value}"));
    }
    if let Some(value) = reset_tokens {
        parts.push(format!("x-ratelimit-reset-tokens-minute={value}"));
    }
    if let Some(value) = remaining_requests {
        parts.push(format!("x-ratelimit-remaining-requests-day={value}"));
    }
    if let Some(value) = reset_requests {
        parts.push(format!("x-ratelimit-reset-requests-day={value}"));
    }
    if let Some(value) = retry_after {
        parts.push(format!("retry-after={value}"));
    }
    parts.join("; ")
}

fn log_rate_limit_headers(context: &str, headers: &reqwest::header::HeaderMap) {
    let Some(remaining_tokens) = header_value(headers, "x-ratelimit-remaining-tokens-minute")
    else {
        return;
    };
    tracing::info!(
        "[runtime] Cerebras rate headers context={} remaining_tokens_minute={} remaining_requests_day={} reset_tokens_s={} reset_requests_s={}",
        context,
        remaining_tokens,
        header_value(headers, "x-ratelimit-remaining-requests-day")
            .unwrap_or_else(|| "unknown".to_string()),
        header_value(headers, "x-ratelimit-reset-tokens-minute")
            .unwrap_or_else(|| "unknown".to_string()),
        header_value(headers, "x-ratelimit-reset-requests-day")
            .unwrap_or_else(|| "unknown".to_string()),
    );
}

fn header_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{
        cerebras_completion_token_budget, cerebras_error_message, current_transcript_block,
    };
    use reqwest::{StatusCode, header::HeaderMap};

    #[test]
    fn extracts_current_transcript_block_for_budgeting() {
        let message = "instructions\n=== BEGIN CURRENT TRANSCRIPT ===\none two three\n=== END CURRENT TRANSCRIPT ===";
        assert_eq!(current_transcript_block(message), Some("one two three"));
    }

    #[test]
    fn completion_budget_has_small_floor_for_short_dictation() {
        let message = "=== BEGIN CURRENT TRANSCRIPT ===\nThik hai\n=== END CURRENT TRANSCRIPT ===";
        assert_eq!(cerebras_completion_token_budget(message), 128);
    }

    #[test]
    fn completion_budget_scales_with_transcript_words() {
        let transcript = std::iter::repeat("word")
            .take(200)
            .collect::<Vec<_>>()
            .join(" ");
        let message = format!(
            "prefix words ignored\n=== BEGIN CURRENT TRANSCRIPT ===\n{transcript}\n=== END CURRENT TRANSCRIPT ==="
        );
        assert_eq!(cerebras_completion_token_budget(&message), 464);
    }

    #[test]
    fn completion_budget_caps_very_long_dictation() {
        let transcript = std::iter::repeat("word")
            .take(5_000)
            .collect::<Vec<_>>()
            .join(" ");
        let message = format!(
            "=== BEGIN CURRENT TRANSCRIPT ===\n{transcript}\n=== END CURRENT TRANSCRIPT ==="
        );
        assert_eq!(cerebras_completion_token_budget(&message), 4096);
    }

    #[test]
    fn cerebras_error_message_prioritizes_rate_limit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining-tokens-minute", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset-tokens-minute", "17".parse().unwrap());
        let message = cerebras_error_message(StatusCode::TOO_MANY_REQUESTS, &headers);
        assert!(message.starts_with("Cerebras returned 429 Too Many Requests"));
        assert!(message.contains("x-ratelimit-remaining-tokens-minute=0"));
        assert!(message.contains("x-ratelimit-reset-tokens-minute=17"));
    }
}
