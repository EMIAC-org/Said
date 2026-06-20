//! OpenAI Codex Responses API client.
//!
//! Uses the same OAuth token that the official Codex CLI uses.
//! Endpoint: https://chatgpt.com/backend-api/codex/responses
//!
//! The payload format differs from standard Chat Completions:
//!   - `instructions` = system prompt
//!   - `input` = array of {type, role, content} (user/assistant messages only)
//! SSE events use `response.output_text.delta` (not `choices[].delta.content`).
//!
//! Gap 5: switched from `resp.bytes().await` (buffers entire response) to
//! `resp.bytes_stream()` (true token-by-token streaming, same as groq.rs).
//! First polished word now appears ~200-400ms after the LLM starts responding
//! instead of waiting for the full completion to arrive.

use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

pub use super::PolishResult;

const CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Available model IDs via the Codex Responses endpoint.
pub const MODEL_SMART: &str = "gpt-5.4";
pub const MODEL_MINI: &str = "gpt-5.4-mini";

/// Stream a polish operation through the Codex endpoint.
/// `model` should be `MODEL_SMART` or `MODEL_MINI`.
pub async fn stream_polish(
    client: &Client,
    access_token: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    let start = Instant::now();

    // Build Codex-format payload
    let payload = json!({
        "model":               model,
        "instructions":        system_prompt,
        "input": [
            { "type": "message", "role": "user", "content": user_message }
        ],
        "tools":               [],
        "tool_choice":         "auto",
        "parallel_tool_calls": false,
        "reasoning":           { "summary": "auto" },
        "store":               false,
        "stream":              true,
        "prompt_cache_key":    uuid::Uuid::new_v4().to_string(),
    });

    info!("[codex] POST {CODEX_ENDPOINT} model={model}");

    let resp = client
        .post(CODEX_ENDPOINT)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("codex request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!("[codex] HTTP {status}: {body}");
        return Err(format!("Codex API error {status}: {body}"));
    }

    // Gap 5: true SSE streaming — yield tokens as they arrive, not after full buffer.
    //
    // gpt-5 reasoning models can emit MULTIPLE output_text items in one
    // response — typically a draft and a final. Without tracking the active
    // output_item, naive concatenation of all output_text.delta events
    // produces duplicated output ("Hello, aur kya chal raha haiHello, aur
    // kya chal raha hai?"). We track the current output_item index and
    // RESET on every new message-typed item so only the LAST item's text
    // becomes the final polished output.
    let mut stream = resp.bytes_stream();
    let mut polished = String::new();
    let mut buf = String::new(); // incomplete SSE line buffer
    let mut ttft_ms: Option<u64> = None;
    let mut current_message_item_idx: Option<i64> = None;
    let mut item_resets = 0_usize;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("codex stream read error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        // SSE lines end with \n; process all complete lines
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

            let Ok(evt) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            match evt.get("type").and_then(|t| t.as_str()) {
                // Track output-item lifecycle. When a new MESSAGE item starts
                // and we already have one going, the previous one was a draft —
                // reset the buffer + emit a stream RESET so the typing path can
                // clear what it already typed.
                Some("response.output_item.added") => {
                    let item_type = evt
                        .pointer("/item/type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let item_idx = evt
                        .get("output_index")
                        .and_then(|i| i.as_i64())
                        .unwrap_or(-1);
                    if item_type == "message" {
                        if let Some(prev_idx) = current_message_item_idx {
                            if item_idx != prev_idx {
                                // Previous message item was a draft — discard it.
                                item_resets += 1;
                                warn!(
                                    "[codex] new message output_item (idx {item_idx}) replaced draft (idx {prev_idx}) — \
                                     {} chars discarded; reset stream",
                                    polished.len(),
                                );
                                polished.clear();
                                // Send a sentinel so the typing path can clear any partial output.
                                let _ = token_tx.send("\u{1F}__RESET__\u{1F}".to_string()).await;
                            }
                        }
                        current_message_item_idx = Some(item_idx);
                    }
                }
                Some("response.output_text.delta") => {
                    let delta = evt.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                    if !delta.is_empty() {
                        if ttft_ms.is_none() {
                            let ms = start.elapsed().as_millis() as u64;
                            ttft_ms = Some(ms);
                            info!(
                                "[codex] GAP-5: first token in {ms}ms (true streaming, not buffered)"
                            );
                        }
                        polished.push_str(delta);
                        trace!("[codex] token: {delta:?}");
                        let _ = token_tx.send(delta.to_string()).await;
                    }
                }
                Some("response.completed") => {
                    debug!("[codex] response.completed");
                    break;
                }
                _ => {}
            }
        }
    }

    let polish_ms = start.elapsed().as_millis() as u64;
    if item_resets > 0 {
        warn!(
            "[codex] {item_resets} draft item(s) discarded during stream — final polish: {} chars",
            polished.len(),
        );
    }
    info!("[codex] done in {polish_ms}ms, {} chars", polished.len());
    Ok(PolishResult {
        polished,
        polish_ms,
    })
}

/// Non-streaming JSON call to the Codex endpoint — for background tasks
/// like the edit classifier where we need a single JSON response, not a
/// token stream. Uses the same Codex Responses API but collects the full
/// output instead of streaming.
pub async fn call_json(
    client: &Client,
    access_token: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let start = Instant::now();

    let payload = json!({
        "model":               model,
        "instructions":        system_prompt,
        "input": [
            { "type": "message", "role": "user", "content": user_message }
        ],
        "tools":               [],
        "store":               false,
        "stream":              false,
    });

    info!("[codex-json] POST {CODEX_ENDPOINT} model={model}");

    let resp = client
        .post(CODEX_ENDPOINT)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("codex request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(
            "[codex-json] HTTP {status}: {}",
            said_core::text::truncate_utf8(&body, 300)
        );
        return Err(format!("Codex API error {status}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("codex json parse: {e}"))?;

    let ms = start.elapsed().as_millis();

    let content = body
        .get("output")
        .and_then(|o| o.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("message"))
                .last()
        })
        .and_then(|msg| msg.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("output_text"))
                .map(|p| p.get("text").and_then(|t| t.as_str()).unwrap_or(""))
                .reduce(|a, b| if b.len() > a.len() { b } else { a })
        })
        .unwrap_or("")
        .trim()
        .to_string();

    info!("[codex-json] done in {ms}ms, {} chars", content.len());
    Ok(content)
}

pub fn is_auth_error(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("invalid_token")
        || lower.contains("invalid token")
}

// ── Token refresh ─────────────────────────────────────────────────────────────

pub struct RefreshedToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
}

/// Exchange a refresh_token for a new access_token.
pub async fn refresh_token(client: &Client, refresh_tok: &str) -> Result<RefreshedToken, String> {
    let payload = json!({
        "client_id":     CLIENT_ID,
        "refresh_token": refresh_tok,
        "grant_type":    "refresh_token",
    });

    let resp = client
        .post(TOKEN_URL)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("refresh request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("refresh error {status}: {body}"));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("refresh parse failed: {e}"))?;

    let access_token = v["access_token"].as_str().unwrap_or("").to_string();
    let refresh_token = v["refresh_token"]
        .as_str()
        .unwrap_or(refresh_tok)
        .to_string();
    let expires_in = v["expires_in"].as_i64().unwrap_or(864_000);
    let expires_at_ms = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64)
        + expires_in * 1000;

    Ok(RefreshedToken {
        access_token,
        refresh_token,
        expires_at_ms,
    })
}
