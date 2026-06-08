//! Async HTTP client for the local airnote-backend daemon.
//!
//! All functions take a `&BackendEndpoint` (url + secret).
//! They never interact with the child process — only the BackendState owns that.

use futures::StreamExt;
use reqwest::Client;
use said_core::deepgram::{BiasPackage, TranscriptMeta};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use crate::backend::BackendEndpoint;

// ── Shared types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub user_id: String,
    pub selected_model: String,
    pub tone_preset: String,
    pub custom_prompt: Option<String>,
    pub language: String,
    #[serde(default)]
    pub output_language: String,
    pub auto_paste: bool,
    pub edit_capture: bool,
    pub polish_text_hotkey: String,
    pub record_hotkey: String,
    #[serde(default = "default_learning_enabled")]
    pub learning_enabled: bool,
    #[serde(default)]
    pub server_runtime_enabled: bool,
    #[serde(default)]
    pub server_audio_runtime_enabled: bool,
    // API keys (stored in SQLite; None if not set yet)
    #[serde(default)]
    pub deepgram_api_key: Option<String>,
    #[serde(default)]
    pub gemini_api_key: Option<String>,
    #[serde(default)]
    pub gateway_api_key: Option<String>,
    #[serde(default)]
    pub groq_api_key: Option<String>,
    #[serde(default)]
    pub cerebras_api_key: Option<String>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "cerebras" | "openai_codex"
    #[serde(default = "default_llm_provider")]
    pub llm_provider: String,
}

fn default_llm_provider() -> String {
    "gateway".to_string()
}

fn default_learning_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrefsUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone_preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_paste: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_capture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polish_text_hotkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_hotkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learning_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_runtime_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_audio_runtime_enabled: Option<bool>,
    // API keys — Some(Some(value)) = set; Some(None) = clear; None = don't touch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_api_key: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepgram_api_key: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini_api_key: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cerebras_api_key: Option<Option<String>>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "cerebras" | "openai_codex"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplateResponse {
    pub kind: String,
    pub title: String,
    pub base_version: String,
    pub active_body: String,
    pub draft_body: Option<String>,
    pub default_body: String,
    pub updated_at: i64,
    pub applied_at: Option<i64>,
    pub has_draft: bool,
    pub active_is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTestResponse {
    pub output: String,
    pub model_used: String,
    pub latency_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub timestamp_ms: i64,
    pub transcript: String,
    pub polished: String,
    pub final_text: Option<String>,
    pub word_count: i64,
    pub recording_seconds: f64,
    pub model_used: String,
    pub confidence: Option<f64>,
    pub transcribe_ms: Option<i64>,
    pub embed_ms: Option<i64>,
    pub polish_ms: Option<i64>,
    pub target_app: Option<String>,
    pub edit_count: i64,
    pub source: String,
    pub audio_id: Option<String>,
    pub enriched_transcript: Option<String>,
}

/// Result of a completed polish operation (from the `done` SSE event).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolishDone {
    pub recording_id: String,
    pub transcript: String,
    pub polished: String,
    pub model_used: String,
    pub confidence: Option<f64>,
    pub audio_id: Option<String>,
    pub source: Option<String>,
    pub target_app: Option<String>,
    pub output_language: Option<String>,
    #[serde(default)]
    pub enriched_transcript: Option<String>,
    pub examples_used: u32,
    pub latency_ms: PolishLatency,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolishLatency {
    pub transcribe: i64,
    pub embed: i64,
    pub retrieve: i64,
    pub polish: i64,
    pub total: i64,
}

// ── SSE event enum ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PolishEvent {
    Status {
        phase: String,
        transcript: Option<String>,
    },
    Token {
        token: String,
    },
    Done(PolishDone),
    Error {
        message: String,
        audio_id: Option<String>,
        error_code: Option<String>,
    },
}

fn http_error_event(
    path: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> (String, Option<String>) {
    let preview = &body[..body.len().min(300)];
    if let Ok(val) = serde_json::from_str::<Value>(body) {
        let error_code = val
            .get("error_code")
            .and_then(Value::as_str)
            .map(str::to_string);
        let message = val
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        return (
            format!("{path} error {status}: {preview}"),
            error_code.or_else(|| {
                if message == "API keys required" {
                    Some("missing_api_keys".to_string())
                } else {
                    None
                }
            }),
        );
    }
    (format!("{path} error {status}: {preview}"), None)
}

fn redact_pref_key_fields(raw: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    for field in [
        "gateway_api_key",
        "deepgram_api_key",
        "gemini_api_key",
        "groq_api_key",
    ] {
        if let Some(slot) = value.get_mut(field) {
            *slot = match slot {
                Value::Null => Value::Null,
                _ => Value::String("<redacted>".to_string()),
            };
        }
    }
    serde_json::to_string(&value).unwrap_or_else(|_| raw.to_string())
}

// ── Voice polish ──────────────────────────────────────────────────────────────

/// Stream polish events for a WAV recording. Calls `on_event` as events arrive.
///
/// `pre_transcript` — if the caller already obtained a transcript via Deepgram
/// WebSocket streaming (P5), pass it here so the backend can skip its own STT call.
pub async fn stream_voice_polish<F>(
    ep: &BackendEndpoint,
    wav_data: Vec<u8>,
    target_app: Option<String>,
    client_run_id: Option<String>,
    pre_transcript: Option<String>,
    pre_transcript_meta: Option<TranscriptMeta>,
    repair_mode: Option<String>,
    screen_context: Option<String>,
    message_polish_mode: bool,
    mut on_event: F,
) -> Result<PolishDone, String>
where
    F: FnMut(PolishEvent),
{
    let url = format!("{}/v1/voice/polish", ep.url);
    let client = Client::new();

    let mut form = reqwest::multipart::Form::new();
    if !wav_data.is_empty() {
        form = form.part(
            "audio",
            reqwest::multipart::Part::bytes(wav_data)
                .file_name("recording.wav")
                .mime_str("audio/wav")
                .map_err(|e| format!("mime error: {e}"))?,
        );
    }
    if let Some(app) = target_app {
        form = form.text("target_app", app);
    }
    if let Some(client_run_id) = client_run_id {
        form = form.text("client_run_id", client_run_id);
    }
    // P5: forward pre-transcribed text so backend can skip Deepgram HTTP call
    if let Some(transcript) = pre_transcript {
        form = form.text("pre_transcript", transcript);
    }
    if let Some(meta) = pre_transcript_meta {
        form = form.text(
            "pre_transcript_meta",
            serde_json::to_string(&meta).map_err(|e| format!("encode transcript meta: {e}"))?,
        );
    }
    if let Some(mode) = repair_mode {
        form = form.text("repair_mode", mode);
    }
    if let Some(ctx) = screen_context {
        let trimmed = ctx.chars().take(500).collect::<String>();
        if !trimmed.trim().is_empty() {
            form = form.text("screen_context", trimmed);
        }
    }
    if message_polish_mode {
        form = form.text("message_polish_mode", "true");
    }

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .header("Accept", "text/event-stream")
        .multipart(form)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("voice polish request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let (message, error_code) = http_error_event("voice/polish", status, &body);
        on_event(PolishEvent::Error {
            message: message.clone(),
            audio_id: None,
            error_code,
        });
        return Err(message);
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

pub async fn stream_text_polish<F>(
    ep: &BackendEndpoint,
    text: String,
    target_app: Option<String>,
    tone_override: Option<String>,
    mut on_event: F,
) -> Result<PolishDone, String>
where
    F: FnMut(PolishEvent),
{
    let url = format!("{}/v1/text/polish", ep.url);
    let client = Client::new();
    let body = serde_json::json!({
        "text":          text,
        "target_app":    target_app,
        "tone_override": tone_override,
    });

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .header("Accept", "text/event-stream")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("text polish request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let (message, error_code) = http_error_event("text/polish", status, &body);
        on_event(PolishEvent::Error {
            message: message.clone(),
            audio_id: None,
            error_code,
        });
        return Err(message);
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

pub async fn stream_text_refine_last<F>(
    ep: &BackendEndpoint,
    source_text: String,
    previous_output: String,
    tone: Option<String>,
    on_event: F,
) -> Result<PolishDone, String>
where
    F: FnMut(PolishEvent),
{
    let url = format!("{}/v1/text/refine-last", ep.url);
    let client = Client::new();
    let body = serde_json::json!({
        "source_text": source_text,
        "previous_output": previous_output,
        "tone": tone,
    });

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .header("Accept", "text/event-stream")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("text refine request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "text/refine-last error {status}: {}",
            &body[..body.len().min(300)]
        ));
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

pub async fn stream_voice_repair<F>(
    ep: &BackendEndpoint,
    transcript: String,
    previous_output: String,
    target_app: Option<String>,
    output_language: String,
    audio_id: Option<String>,
    enriched_transcript: Option<String>,
    reason: Option<String>,
    on_event: F,
) -> Result<PolishDone, String>
where
    F: FnMut(PolishEvent),
{
    let url = format!("{}/v1/voice/repair", ep.url);
    let client = Client::new();
    let body = serde_json::json!({
        "transcript": transcript,
        "previous_output": previous_output,
        "target_app": target_app,
        "output_language": output_language,
        "audio_id": audio_id,
        "enriched_transcript": enriched_transcript,
        "reason": reason,
    });

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .header("Accept", "text/event-stream")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("voice repair request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "voice/repair error {status}: {}",
            &body[..body.len().min(300)]
        ));
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

// ── SSE parser ────────────────────────────────────────────────────────────────

async fn consume_sse<S, F>(mut stream: S, mut on_event: F) -> Result<PolishDone, String>
where
    S: StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    F: FnMut(PolishEvent),
{
    let mut buf = String::new();
    let mut done_event: Option<PolishDone> = None;
    // Track the most recently seen `event:` line so we can dispatch correctly
    let mut event_name = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE lines
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();

            if line.is_empty() {
                event_name.clear();
                continue;
            }

            if let Some(name) = line.strip_prefix("event: ") {
                event_name = name.trim().to_string();
                continue;
            }

            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }

            parse_and_dispatch(data, &event_name, &mut on_event, &mut done_event);
        }
    }

    done_event.ok_or_else(|| "SSE stream ended without a `done` event".into())
}

fn parse_and_dispatch(
    data: &str,
    event_name: &str,
    on_event: &mut impl FnMut(PolishEvent),
    done_event: &mut Option<PolishDone>,
) {
    let Ok(val) = serde_json::from_str::<Value>(data) else {
        warn!("[api] unparseable SSE data: {data:?}");
        return;
    };

    // Prefer explicit event name; fall back to key-sniffing for resilience
    match event_name {
        "token" => {
            if let Some(token) = val.get("token").and_then(Value::as_str) {
                debug!("[api] token: {token:?}");
                on_event(PolishEvent::Token {
                    token: token.to_string(),
                });
            }
        }
        "status" => {
            if let Some(phase) = val.get("phase").and_then(Value::as_str) {
                let transcript = val
                    .get("transcript")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                on_event(PolishEvent::Status {
                    phase: phase.to_string(),
                    transcript,
                });
            }
        }
        "done" => {
            if let Some(done) = parse_done(&val) {
                on_event(PolishEvent::Done(done.clone()));
                *done_event = Some(done);
            }
        }
        "error" => {
            if let Some(msg) = val.get("message").and_then(Value::as_str) {
                let audio_id = val
                    .get("audio_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                on_event(PolishEvent::Error {
                    message: msg.to_string(),
                    audio_id,
                    error_code: val
                        .get("error_code")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
        // Key-sniff fallback (handles backends that omit the `event:` line)
        _ => {
            if let Some(token) = val.get("token").and_then(Value::as_str) {
                on_event(PolishEvent::Token {
                    token: token.to_string(),
                });
            } else if let Some(phase) = val.get("phase").and_then(Value::as_str) {
                let transcript = val
                    .get("transcript")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                on_event(PolishEvent::Status {
                    phase: phase.to_string(),
                    transcript,
                });
            } else if val.get("recording_id").is_some() {
                if let Some(done) = parse_done(&val) {
                    on_event(PolishEvent::Done(done.clone()));
                    *done_event = Some(done);
                }
            } else if let Some(msg) = val.get("message").and_then(Value::as_str) {
                let audio_id = val
                    .get("audio_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                on_event(PolishEvent::Error {
                    message: msg.to_string(),
                    audio_id,
                    error_code: val
                        .get("error_code")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
    }
}

fn parse_done(val: &Value) -> Option<PolishDone> {
    let recording_id = val["recording_id"].as_str()?.to_string();
    let transcript = val["transcript"].as_str().unwrap_or("").to_string();
    let polished = val["polished"].as_str().unwrap_or("").to_string();
    let model_used = val["model_used"].as_str().unwrap_or("").to_string();
    let confidence = val["confidence"].as_f64();
    let examples = val["examples_used"].as_u64().unwrap_or(0) as u32;
    let lat = val.get("latency_ms").cloned().unwrap_or_default();
    Some(PolishDone {
        recording_id,
        transcript,
        polished,
        model_used,
        confidence,
        audio_id: val
            .get("audio_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        source: val
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        target_app: val
            .get("target_app")
            .and_then(Value::as_str)
            .map(str::to_string),
        output_language: val
            .get("output_language")
            .and_then(Value::as_str)
            .map(str::to_string),
        enriched_transcript: val
            .get("enriched_transcript")
            .and_then(Value::as_str)
            .map(str::to_string),
        examples_used: examples,
        latency_ms: PolishLatency {
            transcribe: lat["transcribe"].as_i64().unwrap_or(0),
            embed: lat["embed"].as_i64().unwrap_or(0),
            retrieve: lat["retrieve"].as_i64().unwrap_or(0),
            polish: lat["polish"].as_i64().unwrap_or(0),
            total: lat["total"].as_i64().unwrap_or(0),
        },
    })
}

// ── Preferences ───────────────────────────────────────────────────────────────

pub async fn get_preferences(ep: &BackendEndpoint) -> Result<Preferences, String> {
    let url = format!("{}/v1/preferences", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("get prefs failed: {e}"))?
        .json::<Preferences>()
        .await
        .map_err(|e| format!("parse prefs failed: {e}"))
}

pub async fn patch_preferences(
    ep: &BackendEndpoint,
    update: PrefsUpdate,
) -> Result<Preferences, String> {
    let url = format!("{}/v1/preferences", ep.url);
    let body = serde_json::to_string(&update).unwrap_or_else(|e| format!("<serialize error: {e}>"));
    tracing::info!(
        "[patch_prefs] → PATCH {url}  body={}",
        redact_pref_key_fields(&body)
    );
    let resp = Client::new()
        .patch(&url)
        .header("Authorization", ep.bearer())
        .json(&update)
        .send()
        .await
        .map_err(|e| format!("patch prefs failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    tracing::info!(
        "[patch_prefs] ← {status}  body={}",
        redact_pref_key_fields(&text)
    );
    serde_json::from_str::<Preferences>(&text).map_err(|e| {
        format!(
            "parse prefs failed: {e} — raw: {}",
            &text[..text.len().min(200)]
        )
    })
}

pub async fn get_voice_prompt(ep: &BackendEndpoint) -> Result<PromptTemplateResponse, String> {
    let url = format!("{}/v1/prompts/voice", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("get voice prompt failed: {e}"))?
        .json::<PromptTemplateResponse>()
        .await
        .map_err(|e| format!("parse voice prompt failed: {e}"))
}

pub async fn save_voice_prompt_draft(
    ep: &BackendEndpoint,
    draft_body: String,
) -> Result<PromptTemplateResponse, String> {
    let url = format!("{}/v1/prompts/voice/draft", ep.url);
    Client::new()
        .patch(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "draft_body": draft_body }))
        .send()
        .await
        .map_err(|e| format!("save voice prompt draft failed: {e}"))?
        .json::<PromptTemplateResponse>()
        .await
        .map_err(|e| format!("parse voice prompt failed: {e}"))
}

pub async fn apply_voice_prompt_draft(
    ep: &BackendEndpoint,
) -> Result<PromptTemplateResponse, String> {
    let url = format!("{}/v1/prompts/voice/apply", ep.url);
    Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("apply voice prompt failed: {e}"))?
        .json::<PromptTemplateResponse>()
        .await
        .map_err(|e| format!("parse voice prompt failed: {e}"))
}

pub async fn reset_voice_prompt(ep: &BackendEndpoint) -> Result<PromptTemplateResponse, String> {
    let url = format!("{}/v1/prompts/voice/reset", ep.url);
    Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("reset voice prompt failed: {e}"))?
        .json::<PromptTemplateResponse>()
        .await
        .map_err(|e| format!("parse voice prompt failed: {e}"))
}

pub async fn test_voice_prompt(
    ep: &BackendEndpoint,
    transcript: String,
    draft_body: Option<String>,
) -> Result<PromptTestResponse, String> {
    let url = format!("{}/v1/prompts/voice/test", ep.url);
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({
            "transcript": transcript,
            "draft_body": draft_body,
        }))
        .send()
        .await
        .map_err(|e| format!("test voice prompt failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(extract_error(&text));
    }
    serde_json::from_str::<PromptTestResponse>(&text).map_err(|e| {
        format!(
            "parse voice prompt test failed: {e} — raw: {}",
            &text[..text.len().min(200)]
        )
    })
}

// ── History ───────────────────────────────────────────────────────────────────

pub async fn get_history(ep: &BackendEndpoint, limit: i64) -> Result<Vec<Recording>, String> {
    let url = format!("{}/v1/history?limit={limit}", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("get history failed: {e}"))?
        .json::<Vec<Recording>>()
        .await
        .map_err(|e| format!("parse history failed: {e}"))
}

// ── Cloud auth (calls the cloud control plane directly) ───────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudAccount {
    pub id: String,
    pub email: String,
    pub license_tier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudAuthResponse {
    pub token: String,
    pub account: CloudAccount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudStatus {
    pub connected: bool,
    pub license_tier: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseStatus {
    pub connected: bool,
    pub license_tier: String,
    pub email: Option<String>,
    pub server_url: Option<String>,
    pub org_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLiveConfig {
    pub enabled: bool,
    pub connected: bool,
    pub server_url: Option<String>,
    pub runtime_ws_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeNotificationConfig {
    pub connected: bool,
    pub server_url: Option<String>,
    pub notifications_ws_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLiveLatency {
    pub stt: i64,
    pub polish: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLiveResultUpload {
    pub client_run_id: String,
    pub transcript: String,
    pub output: String,
    pub model_used: String,
    pub latency_ms: RuntimeLiveLatency,
}

/// POST /v1/auth/signup on the cloud control plane.
pub async fn cloud_signup(
    cloud_url: &str,
    email: &str,
    password: &str,
) -> Result<CloudAuthResponse, String> {
    let url = format!("{}/v1/auth/signup", cloud_url.trim_end_matches('/'));
    let body = serde_json::json!({ "email": email, "password": password });
    let resp = Client::new()
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("cloud signup failed: {e}"))?;

    if !resp.status().is_success() {
        let msg = resp.text().await.unwrap_or_default();
        return Err(format!("signup error: {}", extract_error(&msg)));
    }
    resp.json::<CloudAuthResponse>()
        .await
        .map_err(|e| format!("parse signup response: {e}"))
}

/// POST /v1/auth/login on the cloud control plane.
pub async fn cloud_login(
    cloud_url: &str,
    email: &str,
    password: &str,
) -> Result<CloudAuthResponse, String> {
    let url = format!("{}/v1/auth/login", cloud_url.trim_end_matches('/'));
    let body = serde_json::json!({ "email": email, "password": password });
    let resp = Client::new()
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("cloud login failed: {e}"))?;

    if !resp.status().is_success() {
        let msg = resp.text().await.unwrap_or_default();
        return Err(format!("login error: {}", extract_error(&msg)));
    }
    resp.json::<CloudAuthResponse>()
        .await
        .map_err(|e| format!("parse login response: {e}"))
}

/// PUT /v1/cloud/token — persist cloud token in the local backend's SQLite.
pub async fn store_cloud_token(
    ep: &BackendEndpoint,
    token: &str,
    tier: &str,
) -> Result<(), String> {
    let url = format!("{}/v1/cloud/token", ep.url);
    let body = serde_json::json!({ "token": token, "license_tier": tier });
    let status = Client::new()
        .put(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("store token failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("store token error: {status}"))
    }
}

/// PUT /v1/cloud/token with email — used by enterprise auth to store identity.
pub async fn store_enterprise_token(
    ep: &BackendEndpoint,
    token: &str,
    tier: &str,
    email: &str,
    server_url: &str,
    org_name: Option<&str>,
) -> Result<(), String> {
    let url = format!("{}/v1/cloud/token", ep.url);
    let body = serde_json::json!({
        "token": token,
        "license_tier": tier,
        "email": email,
        "server_url": server_url,
        "org_name": org_name,
    });
    let status = Client::new()
        .put(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("store enterprise token failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("store enterprise token error: {status}"))
    }
}

/// DELETE /v1/cloud/token — clear cloud token from local backend.
pub async fn clear_cloud_token(ep: &BackendEndpoint) -> Result<(), String> {
    let url = format!("{}/v1/cloud/token", ep.url);
    let status = Client::new()
        .delete(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("clear token failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("clear token error: {status}"))
    }
}

/// GET /v1/cloud/status — read current cloud connection status from local backend.
pub async fn get_cloud_status(ep: &BackendEndpoint) -> Result<CloudStatus, String> {
    let url = format!("{}/v1/cloud/status", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("cloud status failed: {e}"))?
        .json::<CloudStatus>()
        .await
        .map_err(|e| format!("parse cloud status: {e}"))
}

/// GET /v1/enterprise/status — read workspace connection from local backend.
pub async fn get_enterprise_status(ep: &BackendEndpoint) -> Result<EnterpriseStatus, String> {
    let url = format!("{}/v1/enterprise/status", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("enterprise status failed: {e}"))?
        .json::<EnterpriseStatus>()
        .await
        .map_err(|e| format!("parse enterprise status: {e}"))
}

pub async fn get_runtime_live_config(ep: &BackendEndpoint) -> Result<RuntimeLiveConfig, String> {
    let url = format!("{}/v1/runtime/live/config", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("runtime live config failed: {e}"))?
        .json::<RuntimeLiveConfig>()
        .await
        .map_err(|e| format!("parse runtime live config: {e}"))
}

pub async fn get_runtime_notification_config(
    ep: &BackendEndpoint,
) -> Result<RuntimeNotificationConfig, String> {
    let url = format!("{}/v1/runtime/notifications/config", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("runtime notification config failed: {e}"))?
        .json::<RuntimeNotificationConfig>()
        .await
        .map_err(|e| format!("parse runtime notification config: {e}"))
}

pub async fn cache_runtime_live_result(
    ep: &BackendEndpoint,
    result: RuntimeLiveResultUpload,
) -> Result<(), String> {
    let url = format!("{}/v1/runtime/live/result", ep.url);
    let status = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&result)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("cache runtime live result failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("cache runtime live result error: {status}"))
    }
}

fn extract_error(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| body[..body.len().min(200)].to_string())
}

// ── Edit feedback ─────────────────────────────────────────────────────────────

pub async fn submit_feedback(
    ep: &BackendEndpoint,
    recording_id: &str,
    user_kept: &str,
    target_app: Option<&str>,
) -> Result<(), String> {
    let url = format!("{}/v1/edit-feedback", ep.url);
    let body = serde_json::json!({
        "recording_id": recording_id,
        "user_kept":    user_kept,
        "target_app":   target_app,
    });

    let status = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("submit feedback failed: {e}"))?
        .status();

    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("edit-feedback error: {status}"))
    }
}

// ── Pending edits ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEdit {
    pub id: String,
    pub recording_id: Option<String>,
    pub ai_output: String,
    pub user_kept: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEditsResponse {
    pub edits: Vec<PendingEdit>,
    pub total: i64,
}

/// Four-way edit classifier response.
///
/// `class` is one of `STT_ERROR | POLISH_ERROR | USER_REPHRASE | USER_REWRITE`.
/// `learned` indicates whether any artifact was written (vocabulary entry,
/// post-STT replacement, or word correction).  `notify` is the desktop's
/// notification gate.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClassifyEditResponse {
    pub class: String,
    pub reason: String,
    pub pending_id: Option<String>,
    #[serde(default)]
    pub learned: bool,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub promoted_count: usize,
    #[serde(default)]
    pub is_repeat: bool,
    /// Flat correct_form values that were just promoted to vocabulary.
    /// Driven by the toast event the desktop emits to the frontend.
    #[serde(default)]
    pub promoted_terms: Vec<String>,
    /// Email addresses saved to local deterministic email memory.
    #[serde(default)]
    pub learned_emails: Vec<String>,
    /// Terms recorded into the pending-promotions queue but not yet promoted
    /// (k-threshold not met). The desktop surfaces these as a soft "noticed"
    /// toast so the user knows the system saw the correction.
    #[serde(default)]
    pub queued_terms: Vec<QueuedTermResponse>,
    /// Ambiguous terms where the classifier can't decide — needs user confirmation.
    #[serde(default)]
    pub ambiguous_terms: Vec<AmbiguousTermResponse>,
    /// Corrections the system keeps making wrong — needs user to confirm blocking.
    #[serde(default)]
    pub negative_terms: Vec<NegativeTermResponse>,
    /// Changes the user should review before learning.
    #[serde(default)]
    pub review_candidates: Vec<ReviewCandidateResponse>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AmbiguousTermResponse {
    pub original: String,
    pub corrected: String,
    pub context: String,
    pub recording_id: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct NegativeTermResponse {
    pub term: String,
    pub wrong_replacement: String,
    pub correction_count: i64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct QueuedTermResponse {
    pub term: String,
    pub sighting_count: i64,
    pub k: i64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ReviewCandidateResponse {
    pub original: String,
    pub corrected: String,
    pub term_type: String,
    pub learnable: bool,
    pub tag: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ConfirmBatchResponse {
    pub learned_count: usize,
    pub learned_terms: Vec<String>,
    #[serde(default)]
    pub blocked_count: usize,
    #[serde(default)]
    pub server_owned: bool,
}

pub async fn confirm_batch(
    ep: &BackendEndpoint,
    items: &[(String, String)],
    recording_id: Option<&str>,
) -> Result<ConfirmBatchResponse, String> {
    let url = format!("{}/v1/confirm-batch", ep.url);
    let items_json: Vec<serde_json::Value> = items
        .iter()
        .map(|(orig, corr)| serde_json::json!({ "original": orig, "corrected": corr }))
        .collect();
    let body = serde_json::json!({
        "items": items_json,
        "recording_id": recording_id,
    });
    Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("confirm batch failed: {e}"))?
        .json::<ConfirmBatchResponse>()
        .await
        .map_err(|e| format!("parse confirm batch: {e}"))
}

/// Classify an edit using the four-way classifier.
///
/// Sends (recording_id, ai_output, user_kept) to the backend, which looks up
/// the original transcript and calls Groq.  Promotion happens server-side:
/// STT_ERROR auto-promotes on first sighting; POLISH_ERROR promotes only on
/// repeat; REPHRASE/REWRITE do nothing.
/// Capture-error metadata.  Sent alongside the edit so the backend's
/// CAPTURE_ERROR pre-filter can cheaply reject obvious bad signals
/// (app-switch, paste-on-top, stale capture) before any pipeline cost.
#[derive(Debug, Default, Clone, Copy)]
pub struct CaptureMeta {
    pub time_since_paste_ms: u64,
    pub app_switched: bool,
    pub matches_clipboard: bool,
}

pub async fn classify_edit(
    ep: &BackendEndpoint,
    recording_id: &str,
    ai_output: &str,
    user_kept: &str,
    capture_method: &str,
    capture_meta: CaptureMeta,
) -> Result<ClassifyEditResponse, String> {
    let url = format!("{}/v1/classify-edit", ep.url);
    let body = serde_json::json!({
        "recording_id":        recording_id,
        "ai_output":           ai_output,
        "user_kept":           user_kept,
        "capture_method":      capture_method,
        "time_since_paste_ms": capture_meta.time_since_paste_ms,
        "app_switched":        capture_meta.app_switched,
        "matches_clipboard":   capture_meta.matches_clipboard,
    });
    Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("classify edit failed: {e}"))?
        .json::<ClassifyEditResponse>()
        .await
        .map_err(|e| format!("parse classify response: {e}"))
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RetrainStatus {
    pub scheduled: bool,
    pub running: bool,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub success: bool,
}

pub async fn get_retrain_status(ep: &BackendEndpoint) -> Result<RetrainStatus, String> {
    let url = format!("{}/v1/retrain-status", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| format!("retrain status failed: {e}"))?
        .json::<RetrainStatus>()
        .await
        .map_err(|e| format!("parse retrain status: {e}"))
}

pub async fn get_stt_bias(ep: &BackendEndpoint) -> Result<BiasPackage, String> {
    let url = format!("{}/v1/stt/bias", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_millis(800))
        .send()
        .await
        .map_err(|e| format!("get stt bias failed: {e}"))?
        .json::<BiasPackage>()
        .await
        .map_err(|e| format!("parse stt bias: {e}"))
}

// ── Vocabulary management API (settings UI) ─────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VocabRow {
    pub term: String,
    pub weight: f64,
    pub use_count: i64,
    pub last_used: i64,
    pub source: String,
    #[serde(default)]
    pub meaning: Option<String>,
    #[serde(default)]
    pub term_type: Option<String>,
    #[serde(default)]
    pub example_context: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VocabListResponse {
    pub terms: Vec<VocabRow>,
    pub total: i64,
}

/// Full vocab list with metadata, for the management view.
pub async fn list_vocabulary(ep: &BackendEndpoint) -> Result<VocabListResponse, String> {
    let url = format!("{}/v1/vocabulary", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("list vocab failed: {e}"))?
        .json::<VocabListResponse>()
        .await
        .map_err(|e| format!("parse vocab list: {e}"))
}

pub async fn patch_vocabulary_term(
    ep: &BackendEndpoint,
    term: &str,
    meaning: Option<&str>,
    term_type: Option<&str>,
    example_context: Option<&str>,
) -> Result<(), String> {
    let encoded_term: String = term
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect();
    let url = format!("{}/v1/vocabulary/{}", ep.url, encoded_term);
    let mut body = serde_json::Map::new();
    if let Some(m) = meaning {
        body.insert("meaning".into(), serde_json::Value::String(m.to_string()));
    }
    if let Some(t) = term_type {
        body.insert("term_type".into(), serde_json::Value::String(t.to_string()));
    }
    if let Some(c) = example_context {
        body.insert(
            "example_context".into(),
            serde_json::Value::String(c.to_string()),
        );
    }
    Client::new()
        .patch(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .map_err(|e| format!("patch vocab failed: {e}"))?;
    Ok(())
}

/// Manually add a term (source = "manual", weight 1.5).
pub async fn add_vocabulary_term(ep: &BackendEndpoint, term: &str) -> Result<(), String> {
    let url = format!("{}/v1/vocabulary", ep.url);
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "term": term }))
        .send()
        .await
        .map_err(|e| format!("add vocab failed: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(format!("add vocab error {status}: {body}"))
    }
}

/// Wipe all learning data (vocab, corrections, STT aliases, embeddings).
pub async fn reset_all_vocabulary(ep: &BackendEndpoint) -> Result<(), String> {
    let url = format!("{}/v1/vocabulary/all", ep.url);
    let status = Client::new()
        .delete(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("reset vocab failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("reset vocab error {status}"))
    }
}

/// Hard-delete a single vocab term.
pub async fn delete_vocabulary_term(ep: &BackendEndpoint, term: &str) -> Result<(), String> {
    let encoded = urlencoding_encode(term);
    let url = format!("{}/v1/vocabulary/{}", ep.url, encoded);
    let status = Client::new()
        .delete(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("delete vocab failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("delete vocab error {status}"))
    }
}

/// Toggle starred status — returns the new starred state.
pub async fn star_vocabulary_term(ep: &BackendEndpoint, term: &str) -> Result<bool, String> {
    let encoded = urlencoding_encode(term);
    let url = format!("{}/v1/vocabulary/{}/star", ep.url, encoded);
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("star vocab failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse star response: {e}"))?;
    Ok(resp["starred"].as_bool().unwrap_or(false))
}

/// Send an invite email via the backend (Resend under the hood).
///
/// Returns:
///   Ok(true)                              — sent server-side
///   Err("email_not_configured")           — backend has no RESEND_API_KEY
///                                           caller should fall back to mailto
///   Err("...")                            — any other failure (network, 5xx)
pub async fn send_invite_email(ep: &BackendEndpoint, to: &str) -> Result<bool, String> {
    let url = format!("{}/v1/invite", ep.url);
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "to": to }))
        .send()
        .await
        .map_err(|e| format!("invite request failed: {e}"))?;

    let status = resp.status();
    if status.is_success() {
        return Ok(true);
    }

    // Try to parse error body so the frontend can branch on the reason.
    let body = resp.text().await.unwrap_or_default();
    if body.contains("email_not_configured") {
        return Err("email_not_configured".into());
    }
    Err(format!("invite send error {status}: {body}"))
}

/// Minimal RFC-3986 path-segment encoder (Tauri-side).  Same conservative
/// rules as the backend's keyterm encoder so server-side parsing matches.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", b);
            }
        }
    }
    out
}

pub async fn get_pending_edits(ep: &BackendEndpoint) -> Result<PendingEditsResponse, String> {
    let url = format!("{}/v1/pending-edits", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("get pending edits failed: {e}"))?
        .json::<PendingEditsResponse>()
        .await
        .map_err(|e| format!("parse pending edits: {e}"))
}

pub async fn resolve_pending_edit(
    ep: &BackendEndpoint,
    id: &str,
    action: &str, // "approve" | "skip"
) -> Result<(), String> {
    let url = format!("{}/v1/pending-edits/{id}/resolve", ep.url);
    let status = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "action": action }))
        .send()
        .await
        .map_err(|e| format!("resolve pending edit failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("resolve error: {status}"))
    }
}

pub async fn dismiss_pending_edit(ep: &BackendEndpoint, id: &str) -> Result<(), String> {
    let url = format!("{}/v1/pending-edits/{id}/dismiss", ep.url);
    let status = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("dismiss pending edit failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("dismiss error: {status}"))
    }
}

/// Hard-delete a single recording (SQLite row + WAV file).
pub async fn delete_recording(ep: &BackendEndpoint, id: &str) -> Result<(), String> {
    let url = format!("{}/v1/recordings/{id}", ep.url);
    let status = Client::new()
        .delete(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("delete recording failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("delete error: {status}"))
    }
}

/// Return the full URL (with inline bearer token) to stream a recording's WAV.
/// Used by the frontend to construct an <audio> src via fetch+blob.
pub fn recording_audio_url(ep: &BackendEndpoint, id: &str) -> String {
    format!("{}/v1/recordings/{id}/audio", ep.url)
}

/// Fetch a recording's WAV bytes from the local backend using native reqwest.
///
/// The frontend previously fetched this URL directly from the WKWebView, which
/// is fragile because it needs CORS + an Authorization header + media playback
/// to all line up inside the webview. Keeping the authenticated fetch in Tauri
/// makes play/download buttons independent of browser fetch behavior.
pub async fn recording_audio_bytes(ep: &BackendEndpoint, id: &str) -> Result<Vec<u8>, String> {
    let url = recording_audio_url(ep, id);
    let res = Client::new()
        .get(&url)
        .bearer_auth(&ep.secret)
        .send()
        .await
        .map_err(|e| format!("audio fetch failed: {e}"))?;

    let status = res.status();
    if !status.is_success() {
        return Err(format!("audio fetch error: {status}"));
    }

    res.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("audio read failed: {e}"))
}
