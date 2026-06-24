use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing::{info, warn};

pub const DEFAULT_DEEPSEEK_LEARNING_MODEL: &str = "deepseek-v4-flash";

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: String,
}

pub fn learning_api_key() -> String {
    std::env::var("DEEPSEEK_API_KEY").unwrap_or_default()
}

pub fn learning_model() -> String {
    std::env::var("DEEPSEEK_LEARNING_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DEEPSEEK_LEARNING_MODEL.to_string())
}

fn endpoint() -> String {
    let base = std::env::var("DEEPSEEK_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https://api.deepseek.com".to_string());
    format!("{}/chat/completions", base.trim_end_matches('/'))
}

pub async fn chat_json<T>(
    client: &Client,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
    timeout: Duration,
    label: &str,
) -> Result<(T, u128, String), String>
where
    T: DeserializeOwned,
{
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("DEEPSEEK_API_KEY is not configured".to_string());
    }
    let model = learning_model();
    let start = Instant::now();
    let body = json!({
        "model": model,
        "temperature": 0,
        "max_tokens": max_tokens,
        "response_format": {"type": "json_object"},
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ]
    });
    let resp = client
        .post(endpoint())
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("DeepSeek {label} request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let preview = resp.text().await.unwrap_or_default();
        return Err(format!(
            "DeepSeek {label} returned {status}: {}",
            said_core::text::truncate_utf8(&preview, 300)
        ));
    }
    let response: ChatResponse = resp
        .json()
        .await
        .map_err(|e| format!("DeepSeek {label} response parse failed: {e}"))?;
    let content = response
        .choices
        .first()
        .map(|choice| choice.message.content.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("DeepSeek {label} returned empty content"))?;
    let json_text = extract_json_object(&content).unwrap_or(&content);
    let parsed = serde_json::from_str::<T>(json_text)
        .map_err(|e| format!("DeepSeek {label} JSON parse failed: {e}"))?;
    let latency_ms = start.elapsed().as_millis();
    info!("[deepseek] {label} complete model={model} latency_ms={latency_ms}");
    Ok((parsed, latency_ms, model))
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (start <= end).then_some(&raw[start..=end])
}

pub fn log_fail_closed(label: &str, err: &str) {
    warn!("[deepseek] {label} failed closed: {err}");
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Payload {
        ok: bool,
    }

    #[test]
    fn parses_json_object_from_wrapped_content() {
        let raw = "noise {\"ok\":true} trailing";
        let json = super::extract_json_object(raw).unwrap();
        let payload: Payload = serde_json::from_str(json).unwrap();
        assert!(payload.ok);
    }
}
