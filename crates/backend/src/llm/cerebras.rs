//! Cerebras LLM client.
//!
//! Cerebras exposes an OpenAI-compatible Chat Completions endpoint running on
//! Cerebras wafer-scale hardware — ~2,600 tok/s on Llama 4 Scout, 4.4x faster
//! than Groq. Free tier: 30 RPM, 1M tokens/day, 8K context.
//!
//! API reference: https://inference-docs.cerebras.ai/
//! Endpoint: https://api.cerebras.ai/v1/chat/completions
//! Auth: Authorization: Bearer {cerebras_api_key}

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, trace, warn};

pub use super::PolishResult;

const CEREBRAS_ENDPOINT: &str = "https://api.cerebras.ai/v1/chat/completions";

pub const CEREBRAS_MODEL_DEFAULT: &str = "llama3.1-8b";

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
        return Err("Cerebras API key not set — add it in Settings → API Keys".to_string());
    }

    let model = if model.is_empty() {
        CEREBRAS_MODEL_DEFAULT
    } else {
        model
    };
    let start = Instant::now();

    let estimated_input_tokens = user_message.len() / 4;
    let max_tokens = (estimated_input_tokens * 2 + 256).min(4096) as u32;

    let body = json!({
        "model":       model,
        "stream":      true,
        "temperature": 0.0,
        "top_p":       0.9,
        "max_tokens":  max_tokens,
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

    info!("[cerebras] POST {CEREBRAS_ENDPOINT} model={model}");

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
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        warn!("[cerebras] HTTP {status}: {body_text}");
        return Err(format!(
            "Cerebras API error {status}: {}",
            &body_text[..body_text.len().min(400)]
        ));
    }

    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new();
    let mut ttft_ms: Option<u64> = None;

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
                            if ttft_ms.is_none() {
                                let ms = start.elapsed().as_millis() as u64;
                                ttft_ms = Some(ms);
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
