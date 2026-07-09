//! Cerebras LLM client — OpenAI-compatible chat completions.
//!
//! API reference: https://inference-docs.cerebras.ai/
//! Endpoint: https://api.cerebras.ai/v1/chat/completions

use futures::StreamExt;
use reqwest::{Client, StatusCode, header::HeaderMap};
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, trace, warn};

use super::{LlmErrorDetails, encode_llm_error};

pub use super::PolishResult;

const CEREBRAS_ENDPOINT: &str = "https://api.cerebras.ai/v1/chat/completions";

pub const CEREBRAS_MODEL_DEFAULT: &str = "gemma-4-31b";
const MIN_COMPLETION_TOKENS: usize = 128;
const MAX_COMPLETION_TOKENS: usize = 4096;

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
    cerebras_api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    if cerebras_api_key.is_empty() {
        return Err(
            "Cerebras API key not set — configure CEREBRAS_API_KEY on the server".to_string(),
        );
    }

    let model = if model.is_empty() {
        CEREBRAS_MODEL_DEFAULT
    } else {
        model
    };
    let start = Instant::now();

    let max_completion_tokens = cerebras_completion_token_budget(user_message);
    let mut body = json!({
        "model":                 model,
        "stream":                true,
        "temperature":           0.0,
        "top_p":                 0.9,
        "max_completion_tokens": max_completion_tokens,
        "stop": [
            "=== BEGIN TRANSCRIPT",
            "=== END TRANSCRIPT",
            "<transcript>",
            "</transcript>",
        ],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });
    if model.contains("gpt-oss") {
        body["reasoning_effort"] = json!("low");
    }

    info!(
        "[cerebras] POST {CEREBRAS_ENDPOINT} model={model} max_completion_tokens={max_completion_tokens}"
    );

    let resp = client
        .post(CEREBRAS_ENDPOINT)
        .header("Authorization", format!("Bearer {cerebras_api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Cerebras request failed: {e}"))?;

    let status = resp.status();
    let headers = resp.headers().clone();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        warn!("[cerebras] HTTP {status}: {body_text}");
        if let Some(details) = cerebras_rate_limit_error(status, &headers, &body_text) {
            return Err(encode_llm_error(&details));
        }
        return Err(format!(
            "Cerebras API error {status}: {}",
            said_core::text::truncate_utf8(&body_text, 400)
        ));
    }
    log_rate_limit_headers("success", &headers);

    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Cerebras stream read error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();

            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }

            if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                for choice in chunk.choices {
                    if let Some(token) = choice.delta.content {
                        if !token.is_empty() {
                            if polished.is_empty() {
                                let ms = start.elapsed().as_millis();
                                info!("[cerebras] first token in {ms}ms");
                            }
                            polished.push_str(&token);
                            trace!("[cerebras] token: {token:?}");
                            let _ = token_tx.send(token).await;
                        }
                    }
                }
            }
        }
    }

    let polish_ms = start.elapsed().as_millis() as u64;
    info!("[cerebras] done in {polish_ms}ms, {} chars", polished.len());

    Ok(PolishResult {
        polished,
        polish_ms,
    })
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

fn cerebras_rate_limit_error(
    status: StatusCode,
    headers: &HeaderMap,
    body_text: &str,
) -> Option<LlmErrorDetails> {
    let has_rate_limit_headers = headers
        .keys()
        .any(|name| name.as_str().starts_with("x-ratelimit-"));
    if status != StatusCode::TOO_MANY_REQUESTS
        && !(has_rate_limit_headers && body_text.to_ascii_lowercase().contains("rate"))
    {
        return None;
    }

    let reset_tokens = header_value(headers, "x-ratelimit-reset-tokens-minute");
    let reset_requests = header_value(headers, "x-ratelimit-reset-requests-day");
    let remaining_tokens = header_value(headers, "x-ratelimit-remaining-tokens-minute");
    let remaining_requests = header_value(headers, "x-ratelimit-remaining-requests-day");

    let mut message = "Cerebras rate limit hit".to_string();
    if let Some(reset) = reset_tokens.as_deref() {
        message.push_str(&format!(" — token limit resets in {reset}s"));
    } else if let Some(reset) = reset_requests.as_deref() {
        message.push_str(&format!(" — request limit resets in {reset}s"));
    }
    if let Some(remaining) = remaining_tokens.as_deref() {
        message.push_str(&format!("; tokens remaining this minute: {remaining}"));
    }
    if let Some(remaining) = remaining_requests.as_deref() {
        message.push_str(&format!("; requests remaining today: {remaining}"));
    }

    let diagnostic = format!(
        "Cerebras API error {status}\n{}\nbody: {}",
        rate_limit_headers_diagnostic(headers),
        said_core::text::truncate_utf8(body_text, 800)
    );

    Some(LlmErrorDetails {
        message,
        error_code: Some("cerebras_rate_limit".to_string()),
        retryable: Some(true),
        diagnostic: Some(diagnostic),
    })
}

fn rate_limit_headers_diagnostic(headers: &HeaderMap) -> String {
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

fn log_rate_limit_headers(context: &str, headers: &HeaderMap) {
    let Some(remaining_tokens) = header_value(headers, "x-ratelimit-remaining-tokens-minute")
    else {
        return;
    };
    info!(
        "[cerebras] rate headers context={} remaining_tokens_minute={} remaining_requests_day={} reset_tokens_s={} reset_requests_s={}",
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

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{cerebras_completion_token_budget, current_transcript_block};

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
}
