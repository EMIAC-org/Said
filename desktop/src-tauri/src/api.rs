//! Async HTTP client for the local airnote-backend daemon.
//!
//! All functions take a `&BackendEndpoint` (url + secret).
//! They never interact with the child process — only the BackendState owns that.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{SinkExt, StreamExt};
use reqwest::Client;
use said_core::{text::Utf8LineBuffer, transcript::TranscriptMeta};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message as WsMessage,
        client::IntoClientRequest,
        http::{HeaderValue, header},
    },
};
use tracing::{debug, info, warn};

use crate::backend::BackendEndpoint;

type LocalPolishWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const LOCAL_POLISH_TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
const LOCAL_POLISH_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const LOCAL_POLISH_MAX_RECONNECTS: u8 = 3;

struct LocalPolishWsConnection {
    endpoint_url: String,
    secret: String,
    socket: LocalPolishWs,
}

/// One warm, authenticated WebSocket to the local AirNote backend. The mutex
/// intentionally serializes dictation runs: the desktop state machine permits
/// one in-flight dictation, and serial ownership prevents token interleaving.
#[derive(Clone, Default)]
pub struct PersistentPolishSocket {
    connection: Arc<Mutex<Option<LocalPolishWsConnection>>>,
}

impl PersistentPolishSocket {
    pub fn new() -> Self {
        Self::default()
    }

    /// Establish the local connection early so the first dictation does not pay
    /// the TCP/WebSocket handshake cost. Failure is intentionally non-fatal: the
    /// next run reconnects with its normal bounded recovery path.
    pub async fn prewarm(&self, ep: &BackendEndpoint) {
        let mut slot = self.connection.lock().await;
        let _ = ensure_local_polish_connection(&mut slot, ep).await;
    }

    /// Keep the dormant loopback connection warm. Runs never share a socket, so
    /// this waits for any active dictation instead of competing with its stream.
    /// A failed heartbeat simply drops the stale socket; the next dictation will
    /// establish a fresh one through its bounded reconnect path.
    pub async fn keep_alive(&self, ep: &BackendEndpoint) {
        let mut slot = self.connection.lock().await;
        if ensure_local_polish_connection(&mut slot, ep).await.is_err() {
            *slot = None;
            return;
        }
        let Some(connection) = slot.as_mut() else {
            return;
        };
        if connection
            .socket
            .send(WsMessage::Text(
                r#"{"type":"ping","protocol_version":1}"#.into(),
            ))
            .await
            .is_err()
        {
            *slot = None;
        }
    }
}

/// The persistent protocol is deliberately an *owned-sidecar*, loopback-only
/// optimization. An explicitly configured backend uses legacy HTTP/SSE even
/// when that backend happens to listen on a loopback address.
pub fn supports_local_polish_websocket(ep: &BackendEndpoint, using_external_backend: bool) -> bool {
    !using_external_backend
        && reqwest::Url::parse(&ep.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1"))
}

fn local_polish_ws_url(ep: &BackendEndpoint) -> Result<String, String> {
    let scheme = if let Some(rest) = ep.url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = ep.url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        return Err("backend endpoint must use http or https".to_string());
    };
    Ok(format!(
        "{}/v1/voice/polish/ws",
        scheme.trim_end_matches('/')
    ))
}

async fn open_local_polish_connection(
    ep: &BackendEndpoint,
) -> Result<LocalPolishWsConnection, String> {
    let url = local_polish_ws_url(ep)?;
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("build local polish WebSocket request: {e}"))?;
    let bearer = HeaderValue::from_str(&ep.bearer())
        .map_err(|e| format!("encode local polish authorization: {e}"))?;
    request.headers_mut().insert(header::AUTHORIZATION, bearer);
    let (socket, _) = connect_async(request)
        .await
        .map_err(|e| format!("connect local polish WebSocket: {e}"))?;
    Ok(LocalPolishWsConnection {
        endpoint_url: ep.url.clone(),
        secret: ep.secret.clone(),
        socket,
    })
}

async fn ensure_local_polish_connection(
    slot: &mut Option<LocalPolishWsConnection>,
    ep: &BackendEndpoint,
) -> Result<(), String> {
    let matches_endpoint = slot.as_ref().is_some_and(|connection| {
        connection.endpoint_url == ep.url && connection.secret == ep.secret
    });
    if !matches_endpoint {
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(open_local_polish_connection(ep).await?);
    }
    Ok(())
}

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
    pub gemini_api_key: Option<String>,
    #[serde(default)]
    pub gateway_api_key: Option<String>,
    #[serde(default)]
    pub groq_api_key: Option<String>,
    #[serde(default)]
    pub deepinfra_api_key: Option<String>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex"
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
    pub gemini_api_key: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepinfra_api_key: Option<Option<String>>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemTranscribeResponse {
    pub transcript: String,
    pub source: String,
    pub confidence: f64,
    pub word_count: usize,
    pub latency_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemSolveRequest {
    pub transcript: String,
    pub context_mode: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub project_context: Option<String>,
    pub screen_context: Option<String>,
    pub client_run_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemSolveResponse {
    pub run_id: String,
    pub output: String,
    pub model_used: String,
    pub prompt_version: String,
    pub latency_ms: ProblemSolveLatency,
    pub context_mode: String,
    pub project_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemSolveLatency {
    pub prompt: i64,
    pub model: i64,
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
        run_id: Option<String>,
        audio_id: Option<String>,
        error_code: Option<String>,
        retryable: Option<bool>,
        owned_by_airnote: Option<bool>,
        diagnostic: Option<String>,
    },
}

fn http_error_event(
    path: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> (String, Option<String>) {
    let preview = said_core::text::truncate_utf8(&body, 300);
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
        "gemini_api_key",
        "groq_api_key",
        "deepinfra_api_key",
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
/// `pre_transcript` is the local ASR result. The backend only polishes text.
pub async fn stream_voice_polish<F>(
    ep: &BackendEndpoint,
    wav_data: Vec<u8>,
    target_app: Option<String>,
    client_run_id: Option<String>,
    client_trace_json: Option<Value>,
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
    let request_start = std::time::Instant::now();
    let wav_bytes = wav_data.len();
    let pre_transcript_chars = pre_transcript
        .as_ref()
        .map(|t| t.chars().count())
        .unwrap_or(0);
    let pre_transcript_words = pre_transcript
        .as_ref()
        .map(|t| t.split_whitespace().count())
        .unwrap_or(0);
    let has_pre_transcript = pre_transcript.is_some();
    let has_pre_transcript_meta = pre_transcript_meta.is_some();
    let screen_context_chars = screen_context
        .as_ref()
        .map(|s| s.chars().take(500).count())
        .unwrap_or(0);
    let has_repair_mode = repair_mode.is_some();
    let has_target_app = target_app.is_some();
    let client_run_id_label = client_run_id.as_deref().unwrap_or("none").to_string();

    info!(
        "[api] voice/polish request start run_id={} wav_bytes={} pre_transcript_present={} pre_chars={} pre_words={} pre_meta={} message_polish={} repair_mode={} screen_context_chars={} target_app_present={}",
        client_run_id_label,
        wav_bytes,
        has_pre_transcript,
        pre_transcript_chars,
        pre_transcript_words,
        has_pre_transcript_meta,
        message_polish_mode,
        has_repair_mode,
        screen_context_chars,
        has_target_app,
    );

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
    if let Some(trace) = client_trace_json {
        form = form.text("client_trace_json", trace.to_string());
    }
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
    let response_headers_ms = request_start.elapsed().as_millis();
    info!(
        "[api] voice/polish response headers run_id={} status={} after={}ms pre_transcript_present={} wav_bytes={}",
        client_run_id_label,
        resp.status(),
        response_headers_ms,
        has_pre_transcript,
        wav_bytes,
    );

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let (message, error_code) = http_error_event("voice/polish", status, &body);
        let parsed = serde_json::from_str::<Value>(&body).ok();
        on_event(PolishEvent::Error {
            message: message.clone(),
            run_id: parsed
                .as_ref()
                .and_then(|v| v.get("run_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            audio_id: parsed
                .as_ref()
                .and_then(|v| v.get("audio_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            error_code,
            retryable: parsed
                .as_ref()
                .and_then(|v| v.get("retryable"))
                .and_then(Value::as_bool),
            owned_by_airnote: parsed
                .as_ref()
                .and_then(|v| v.get("owned_by_airnote"))
                .and_then(Value::as_bool),
            diagnostic: Some(body),
        });
        return Err(message);
    }

    consume_sse(resp.bytes_stream(), on_event).await
}

/// Stream a transcript-only polish run over the persistent localhost
/// WebSocket. The desktop has already completed STT, so this path never sends
/// the WAV to the polish backend.
///
/// If the local socket drops after `polish.start`, the same immutable run id is
/// resumed; it is never re-submitted as a second model request. The backend
/// remains the source of truth for whether that run was accepted or completed.
pub async fn stream_voice_polish_ws<F>(
    transport: &PersistentPolishSocket,
    ep: &BackendEndpoint,
    target_app: Option<String>,
    client_run_id: Option<String>,
    client_trace_json: Option<Value>,
    pre_transcript: String,
    pre_transcript_meta: TranscriptMeta,
    repair_mode: Option<String>,
    screen_context: Option<String>,
    message_polish_mode: bool,
    mut on_event: F,
) -> Result<PolishDone, String>
where
    F: FnMut(PolishEvent),
{
    let run_id = client_run_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let started = Instant::now();
    let total_timeout = LOCAL_POLISH_TOTAL_TIMEOUT;
    let idle_timeout = LOCAL_POLISH_IDLE_TIMEOUT;
    let mut reconnect_attempts = 0u8;
    let mut unknown_before_accept_retries = 0u8;
    let mut sent_start = false;
    let mut run_accepted = false;
    let mut last_seq = 0u64;

    loop {
        if started.elapsed() >= total_timeout {
            return Err("local polish run timed out before a terminal response".to_string());
        }

        let mut slot = transport.connection.lock().await;
        if let Err(error) = ensure_local_polish_connection(&mut slot, ep).await {
            drop(slot);
            reconnect_attempts = reconnect_attempts.saturating_add(1);
            if reconnect_attempts > LOCAL_POLISH_MAX_RECONNECTS {
                return Err(error);
            }
            tokio::time::sleep(Duration::from_millis(
                150 * 2u64.pow(reconnect_attempts as u32),
            ))
            .await;
            continue;
        }
        let socket = &mut slot.as_mut().expect("connection ensured").socket;
        let outbound = if sent_start {
            serde_json::json!({
                "type": "run.resume",
                "protocol_version": 1,
                "run_id": &run_id,
                "last_seq": last_seq,
            })
        } else {
            serde_json::json!({
                "type": "polish.start",
                "protocol_version": 1,
                "run_id": &run_id,
                "transcript": &pre_transcript,
                "target_app": target_app.clone(),
                "pre_transcript_meta": pre_transcript_meta.clone(),
                "repair_mode": repair_mode.clone(),
                "screen_context": screen_context.clone(),
                "message_polish_mode": message_polish_mode,
                "client_trace_json": client_trace_json.clone(),
            })
        };
        sent_start = true;
        if socket
            .send(WsMessage::Text(outbound.to_string()))
            .await
            .is_err()
        {
            *slot = None;
            drop(slot);
            reconnect_attempts = reconnect_attempts.saturating_add(1);
            if reconnect_attempts > LOCAL_POLISH_MAX_RECONNECTS {
                return Err("local polish connection closed while submitting run".to_string());
            }
            continue;
        }

        let mut reconnect = false;
        let mut retry_start_after_unknown = false;
        while started.elapsed() < total_timeout {
            let remaining = total_timeout.saturating_sub(started.elapsed());
            let wait_for = remaining.min(idle_timeout);
            let inbound = tokio::time::timeout(wait_for, socket.next()).await;
            let message = match inbound {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(_))) | Ok(None) | Err(_) => {
                    reconnect = true;
                    break;
                }
            };
            match message {
                WsMessage::Ping(payload)
                    if socket.send(WsMessage::Pong(payload.clone())).await.is_err() =>
                {
                    reconnect = true;
                    break;
                }
                WsMessage::Ping(_) => {}
                WsMessage::Close(_) => {
                    reconnect = true;
                    break;
                }
                WsMessage::Text(text) => {
                    let Ok(event) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if event
                        .get("run_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id != run_id)
                    {
                        continue;
                    }
                    if let Some(seq) = event.get("seq").and_then(Value::as_u64) {
                        if seq > 0 && seq <= last_seq {
                            continue;
                        }
                        last_seq = last_seq.max(seq);
                    }
                    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
                    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
                    match kind {
                        "polish.connected" | "pong" | "run.resumed" | "run.status" => {}
                        "run.accepted" => run_accepted = true,
                        "run.resync_required" => {
                            // The final done event remains authoritative and will
                            // reconcile any missed preview tokens.
                        }
                        "status" => {
                            run_accepted = true;
                            let phase = payload
                                .get("phase")
                                .and_then(Value::as_str)
                                .unwrap_or("polishing")
                                .to_string();
                            let transcript = payload
                                .get("transcript")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            on_event(PolishEvent::Status { phase, transcript });
                        }
                        "token" => {
                            run_accepted = true;
                            if let Some(token) = payload.get("token").and_then(Value::as_str) {
                                on_event(PolishEvent::Token {
                                    token: token.to_string(),
                                });
                            }
                        }
                        "done" => {
                            let done: PolishDone = serde_json::from_value(payload)
                                .map_err(|e| format!("invalid local polish completion: {e}"))?;
                            on_event(PolishEvent::Done(done.clone()));
                            return Ok(done);
                        }
                        "error" => {
                            let error_code = payload
                                .get("error_code")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            // A disconnect can occur between transmitting the
                            // first start frame and receiving its acknowledgement.
                            // If the reconnected backend proves it never saw the
                            // run, one same-ID re-submit is safe; after acceptance
                            // we *only* resume and never create a second request.
                            if !run_accepted
                                && sent_start
                                && error_code.as_deref() == Some("unknown_run")
                                && unknown_before_accept_retries == 0
                            {
                                unknown_before_accept_retries = 1;
                                sent_start = false;
                                retry_start_after_unknown = true;
                                break;
                            }
                            let message = payload
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("local polish failed")
                                .to_string();
                            on_event(PolishEvent::Error {
                                message: message.clone(),
                                run_id: event
                                    .get("run_id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                audio_id: payload
                                    .get("audio_id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                error_code,
                                retryable: payload.get("retryable").and_then(Value::as_bool),
                                owned_by_airnote: payload
                                    .get("owned_by_airnote")
                                    .and_then(Value::as_bool),
                                diagnostic: payload
                                    .get("diagnostic")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                            });
                            return Err(message);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if retry_start_after_unknown {
            drop(slot);
            continue;
        }
        if !reconnect {
            return Err("local polish run timed out before a terminal response".to_string());
        }
        *slot = None;
        drop(slot);
        reconnect_attempts = reconnect_attempts.saturating_add(1);
        if reconnect_attempts > LOCAL_POLISH_MAX_RECONNECTS {
            return Err("local polish connection closed before a terminal response".to_string());
        }
        tokio::time::sleep(Duration::from_millis(
            150 * 2u64.pow(reconnect_attempts as u32),
        ))
        .await;
    }
}

pub async fn patch_dictation_trace(
    ep: &BackendEndpoint,
    recording_id: &str,
    dictation_trace_json: Value,
) -> Result<(), String> {
    let url = format!(
        "{}/v1/observability/dictation/{}/trace",
        ep.url, recording_id
    );
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "dictation_trace_json": dictation_trace_json }))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("patch dictation trace failed: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("patch dictation trace HTTP {}", resp.status()))
    }
}

pub async fn transcribe_problem_audio(
    ep: &BackendEndpoint,
    wav_data: Vec<u8>,
    client_run_id: Option<String>,
    pre_transcript: Option<String>,
    pre_transcript_meta: Option<TranscriptMeta>,
) -> Result<ProblemTranscribeResponse, String> {
    let url = format!("{}/v1/problem/transcribe", ep.url);
    let client = Client::new();
    let mut form = reqwest::multipart::Form::new();
    if !wav_data.is_empty() {
        form = form.part(
            "audio",
            reqwest::multipart::Part::bytes(wav_data)
                .file_name("problem-recording.wav")
                .mime_str("audio/wav")
                .map_err(|e| format!("mime error: {e}"))?,
        );
    }
    if let Some(run_id) = client_run_id {
        form = form.text("client_run_id", run_id);
    }
    if let Some(transcript) = pre_transcript {
        form = form.text("pre_transcript", transcript);
    }
    if let Some(meta) = pre_transcript_meta {
        form = form.text(
            "pre_transcript_meta",
            serde_json::to_string(&meta).map_err(|e| format!("encode transcript meta: {e}"))?,
        );
    }

    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .multipart(form)
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .map_err(|e| format!("problem transcribe request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let (message, _error_code) = http_error_event("problem/transcribe", status, &body);
        return Err(message);
    }

    resp.json::<ProblemTranscribeResponse>()
        .await
        .map_err(|e| format!("problem transcribe response parse failed: {e}"))
}

pub async fn solve_problem(
    ep: &BackendEndpoint,
    req: ProblemSolveRequest,
) -> Result<ProblemSolveResponse, String> {
    let url = format!("{}/v1/problem/solve", ep.url);
    let client = Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&req)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("problem solve request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let (message, _error_code) = http_error_event("problem/solve", status, &body);
        return Err(message);
    }

    resp.json::<ProblemSolveResponse>()
        .await
        .map_err(|e| format!("problem solve response parse failed: {e}"))
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
            run_id: None,
            audio_id: None,
            error_code,
            retryable: None,
            owned_by_airnote: None,
            diagnostic: Some(body),
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
            said_core::text::truncate_utf8(&body, 300)
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
            said_core::text::truncate_utf8(&body, 300)
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
    let mut line_buffer = Utf8LineBuffer::default();
    let mut done_event: Option<PolishDone> = None;
    let mut last_error: Option<String> = None;
    // Track the most recently seen `event:` line so we can dispatch correctly
    let mut event_name = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;

        // HTTP chunks can split a multi-byte UTF-8 character. Decode only
        // after a complete SSE line has arrived.
        for raw_line in line_buffer
            .push(&chunk)
            .map_err(|e| format!("invalid UTF-8 in SSE stream: {e}"))?
        {
            let line = raw_line.trim().to_string();

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

            parse_and_dispatch(
                data,
                &event_name,
                &mut on_event,
                &mut done_event,
                &mut last_error,
            );
        }
    }

    done_event.ok_or_else(|| {
        last_error.unwrap_or_else(|| "SSE stream ended without a `done` event".into())
    })
}

fn parse_and_dispatch(
    data: &str,
    event_name: &str,
    on_event: &mut impl FnMut(PolishEvent),
    done_event: &mut Option<PolishDone>,
    last_error: &mut Option<String>,
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
                *last_error = Some(msg.to_string());
                on_event(parse_error_event(&val, msg));
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
                *last_error = Some(msg.to_string());
                on_event(parse_error_event(&val, msg));
            }
        }
    }
}

fn parse_error_event(val: &Value, msg: &str) -> PolishEvent {
    PolishEvent::Error {
        message: msg.to_string(),
        run_id: val
            .get("run_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        audio_id: val
            .get("audio_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        error_code: val
            .get("error_code")
            .and_then(Value::as_str)
            .map(str::to_string),
        retryable: val.get("retryable").and_then(Value::as_bool),
        owned_by_airnote: val.get("owned_by_airnote").and_then(Value::as_bool),
        diagnostic: val
            .get("diagnostic")
            .and_then(Value::as_str)
            .map(str::to_string),
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
            said_core::text::truncate_utf8(&text, 200)
        )
    })
}

// ── History ───────────────────────────────────────────────────────────────────

pub async fn get_history(
    ep: &BackendEndpoint,
    limit: i64,
    before: Option<i64>,
) -> Result<Vec<Recording>, String> {
    // `before` (ms) paginates older pages; the backend `/v1/history` already
    // supports it via list_recordings(pool, user, limit, before).
    let url = match before {
        Some(ms) => format!("{}/v1/history?limit={limit}&before={ms}", ep.url),
        None => format!("{}/v1/history?limit={limit}", ep.url),
    };
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

/// Per-app dictation usage from the backend (`/v1/history/apps`). The `app` field
/// is the raw target_app key; the desktop resolves its icon/name/category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUsage {
    pub app: String,
    pub count: i64,
    pub total_words: i64,
    pub last_used_ms: i64,
}

pub async fn get_app_usage(ep: &BackendEndpoint) -> Result<Vec<AppUsage>, String> {
    let url = format!("{}/v1/history/apps", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("get app usage failed: {e}"))?
        .json::<Vec<AppUsage>>()
        .await
        .map_err(|e| format!("parse app usage failed: {e}"))
}

/// Record a browser dictation's site to the LOCAL backend (`/v1/site-context`).
/// On-device only — never reaches the cloud runtime.
pub async fn record_site_context(
    ep: &BackendEndpoint,
    target_app: &str,
    host: &str,
) -> Result<(), String> {
    let url = format!("{}/v1/site-context", ep.url);
    Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "target_app": target_app, "host": host }))
        .send()
        .await
        .map_err(|e| format!("record site failed: {e}"))?;
    Ok(())
}

/// Per-site dictation usage (grouped by host) for the Insights "Sites" section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteUsage {
    pub host: String,
    pub target_app: String,
    pub count: i64,
    pub last_used_ms: i64,
}

pub async fn get_site_usage(ep: &BackendEndpoint) -> Result<Vec<SiteUsage>, String> {
    let url = format!("{}/v1/history/sites", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("get site usage failed: {e}"))?
        .json::<Vec<SiteUsage>>()
        .await
        .map_err(|e| format!("parse site usage failed: {e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceRun {
    pub run_id: String,
    pub audio_id: Option<String>,
    pub mode: String,
    pub target_app: Option<String>,
    pub status: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub retryable: bool,
    pub owned_by_airnote: bool,
    pub attempt_count: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatestFailedVoiceRunResponse {
    run: Option<VoiceRun>,
}

pub async fn latest_failed_voice_run(ep: &BackendEndpoint) -> Result<Option<VoiceRun>, String> {
    let url = format!("{}/v1/voice-runs/latest-failed", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("latest failed voice run failed: {e}"))?
        .json::<LatestFailedVoiceRunResponse>()
        .await
        .map(|res| res.run)
        .map_err(|e| format!("parse latest failed voice run failed: {e}"))
}

pub async fn mark_voice_run_failed(
    ep: &BackendEndpoint,
    run_id: &str,
    error_code: &str,
    message: &str,
    retryable: bool,
    owned_by_airnote: bool,
) -> Result<Option<VoiceRun>, String> {
    let url = format!("{}/v1/voice-runs/{run_id}/failed", ep.url);
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({
            "error_code": error_code,
            "message": message,
            "retryable": retryable,
            "owned_by_airnote": owned_by_airnote,
            "diagnostic": {
                "error_code": error_code,
                "message": message,
                "owned_by_airnote": owned_by_airnote,
            }
        }))
        .send()
        .await
        .map_err(|e| format!("mark voice run failed request failed: {e}"))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    latest_failed_voice_run(ep).await
}

pub async fn mark_voice_run_paste(
    ep: &BackendEndpoint,
    run_id: &str,
    paste_success: bool,
) -> Result<(), String> {
    let url = format!("{}/v1/voice-runs/{run_id}/paste", ep.url);
    let resp = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&serde_json::json!({ "paste_success": paste_success }))
        .send()
        .await
        .map_err(|e| format!("mark voice paste request failed: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("mark voice paste failed: {}", resp.status()))
    }
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
    pub active_org_id: Option<String>,
    pub personal_mode: Option<bool>,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMembership {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceListResponse {
    pub orgs: Vec<OrgMembership>,
    pub active_org_id: Option<String>,
    pub personal_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeNotificationConfig {
    pub notifications_ws_url: Option<String>,
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
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .map_err(|e| format!("enterprise status failed: {e}"))?
        .json::<EnterpriseStatus>()
        .await
        .map_err(|e| format!("parse enterprise status: {e}"))
}

pub async fn set_local_active_org(
    ep: &BackendEndpoint,
    active_org_id: Option<&str>,
) -> Result<(), String> {
    let url = format!("{}/v1/cloud/active-org", ep.url);
    let body = serde_json::json!({ "active_org_id": active_org_id });
    let status = Client::new()
        .put(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("set active org failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("set active org error: {status}"))
    }
}

pub async fn list_workspaces(
    server_url: &str,
    token: &str,
    active_org_id: Option<&str>,
) -> Result<WorkspaceListResponse, String> {
    let url = format!("{}/v1/orgs", server_url.trim_end_matches('/'));
    let mut req = Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(org_id) = active_org_id.filter(|s| !s.trim().is_empty()) {
        req = req.header("x-airnote-org-id", org_id);
    }
    req.send()
        .await
        .map_err(|e| format!("list orgs failed: {e}"))?
        .json::<WorkspaceListResponse>()
        .await
        .map_err(|e| format!("parse org list: {e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileRunStats {
    pub run_count: i64,
    pub skipped_count: i64,
    pub last_run_at: Option<String>,
    pub last_run_outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnowledgeBase {
    pub background: Option<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub focus_areas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketInsight {
    pub bucket_key: String,
    #[serde(default)]
    pub style: Vec<String>,
    #[serde(default)]
    pub speech_patterns: Vec<String>,
    pub version: i64,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileInsights {
    pub run_stats: ProfileRunStats,
    pub knowledge: KnowledgeBase,
    #[serde(default)]
    pub buckets: Vec<BucketInsight>,
}

/// GET /v1/runtime/profile/insights — what the cloud profiling brain has learned.
pub async fn get_profile_insights(
    server_url: &str,
    token: &str,
    active_org_id: Option<&str>,
) -> Result<ProfileInsights, String> {
    let url = format!(
        "{}/v1/runtime/profile/insights",
        server_url.trim_end_matches('/')
    );
    let mut req = Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(org_id) = active_org_id.filter(|s| !s.trim().is_empty()) {
        req = req.header("x-airnote-org-id", org_id);
    }
    req.send()
        .await
        .map_err(|e| format!("profile insights failed: {e}"))?
        .json::<ProfileInsights>()
        .await
        .map_err(|e| format!("parse profile insights: {e}"))
}

/// GET /v1/runtime/profile/buckets — apps the user dictates into, grouped by bucket.
pub async fn get_app_buckets(
    server_url: &str,
    token: &str,
    active_org_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    let url = format!(
        "{}/v1/runtime/profile/buckets",
        server_url.trim_end_matches('/')
    );
    let mut req = Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(org_id) = active_org_id.filter(|s| !s.trim().is_empty()) {
        req = req.header("x-airnote-org-id", org_id);
    }
    req.send()
        .await
        .map_err(|e| format!("app buckets failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse app buckets: {e}"))
}

/// POST /v1/runtime/profile/buckets/override — re-file an app into a bucket (user override).
pub async fn set_app_bucket(
    server_url: &str,
    token: &str,
    active_org_id: Option<&str>,
    app_key: &str,
    bucket_key: &str,
) -> Result<(), String> {
    let url = format!(
        "{}/v1/runtime/profile/buckets/override",
        server_url.trim_end_matches('/')
    );
    let mut req = Client::new()
        .post(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10))
        .json(&serde_json::json!({ "app_key": app_key, "bucket_key": bucket_key }));
    if let Some(org_id) = active_org_id.filter(|s| !s.trim().is_empty()) {
        req = req.header("x-airnote-org-id", org_id);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("set app bucket failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("set app bucket failed: HTTP {}", resp.status()));
    }
    Ok(())
}

pub async fn activate_workspace_on_server(
    server_url: &str,
    token: &str,
    org_id: &str,
) -> Result<String, String> {
    let url = format!(
        "{}/v1/orgs/{}/activate",
        server_url.trim_end_matches('/'),
        org_id
    );
    let resp = Client::new()
        .post(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("activate org failed: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("activate org error: {}", extract_error(&body)));
    }
    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse activate response: {e}"))?;
    value
        .get("active_org_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "activate response missing active_org_id".to_string())
}

pub async fn deactivate_workspace_on_server(server_url: &str, token: &str) -> Result<(), String> {
    let url = format!("{}/v1/orgs/deactivate", server_url.trim_end_matches('/'));
    let resp = Client::new()
        .post(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("deactivate org failed: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(format!("deactivate org error: {}", extract_error(&body)))
    }
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

fn extract_error(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| said_core::text::truncate_utf8(&body, 200).to_string())
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
/// Classification only proposes review candidates. `learned` is retained for
/// wire compatibility and is set by the separate, user-confirmed batch flow.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClassifyEditResponse {
    pub class: String,
    pub reason: String,
    pub pending_id: Option<String>,
    #[serde(default)]
    pub review_session_id: Option<String>,
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
    /// Every deterministic change detected in the final owned text. This is
    /// intentionally broader than `review_candidates`, which contains only the
    /// subset eligible for learning or explicit review.
    #[serde(default)]
    pub changes: Vec<AnalyzedChangeResponse>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AnalyzedChangeResponse {
    pub original: String,
    pub corrected: String,
    pub reason: String,
    #[serde(default)]
    pub should_learn: bool,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub skip_reason: Option<String>,
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
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfirmBatchRequestItem {
    pub original: String,
    pub corrected: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct EditReviewSessionResponse {
    pub id: String,
    pub recording_id: String,
    pub ai_output: String,
    pub user_kept: String,
    #[serde(default)]
    pub review_candidates: Vec<ReviewCandidateResponse>,
    #[serde(default)]
    pub detected_changes: Vec<AnalyzedChangeResponse>,
    pub created_at_ms: i64,
}

pub async fn confirm_batch(
    ep: &BackendEndpoint,
    items: &[ConfirmBatchRequestItem],
    recording_id: Option<&str>,
    review_session_id: Option<&str>,
) -> Result<ConfirmBatchResponse, String> {
    let url = format!("{}/v1/confirm-batch", ep.url);
    let items_json: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "original": item.original,
                "corrected": item.corrected,
                "context": item.context,
                "tag": item.tag,
            })
        })
        .collect();
    let body = serde_json::json!({
        "items": items_json,
        "recording_id": recording_id,
        "review_session_id": review_session_id,
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

pub async fn get_next_edit_review_session(
    ep: &BackendEndpoint,
) -> Result<Option<EditReviewSessionResponse>, String> {
    let url = format!("{}/v1/edit-review-sessions/next", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("get next edit review session failed: {e}"))?
        .json::<Option<EditReviewSessionResponse>>()
        .await
        .map_err(|e| format!("parse next edit review session: {e}"))
}

pub async fn skip_edit_review_session(
    ep: &BackendEndpoint,
    session_id: &str,
) -> Result<(), String> {
    let url = format!("{}/v1/edit-review-sessions/{session_id}/skip", ep.url);
    let response = Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("skip edit review session failed: {e}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "skip edit review session returned {}",
            response.status()
        ))
    }
}

/// Classify an edit using the four-way classifier.
///
/// Sends (recording_id, ai_output, user_kept) to the backend, which looks up
/// the original transcript and proposes only safe review candidates. Nothing
/// is learned until the user explicitly confirms a candidate batch.
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
    client_run_id: Option<&str>,
    prior_text: Option<&str>,
    edit_trace_json: Option<Value>,
) -> Result<ClassifyEditResponse, String> {
    let url = format!("{}/v1/classify-edit", ep.url);
    let mut body = serde_json::json!({
        "recording_id":        recording_id,
        "ai_output":           ai_output,
        "user_kept":           user_kept,
        "capture_method":      capture_method,
        "time_since_paste_ms": capture_meta.time_since_paste_ms,
        "app_switched":        capture_meta.app_switched,
        "matches_clipboard":   capture_meta.matches_clipboard,
    });
    if let Some(run_id) = client_run_id.map(str::trim).filter(|s| !s.is_empty()) {
        body["client_run_id"] = serde_json::Value::String(run_id.to_string());
    }
    // The pre-dictation field baseline. When the user dictated into a field that
    // already had text, this lets the backend scope the edit-diff to OUR output
    // and ignore the surrounding context. Empty/None → field was empty.
    if let Some(prior) = prior_text.filter(|s| !s.is_empty()) {
        body["prior_text"] = serde_json::Value::String(prior.to_string());
    }
    if let Some(trace) = edit_trace_json {
        body["edit_trace_json"] = trace;
    }
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

// ── Vocabulary alias API (honest UI: learned wrong→right fixes) ─────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AliasRow {
    pub correct_form: String,
    pub transcript_form: String,
    pub use_count: i64,
    pub active: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AliasesResponse {
    pub aliases: Vec<AliasRow>,
}

pub async fn list_vocab_aliases(ep: &BackendEndpoint) -> Result<AliasesResponse, String> {
    let url = format!("{}/v1/vocabulary/aliases", ep.url);
    Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .send()
        .await
        .map_err(|e| format!("list vocab aliases failed: {e}"))?
        .json::<AliasesResponse>()
        .await
        .map_err(|e| format!("parse vocab aliases: {e}"))
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

async fn json_or_error(resp: reqwest::Response, label: &str) -> Result<Value, String> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{label} error {status}: {}", extract_error(&text)));
    }
    serde_json::from_str::<Value>(&text).map_err(|e| {
        format!(
            "parse {label} failed: {e} — raw: {}",
            said_core::text::truncate_utf8(&text, 240)
        )
    })
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelemetryRunPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcribe_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polish_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paste_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_clipboard_fallback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_detected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_bucket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_distance_chars: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_distance_words: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_as_is: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_entire_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub re_recorded_quickly: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learning_candidate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learning_modal_shown: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learning_confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learning_dismissed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_learning_saved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_learning_blocked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_numbers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_currency: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_percent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_email: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_url: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_code_like_terms: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mixed_language: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected_term_hit: Option<bool>,
    #[serde(default)]
    pub finalize: bool,
}

pub async fn patch_telemetry_run(
    ep: &BackendEndpoint,
    run_id: &str,
    patch: &TelemetryRunPatch,
) -> Result<(), String> {
    let url = format!("{}/v1/telemetry/runs/{}", ep.url, run_id);
    let status = Client::new()
        .patch(&url)
        .bearer_auth(&ep.secret)
        .json(patch)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| format!("telemetry patch failed: {e}"))?
        .status();
    if status.is_success() || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!("telemetry patch error: {status}"))
    }
}

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

pub async fn upload_recording_audio(
    ep: &BackendEndpoint,
    recording_id: &str,
    wav_data: Vec<u8>,
) -> Result<(), String> {
    if wav_data.is_empty() {
        return Ok(());
    }

    let url = format!("{}/v1/recordings/{recording_id}/audio", ep.url);
    let form = reqwest::multipart::Form::new().part(
        "audio",
        reqwest::multipart::Part::bytes(wav_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .map_err(|e| format!("mime error: {e}"))?,
    );
    let response = Client::new()
        .post(&url)
        .bearer_auth(&ep.secret)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("recording audio upload failed: {e}"))?;

    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!(
            "recording audio upload error {status}: {}",
            said_core::text::truncate_utf8(&body, 300)
        ))
    }
}

#[cfg(test)]
mod local_polish_transport_tests {
    use super::{BackendEndpoint, supports_local_polish_websocket};

    #[test]
    fn persistent_polish_socket_is_loopback_only() {
        let loopback = BackendEndpoint {
            url: "http://127.0.0.1:43123".to_string(),
            secret: "secret".to_string(),
        };
        let remote = BackendEndpoint {
            url: "https://airnote.example.test".to_string(),
            secret: "secret".to_string(),
        };

        assert!(supports_local_polish_websocket(&loopback, false));
        assert!(!supports_local_polish_websocket(&remote, false));
        assert!(!supports_local_polish_websocket(&loopback, true));
    }
}
