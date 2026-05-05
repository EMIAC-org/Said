//! Async HTTP client for the local polish-backend daemon.
//!
//! All functions take a `&BackendEndpoint` (url + secret).
//! They never interact with the child process — only the BackendState owns that.

use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};
use said_core::deepgram::{BiasPackage, TranscriptMeta};

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
    // API keys (stored in SQLite; None if not set yet)
    #[serde(default)]
    pub deepgram_api_key: Option<String>,
    #[serde(default)]
    pub gemini_api_key: Option<String>,
    #[serde(default)]
    pub gateway_api_key: Option<String>,
    #[serde(default)]
    pub groq_api_key: Option<String>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex"
    #[serde(default = "default_llm_provider")]
    pub llm_provider: String,
}

fn default_llm_provider() -> String {
    "gateway".to_string()
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
    // API keys — Some(value) = set; None = don't touch (field omitted from JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepgram_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<String>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
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
    },
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
    pre_transcript: Option<String>,
    pre_transcript_meta: Option<TranscriptMeta>,
    repair_mode: Option<String>,
    on_event: F,
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
        return Err(format!(
            "voice/polish error {status}: {}",
            &body[..body.len().min(300)]
        ));
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

/// Stream polish events when the desktop already has a final Deepgram transcript.
/// This avoids the multipart WAV upload on the latency-critical path; callers can
/// attach the WAV to the completed recording later with `upload_recording_audio`.
pub async fn stream_voice_polish_transcript<F>(
    ep: &BackendEndpoint,
    transcript: String,
    target_app: Option<String>,
    pre_transcript_meta: Option<TranscriptMeta>,
    on_event: F,
) -> Result<PolishDone, String>
where
    F: FnMut(PolishEvent),
{
    let url = format!("{}/v1/voice/polish-transcript", ep.url);
    let client = Client::new();
    let body = serde_json::json!({
        "transcript": transcript,
        "target_app": target_app,
        "pre_transcript_meta": pre_transcript_meta,
    });

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .header("Accept", "text/event-stream")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("voice polish transcript request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "voice/polish-transcript error {status}: {}",
            &body[..body.len().min(300)]
        ));
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

pub async fn upload_recording_audio(
    ep: &BackendEndpoint,
    recording_id: &str,
    wav_data: Vec<u8>,
) -> Result<(), String> {
    if wav_data.is_empty() {
        return Ok(());
    }

    let url = format!("{}/v1/recordings/{}/audio", ep.url, recording_id);
    let client = Client::new();
    let form = reqwest::multipart::Form::new().part(
        "audio",
        reqwest::multipart::Part::bytes(wav_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .map_err(|e| format!("mime error: {e}"))?,
    );

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("recording audio upload failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "recording audio upload error {status}: {}",
            &body[..body.len().min(300)]
        ));
    }

    Ok(())
}

/// Stream polish events for plain text.
#[allow(dead_code)]
pub async fn stream_text_polish<F>(
    ep: &BackendEndpoint,
    text: String,
    target_app: Option<String>,
    tone_override: Option<String>,
    on_event: F,
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
        return Err(format!(
            "text/polish error {status}: {}",
            &body[..body.len().min(300)]
        ));
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
    tracing::info!("[patch_prefs] → PATCH {url}  body={body}");
    let resp = Client::new()
        .patch(&url)
        .header("Authorization", ep.bearer())
        .json(&update)
        .send()
        .await
        .map_err(|e| format!("patch prefs failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    tracing::info!("[patch_prefs] ← {status}  body={text}");
    serde_json::from_str::<Preferences>(&text).map_err(|e| {
        format!(
            "parse prefs failed: {e} — raw: {}",
            &text[..text.len().min(200)]
        )
    })
}

/// Fetch the user's correction keyterms (right-hand words from word_corrections table).
/// Used to boost Deepgram STT recognition for words the user frequently corrects.
pub async fn get_correction_keyterms(ep: &BackendEndpoint) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Resp {
        keyterms: Vec<String>,
    }
    let url = format!("{}/v1/corrections", ep.url);
    let Ok(resp) = Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
    else {
        return vec![];
    };
    resp.json::<Resp>()
        .await
        .map(|b| b.keyterms)
        .unwrap_or_default()
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

/// GET /v1/license/check on the cloud control plane.
#[allow(dead_code)]
pub async fn cloud_license_check(
    cloud_url: &str,
    token: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/v1/license/check", cloud_url.trim_end_matches('/'));
    Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("license check failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse license response: {e}"))
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

// ── OpenAI OAuth ──────────────────────────────────────────────────────────────

pub async fn get_openai_status(ep: &BackendEndpoint) -> Result<serde_json::Value, String> {
    let url = format!("{}/v1/openai-oauth/status", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("openai status failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse openai status: {e}"))
}

pub async fn initiate_openai_oauth(ep: &BackendEndpoint) -> Result<serde_json::Value, String> {
    let url = format!("{}/v1/openai-oauth/initiate", ep.url);
    Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .header("Content-Length", "0")
        .send()
        .await
        .map_err(|e| format!("openai initiate failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse openai initiate: {e}"))
}

pub async fn disconnect_openai(ep: &BackendEndpoint) -> Result<(), String> {
    let url = format!("{}/v1/openai-oauth/disconnect", ep.url);
    let status = Client::new()
        .delete(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("openai disconnect failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("openai disconnect error: {status}"))
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

/// Store a detected edit for user review (called right after detection, before notifying).
/// NOTE: In normal flow, `classify_edit` is used instead (which auto-stores if should_learn).
/// This is kept for manual/direct storage if needed.
#[allow(dead_code)]
pub async fn store_pending_edit(
    ep: &BackendEndpoint,
    recording_id: Option<&str>,
    ai_output: &str,
    user_kept: &str,
) -> Result<String, String> {
    let url = format!("{}/v1/pending-edits", ep.url);
    let body = serde_json::json!({
        "recording_id": recording_id,
        "ai_output":    ai_output,
        "user_kept":    user_kept,
    });
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("store pending edit failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse pending edit response: {e}"))?;
    resp["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "no id in response".into())
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
    /// Terms recorded into the pending-promotions queue but not yet promoted
    /// (k-threshold not met). The desktop surfaces these as a soft "noticed"
    /// toast so the user knows the system saw the correction.
    #[serde(default)]
    pub queued_terms: Vec<String>,
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

/// Light-weight fetch of just the personal vocabulary terms — used by the
/// dictation hot path to bias Deepgram's WS at recording start.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct VocabTermsResponse {
    pub terms: Vec<String>,
}

pub async fn get_vocabulary_terms(ep: &BackendEndpoint) -> Result<Vec<String>, String> {
    get_vocabulary_terms_for_language(ep, None).await
}

/// Same as `get_vocabulary_terms` but scoped to a language bucket.  Pass the
/// user's current `output_language` so the keyterms slate doesn't leak
/// Devanagari into English mode (or vice versa).  `None` returns the
/// language-agnostic top-N (legacy behaviour).
pub async fn get_vocabulary_terms_for_language(
    ep: &BackendEndpoint,
    language: Option<&str>,
) -> Result<Vec<String>, String> {
    let url = match language {
        Some(lang) if !lang.trim().is_empty() => format!(
            "{}/v1/vocabulary/terms?language={}",
            ep.url,
            urlencoding_encode(lang),
        ),
        _ => format!("{}/v1/vocabulary/terms", ep.url),
    };
    let resp = Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_millis(500)) // hot path — fail fast
        .send()
        .await
        .map_err(|e| format!("get vocab terms failed: {e}"))?
        .json::<VocabTermsResponse>()
        .await
        .map_err(|e| format!("parse vocab terms: {e}"))?;
    Ok(resp.terms)
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
    pub source: String, // "auto" | "manual" | "starred"
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
