//! POST /v1/voice/polish
//!
//! Receives a multipart form with:
//!   audio        — WAV bytes  (required)
//!   target_app   — bundle-id of the focused app  (optional)
//!   pre_transcript — local ASR transcript from the desktop  (required)
//!
//! Pipeline: auth → load prefs → local transcript → evidence collection → dynamic prompt →
//!           LLM stream → SSE.

use axum::{
    Json,
    extract::{
        Multipart, State, WebSocketUpgrade,
        ws::{Message as WsMessage, WebSocket},
    },
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::{SinkExt, StreamExt};
use said_core::{
    text::Utf8LineBuffer,
    transcript::{TranscriptMeta, TranscriptOrigin},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

fn chaos_voice_fail_after_save_enabled() -> bool {
    let enabled = |key: &str| {
        matches!(
            std::env::var(key)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        )
    };
    enabled("AIRNOTE_CHAOS") && enabled("AIRNOTE_CHAOS_VOICE_FAIL_AFTER_SAVE")
}

const BACKEND_AI_PAYLOAD_LOG_ENV: &str = "AIRNOTE_BACKEND_AI_PAYLOAD_LOG";
const BACKEND_AI_PAYLOAD_LOG_PATH_ENV: &str = "AIRNOTE_BACKEND_AI_PAYLOAD_LOG_PATH";

fn backend_ai_payload_log_enabled() -> bool {
    matches!(
        std::env::var(BACKEND_AI_PAYLOAD_LOG_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn backend_ai_payload_log_path() -> PathBuf {
    std::env::var(BACKEND_AI_PAYLOAD_LOG_PATH_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("airnote-backend-ai-payloads.jsonl"))
}

fn recent_speech_hints_allowed(vocab_entries: &[VocabEntry]) -> bool {
    !vocab_entries.is_empty()
}

async fn write_backend_ai_payload_log(url: &str, req: &ServerRuntimeVoiceRequest) {
    if !backend_ai_payload_log_enabled() {
        return;
    }

    let path = backend_ai_payload_log_path();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            warn!(
                "[voice] backend AI payload log failed to create parent path={}: {err}",
                path.display()
            );
            return;
        }
    }

    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = json!({
        "kind": "backend_to_control_plane_voice_polish_stream",
        "unix_ms": unix_ms,
        "url": url,
        "client_run_id": &req.client_run_id,
        "selected_model": &req.selected_model,
        "output_language": &req.output_language,
        "target_app": &req.target_app,
        "screen_context": &req.screen_context,
        "safe_vocab_terms": &req.safe_vocab_terms,
        "vocab_cards": &req.vocab_cards,
        "recent_speech_hints": &req.recent_speech_hints,
        "transcript": &req.transcript,
    })
    .to_string();

    match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(mut file) => {
            if let Err(err) = file.write_all(format!("{line}\n").as_bytes()).await {
                warn!(
                    "[voice] backend AI payload log write failed path={}: {err}",
                    path.display()
                );
            } else {
                info!(
                    "[voice] backend AI payload log wrote path={} run_id={} transcript_chars={} vocab_terms={} vocab_cards={} recent_speech_hints={}",
                    path.display(),
                    req.client_run_id.as_deref().unwrap_or("none"),
                    req.transcript.chars().count(),
                    req.safe_vocab_terms.len(),
                    req.vocab_cards.len(),
                    req.recent_speech_hints.len()
                );
            }
        }
        Err(err) => warn!(
            "[voice] backend AI payload log open failed path={}: {err}",
            path.display()
        ),
    }
}

fn voice_error_code_for(message: &str, explicit: Option<&str>) -> String {
    if let Some(code) = explicit.filter(|s| !s.trim().is_empty()) {
        return code.to_string();
    }
    let lower = message.to_ascii_lowercase();
    if lower.contains("payload too large")
        || lower.contains("length limit exceeded")
        || lower.contains("request too large")
        || lower.contains("audio too large")
    {
        "audio_payload_too_large".to_string()
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "runtime_timeout".to_string()
    } else if lower.contains("network")
        || lower.contains("dns")
        || lower.contains("failed to connect")
        || lower.contains("connection")
    {
        "runtime_network_error".to_string()
    } else if lower.contains("server runtime")
        || lower.contains("service unavailable")
        || lower.contains("internal error")
    {
        "server_runtime_failed".to_string()
    } else if lower.contains("no speech") || lower.contains("empty transcript") {
        "no_speech_detected".to_string()
    } else {
        "voice_pipeline_failed".to_string()
    }
}

fn voice_error_retryable(code: &str) -> bool {
    matches!(
        code,
        "audio_payload_too_large"
            | "runtime_timeout"
            | "runtime_network_error"
            | "server_runtime_failed"
            | "voice_pipeline_failed"
            | "local_stt_no_transcript"
            | "no_speech_detected"
    )
}

fn voice_error_owned_by_airnote(code: &str, message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    matches!(
        code,
        "audio_payload_too_large"
            | "runtime_timeout"
            | "runtime_network_error"
            | "server_runtime_failed"
            | "voice_pipeline_failed"
    ) || lower.contains("sse stream ended")
}

fn voice_error_payload(
    message: impl Into<String>,
    run_id: Option<&str>,
    audio_id: Option<&str>,
    explicit_code: Option<&str>,
) -> Value {
    let raw_message = message.into();
    let details = crate::llm::decode_llm_error(&raw_message);
    let message = details
        .as_ref()
        .map(|details| details.message.clone())
        .unwrap_or(raw_message);
    let detail_code = details
        .as_ref()
        .and_then(|details| details.error_code.as_deref());
    let error_code = voice_error_code_for(&message, explicit_code.or(detail_code));
    let retryable = audio_id.is_some()
        && details
            .as_ref()
            .and_then(|details| details.retryable)
            .unwrap_or_else(|| voice_error_retryable(&error_code));
    let owned_by_airnote = voice_error_owned_by_airnote(&error_code, &message);
    let diagnostic = details
        .and_then(|details| details.diagnostic)
        .unwrap_or_else(|| {
            format!(
                "AirNote voice pipeline failure; code={}; retryable={}; saved_audio={}",
                error_code,
                retryable,
                audio_id.unwrap_or("none")
            )
        });
    json!({
        "message": message,
        "run_id": run_id,
        "audio_id": audio_id,
        "error_code": error_code,
        "retryable": retryable,
        "owned_by_airnote": owned_by_airnote,
        "diagnostic": diagnostic,
    })
}

fn voice_error_event(
    message: impl Into<String>,
    audio_id: Option<&str>,
    explicit_code: Option<&str>,
) -> Event {
    Event::default()
        .event("error")
        .data(voice_error_payload(message, None, audio_id, explicit_code).to_string())
}

fn voice_run_failed_event(
    pool: &crate::store::DbPool,
    run_id: &str,
    message: impl Into<String>,
    audio_id: Option<&str>,
    explicit_code: Option<&str>,
) -> Event {
    let raw_message = message.into();
    let details = crate::llm::decode_llm_error(&raw_message);
    let message = details
        .as_ref()
        .map(|details| details.message.clone())
        .unwrap_or(raw_message);
    let detail_code = details
        .as_ref()
        .and_then(|details| details.error_code.as_deref());
    let error_code = voice_error_code_for(&message, explicit_code);
    let error_code = detail_code.unwrap_or(&error_code).to_string();
    let retryable = audio_id.is_some()
        && details
            .as_ref()
            .and_then(|details| details.retryable)
            .unwrap_or_else(|| voice_error_retryable(&error_code));
    let owned_by_airnote = voice_error_owned_by_airnote(&error_code, &message);
    let payload = voice_error_payload(&message, Some(run_id), audio_id, Some(&error_code));
    let payload = if let Some(diagnostic) = details.and_then(|details| details.diagnostic) {
        let mut payload = payload;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("diagnostic".to_string(), json!(diagnostic));
        }
        payload
    } else {
        payload
    };
    let _ = crate::store::voice_runs::mark_voice_run_failed(
        pool,
        run_id,
        &error_code,
        &message,
        retryable,
        owned_by_airnote,
        Some(&payload),
    );
    Event::default().event("error").data(payload.to_string())
}

// ── Audio file helpers ────────────────────────────────────────────────────────

/// Extract actual speech duration from WAV header (byte_rate at offset 28, data size at offset 40).
fn wav_duration_secs(wav: &[u8]) -> f64 {
    if wav.len() < 44 {
        return 0.0;
    }
    let byte_rate = u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]) as f64;
    let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as f64;
    if byte_rate > 0.0 {
        data_size / byte_rate
    } else {
        0.0
    }
}

/// Estimate speaking duration from word count at 130 WPM (used when no audio is available).
fn estimated_secs(word_count: i64) -> f64 {
    word_count as f64 * 60.0 / 130.0
}

/// Directory where WAV recordings are saved locally (1-day retention).
fn audio_dir() -> std::path::PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("VoicePolish").join("audio")
}

/// Save WAV bytes to disk. Returns the path on success.
fn save_audio(id: &str, data: &[u8]) -> Option<std::path::PathBuf> {
    let dir = audio_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{id}.wav"));
    std::fs::write(&path, data).ok()?;
    debug!("[voice] saved audio to {}", path.display());
    Some(path)
}

/// Delete ordinary WAV files older than 24 hours. Retryable failed runs keep
/// their WAVs for 7 days so users can reprocess captured speech.
pub fn cleanup_old_audio(pool: &crate::store::DbPool) {
    let dir = audio_dir();
    let now_ms = crate::store::now_ms();
    let protected_cutoff_ms = now_ms - 7 * 86_400_000i64;
    let protected: std::collections::HashSet<String> =
        crate::store::voice_runs::retryable_failed_audio_ids(pool, protected_cutoff_ms)
            .into_iter()
            .collect();
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(86_400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff {
            let audio_id = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if protected.contains(&audio_id) {
                debug!(
                    "[voice] keeping retryable failed audio {}",
                    entry.path().display()
                );
                continue;
            }
            let _ = std::fs::remove_file(entry.path());
            debug!("[voice] deleted old audio {}", entry.path().display());
        }
    }
}

use crate::{
    AppState,
    embedder::gemini,
    llm::{
        openai_codex,
        prompt::{
            VocabEntry, build_user_message_with_hints, build_voice_repair_system_prompt,
            build_voice_repair_user_message, default_voice_prompt_template,
            render_voice_system_prompt_template_with_profile_and_recent,
        },
        script,
        stream_safety::scrub_polished_output,
        vocab_retrieval::{self, VocabRetrievalRequest},
    },
    store::{
        company_vocab,
        history::{InsertRecording, insert_recording},
        openai_oauth, vocab_embeddings, vocabulary,
    },
};

fn invalidate_openai_session_on_auth_error(
    pool: &crate::store::DbPool,
    user_id: &str,
    llm_provider: &str,
    err: &str,
) -> bool {
    if llm_provider != "openai_codex" || !openai_codex::is_auth_error(err) {
        return false;
    }
    openai_oauth::delete_token(pool, user_id);
    warn!("[voice] invalidated stored OpenAI OAuth token after auth failure");
    true
}

#[derive(Debug)]
struct VoicePolishInput {
    wav_data: Vec<u8>,
    target_app: Option<String>,
    pre_transcript: Option<String>,
    pre_transcript_meta: Option<TranscriptMeta>,
    repair_mode: Option<String>,
    screen_context: Option<String>,
    message_polish_mode: bool,
    client_run_id: Option<String>,
    client_trace_json: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ServerRuntimeVoiceRequest {
    transcript: String,
    output_language: String,
    selected_model: String,
    screen_context: Option<String>,
    safe_vocab_terms: Vec<String>,
    /// Rich, evidence-backed cards selected by the local retriever. The control
    /// plane treats them as soft evidence; the term list remains for prompt
    /// compatibility with older control-plane versions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    vocab_cards: Vec<ServerRuntimeVocabCard>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recent_speech_hints: Vec<String>,
    /// Focused-app key (bundle-id / exe). Lets the server pick the per-app profile
    /// bucket. The learned profile now lives server-side, so the client no longer
    /// ships `client_profile_markdown` — the server injects its own KB.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_app: Option<String>,
    client_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ServerRuntimeVocabCard {
    term: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    term_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meaning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    do_not_use_when: Option<String>,
}

const SERVER_VOCAB_CARD_LIMIT: usize = 8;

fn clean_server_vocab_text(raw: &str, max_chars: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

fn supported_vocab_term_type(term_type: Option<&str>) -> Option<String> {
    match term_type.map(str::trim) {
        Some("acronym" | "proper_noun" | "brand" | "code_identifier" | "phrase" | "other") => {
            term_type.map(str::trim).map(str::to_string)
        }
        _ => None,
    }
}

fn server_vocab_cards(entries: &[VocabEntry]) -> Vec<ServerRuntimeVocabCard> {
    entries
        .iter()
        .filter_map(|entry| {
            let term = clean_server_vocab_text(&entry.term, 96);
            if term.is_empty() {
                return None;
            }

            let clean_optional = |value: Option<&String>| {
                value.and_then(|value| {
                    let value = clean_server_vocab_text(value, 180);
                    (!value.is_empty()).then_some(value)
                })
            };
            Some(ServerRuntimeVocabCard {
                term,
                term_type: supported_vocab_term_type(entry.term_type.as_deref()),
                meaning: clean_optional(entry.meaning.as_ref()),
                context: clean_optional(entry.context.as_ref()),
                aliases: entry
                    .stt_aliases
                    .iter()
                    .map(|(alias, _)| clean_server_vocab_text(alias, 80))
                    .filter(|alias| !alias.is_empty())
                    .take(6)
                    .collect(),
                evidence: entry
                    .evidence
                    .iter()
                    .map(|evidence| clean_server_vocab_text(evidence, 100))
                    .filter(|evidence| !evidence.is_empty())
                    .take(4)
                    .collect(),
                do_not_use_when: clean_optional(entry.do_not_use_when.as_ref()),
            })
        })
        .take(SERVER_VOCAB_CARD_LIMIT)
        .collect()
}

#[derive(Debug, Deserialize)]
struct ServerRuntimeVoiceResponse {
    output: String,
    model_used: String,
    latency_ms: ServerRuntimeLatency,
}

#[derive(Debug, Deserialize)]
struct ServerRuntimeLatency {
    #[serde(default)]
    prompt: i64,
    #[serde(default)]
    model: i64,
    total: i64,
}

#[derive(Debug, Clone, Default)]
struct ServerRuntimeTraceMeta {
    roundtrip_ms: u64,
    server_total_ms: u64,
    server_prompt_ms: i64,
    server_model_ms: i64,
    first_token_ms: Option<u128>,
    token_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptPolishRequest {
    transcript: String,
    target_app: Option<String>,
    #[serde(default)]
    pre_transcript_meta: Option<TranscriptMeta>,
}

#[derive(Debug, Serialize)]
pub struct ProblemTranscribeResponse {
    transcript: String,
    source: String,
    confidence: f64,
    word_count: usize,
    latency_ms: i64,
}

#[derive(Debug, Deserialize)]
pub struct VoiceRepairRequest {
    transcript: String,
    previous_output: String,
    target_app: Option<String>,
    output_language: Option<String>,
    audio_id: Option<String>,
    #[serde(default)]
    enriched_transcript: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

pub async fn polish(State(state): State<AppState>, mut multipart: Multipart) -> impl IntoResponse {
    if !crate::store::users::has_enterprise_auth(&state.pool, &state.default_user_id) {
        return (
            StatusCode::FORBIDDEN,
            json!({"error": "workspace connection required — sign in to your organization in AirNote"}).to_string(),
        )
            .into_response();
    }

    // ── Extract multipart fields ───────────────────────────────────────────────
    let request_start = Instant::now();
    let mut wav_data: Vec<u8> = Vec::new();
    let mut target_app: Option<String> = None;
    let mut pre_transcript: Option<String> = None;
    let mut pre_transcript_meta: Option<TranscriptMeta> = None;
    let mut repair_mode: Option<String> = None;
    let mut screen_context: Option<String> = None;
    let mut message_polish_mode = false;
    let mut client_run_id: Option<String> = None;
    let mut client_trace_json: Option<serde_json::Value> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("audio") => match field.bytes().await {
                Ok(b) => wav_data = b.to_vec(),
                Err(e) => {
                    warn!(
                        "[voice] failed to read audio field: {e} — payload may exceed body limit"
                    );
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        json!({"error": "audio too large"}).to_string(),
                    )
                        .into_response();
                }
            },
            Some("target_app") => {
                target_app = field.text().await.ok();
            }
            Some("pre_transcript") => {
                pre_transcript = field.text().await.ok().filter(|s| !s.is_empty());
            }
            Some("pre_transcript_meta") => {
                pre_transcript_meta = field
                    .text()
                    .await
                    .ok()
                    .and_then(|s| serde_json::from_str::<TranscriptMeta>(&s).ok());
            }
            Some("repair_mode") => {
                repair_mode = field.text().await.ok().filter(|s| !s.is_empty());
            }
            Some("screen_context") => {
                screen_context = field.text().await.ok().filter(|s| !s.trim().is_empty());
            }
            Some("message_polish_mode") => {
                message_polish_mode = field
                    .text()
                    .await
                    .map(|s| matches!(s.as_str(), "1" | "true" | "yes" | "on"))
                    .unwrap_or(false);
            }
            Some("client_run_id") => {
                client_run_id = field.text().await.ok().filter(|s| !s.trim().is_empty());
            }
            Some("client_trace_json") => {
                client_trace_json = field
                    .text()
                    .await
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            }
            _ => {}
        }
    }
    let parse_ms = request_start.elapsed().as_millis();
    let pre_chars = pre_transcript
        .as_ref()
        .map(|t| t.chars().count())
        .unwrap_or(0);
    let pre_words = pre_transcript
        .as_ref()
        .map(|t| t.split_whitespace().count())
        .unwrap_or(0);
    info!(
        "[voice] multipart parsed in {}ms wav_bytes={} pre_transcript_present={} pre_chars={} pre_words={} pre_meta={} message_polish={} repair_mode={} screen_context_chars={} target_app_present={} client_run_id={}",
        parse_ms,
        wav_data.len(),
        pre_transcript.is_some(),
        pre_chars,
        pre_words,
        pre_transcript_meta.is_some(),
        message_polish_mode,
        repair_mode.is_some(),
        screen_context
            .as_ref()
            .map(|s| s.chars().count())
            .unwrap_or(0),
        target_app.is_some(),
        client_run_id.as_deref().unwrap_or("none"),
    );

    if pre_transcript
        .as_deref()
        .is_none_or(|t| t.trim().is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            json!({
                "error_code": "local_transcript_required",
                "message": "local speech transcript is required before voice polish"
            })
            .to_string(),
        )
            .into_response();
    }

    polish_with_input(
        state,
        VoicePolishInput {
            wav_data,
            target_app,
            pre_transcript,
            pre_transcript_meta,
            repair_mode,
            screen_context,
            message_polish_mode,
            client_run_id,
            client_trace_json,
        },
    )
    .await
}

pub async fn polish_transcript(
    State(state): State<AppState>,
    Json(req): Json<TranscriptPolishRequest>,
) -> impl IntoResponse {
    if !crate::store::users::has_enterprise_auth(&state.pool, &state.default_user_id) {
        return (
            StatusCode::FORBIDDEN,
            json!({"error": "workspace connection required — sign in to your organization in AirNote"}).to_string(),
        )
            .into_response();
    }

    let transcript = req.transcript.trim().to_string();
    if transcript.is_empty() {
        warn!("[voice] received empty transcript-only polish request");
        return StatusCode::BAD_REQUEST.into_response();
    }

    polish_with_input(
        state,
        VoicePolishInput {
            wav_data: Vec::new(),
            target_app: req.target_app,
            pre_transcript: Some(transcript),
            pre_transcript_meta: req.pre_transcript_meta,
            repair_mode: None,
            screen_context: None,
            message_polish_mode: false,
            client_run_id: None,
            client_trace_json: None,
        },
    )
    .await
}

// ── Persistent local polish WebSocket ────────────────────────────────────────
//
// This endpoint deliberately transports an already-produced desktop transcript.
// It is not the legacy `/v1/runtime/live/ws` cloud-audio proxy, and it never
// accepts microphone frames. The detached SSE drain keeps an accepted polish run
// alive while a desktop client reconnects to the local sidecar.

#[derive(Debug, Deserialize)]
struct PolishWsStart {
    run_id: String,
    transcript: String,
    #[serde(default)]
    target_app: Option<String>,
    #[serde(default)]
    pre_transcript_meta: Option<TranscriptMeta>,
    #[serde(default)]
    repair_mode: Option<String>,
    #[serde(default)]
    screen_context: Option<String>,
    #[serde(default)]
    message_polish_mode: bool,
    #[serde(default)]
    client_trace_json: Option<Value>,
}

fn ws_envelope(kind: &str, run_id: &str, seq: u64, payload: Value) -> Value {
    json!({
        "type": kind,
        "protocol_version": 1,
        "run_id": run_id,
        "seq": seq,
        "payload": payload,
    })
}

fn ws_error(run_id: Option<&str>, code: &str, message: impl Into<String>) -> Value {
    json!({
        "type": "error",
        "protocol_version": 1,
        "run_id": run_id,
        "payload": {
            "message": message.into(),
            "error_code": code,
            "retryable": false,
            "owned_by_airnote": true,
        }
    })
}

#[derive(Clone, Copy)]
struct PolishWsDeadlines {
    first_event: Duration,
    idle: Duration,
    total: Duration,
}

const DEFAULT_POLISH_WS_DEADLINES: PolishWsDeadlines = PolishWsDeadlines {
    first_event: Duration::from_secs(20),
    idle: Duration::from_secs(30),
    total: Duration::from_secs(120),
};

async fn send_ws_json(
    sink: &mut futures::stream::SplitSink<WebSocket, WsMessage>,
    value: &Value,
) -> bool {
    sink.send(WsMessage::Text(value.to_string())).await.is_ok()
}

/// Return the durable terminal event after a sidecar restart. A non-terminal
/// database row without an in-memory producer is intentionally failed rather
/// than guessed/replayed, because the model work may have been interrupted.
fn durable_ws_resume_event(state: &AppState, run_id: &str) -> Value {
    let Some(run) = crate::store::voice_runs::get_voice_run(&state.pool, run_id) else {
        return ws_error(Some(run_id), "unknown_run", "polish run was not accepted");
    };
    if let Some(event) = run
        .terminal_event_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    {
        return event;
    }

    if run.status == "completed" {
        if let Some(recording) = run
            .recording_id
            .as_deref()
            .and_then(|id| crate::store::history::get_recording(&state.pool, id))
        {
            return ws_envelope(
                "done",
                run_id,
                0,
                json!({
                    "recording_id": recording.id,
                    "transcript": recording.transcript,
                    "polished": recording.polished,
                    "model_used": recording.model_used,
                    "confidence": recording.confidence,
                    "audio_id": recording.audio_id,
                    "source": recording.source,
                    "target_app": recording.target_app,
                    "enriched_transcript": recording.enriched_transcript,
                    "examples_used": 0,
                    "latency_ms": {
                        "transcribe": recording.transcribe_ms.unwrap_or_default(),
                        "embed": recording.embed_ms.unwrap_or_default(),
                        "retrieve": 0,
                        "polish": recording.polish_ms.unwrap_or_default(),
                        "total": recording.transcribe_ms.unwrap_or_default()
                            + recording.embed_ms.unwrap_or_default()
                            + recording.polish_ms.unwrap_or_default(),
                    }
                }),
            );
        }
    }

    if matches!(run.status.as_str(), "captured" | "processing") {
        let message = "local backend restarted before this polish run completed";
        let payload = ws_error(Some(run_id), "backend_restarted", message);
        let _ = crate::store::voice_runs::mark_voice_run_failed(
            &state.pool,
            run_id,
            "backend_restarted",
            message,
            false,
            true,
            Some(&payload),
        );
        let _ = crate::store::voice_runs::store_terminal_event(&state.pool, run_id, &payload);
        return payload;
    }

    ws_error(
        Some(run_id),
        run.error_code.as_deref().unwrap_or("polish_failed"),
        run.error_message
            .unwrap_or_else(|| "polish run failed".to_string()),
    )
}

async fn forward_polish_sse_to_ws(
    state: AppState,
    run_id: String,
    sender: broadcast::Sender<Value>,
    response: Response,
) {
    forward_polish_sse_to_ws_with_deadlines(
        state,
        run_id,
        sender,
        response,
        DEFAULT_POLISH_WS_DEADLINES,
    )
    .await;
}

/// Drains the internal polish SSE independently of a desktop socket. Its own
/// deadlines are essential: a client-side timeout alone would otherwise leave
/// the detached producer and durable run stuck in `processing` forever.
async fn forward_polish_sse_to_ws_with_deadlines(
    state: AppState,
    run_id: String,
    sender: broadcast::Sender<Value>,
    response: Response,
    deadlines: PolishWsDeadlines,
) {
    let status = response.status();
    if !status.is_success() {
        let event = ws_error(
            Some(&run_id),
            "polish_start_failed",
            format!("local polish request was rejected with {status}"),
        );
        let _ = crate::store::voice_runs::store_terminal_event(&state.pool, &run_id, &event);
        let _ = sender.send(event);
        state.voice_run_hub.lock().await.remove(&run_id);
        return;
    }

    let mut stream = response.into_body().into_data_stream();
    let mut line_buffer = Utf8LineBuffer::default();
    let mut event_name = String::from("message");
    let mut data_lines: Vec<String> = Vec::new();
    let mut seq = 0u64;
    let mut terminal = false;
    let started = Instant::now();
    let mut last_progress = started;
    let mut saw_progress = false;
    let mut stream_failure: Option<(&str, &str)> = None;

    loop {
        let now = Instant::now();
        let total_remaining = (started + deadlines.total).saturating_duration_since(now);
        let phase_deadline = if saw_progress {
            last_progress + deadlines.idle
        } else {
            started + deadlines.first_event
        };
        let phase_remaining = phase_deadline.saturating_duration_since(now);
        let wait_for = total_remaining.min(phase_remaining);
        if wait_for.is_zero() {
            stream_failure = Some(if total_remaining.is_zero() {
                (
                    "polish_stream_total_timeout",
                    "local polish stream exceeded its total deadline",
                )
            } else if saw_progress {
                (
                    "polish_stream_idle_timeout",
                    "local polish stream stopped making progress",
                )
            } else {
                (
                    "polish_stream_first_event_timeout",
                    "local polish stream did not start in time",
                )
            });
            break;
        }

        let chunk = match tokio::time::timeout(wait_for, stream.next()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(_))) => {
                stream_failure = Some((
                    "polish_stream_read_failed",
                    "local polish stream failed while reading the provider response",
                ));
                break;
            }
            Ok(None) => break,
            Err(_) => {
                stream_failure = Some(if total_remaining <= phase_remaining {
                    (
                        "polish_stream_total_timeout",
                        "local polish stream exceeded its total deadline",
                    )
                } else if saw_progress {
                    (
                        "polish_stream_idle_timeout",
                        "local polish stream stopped making progress",
                    )
                } else {
                    (
                        "polish_stream_first_event_timeout",
                        "local polish stream did not start in time",
                    )
                });
                break;
            }
        };
        let lines = match line_buffer.push(&chunk) {
            Ok(lines) => lines,
            Err(_) => {
                stream_failure = Some((
                    "polish_stream_invalid_data",
                    "local polish stream returned invalid data",
                ));
                break;
            }
        };
        for mut line in lines {
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                if !data_lines.is_empty() {
                    seq = seq.saturating_add(1);
                    let payload_text = data_lines.join("\n");
                    let payload = serde_json::from_str::<Value>(&payload_text)
                        .unwrap_or_else(|_| json!({ "message": payload_text }));
                    let kind = match event_name.as_str() {
                        "token" | "status" | "done" | "error" => event_name.as_str(),
                        _ => "status",
                    };
                    let event = ws_envelope(kind, &run_id, seq, payload);
                    if matches!(kind, "done" | "error") {
                        let _ = crate::store::voice_runs::store_terminal_event(
                            &state.pool,
                            &run_id,
                            &event,
                        );
                        terminal = true;
                    }
                    let _ = sender.send(event);
                    saw_progress = true;
                    last_progress = Instant::now();
                }
                event_name.clear();
                event_name.push_str("message");
                data_lines.clear();
                continue;
            }
            if let Some(name) = line.strip_prefix("event:") {
                event_name = name.trim().to_string();
            } else if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start().to_string());
            }
        }
    }

    if !terminal {
        let (code, message) = stream_failure.unwrap_or((
            "stream_ended_without_terminal",
            "local polish stream ended without a terminal event",
        ));
        let event = ws_error(Some(&run_id), code, message);
        let _ = crate::store::voice_runs::mark_voice_run_failed(
            &state.pool,
            &run_id,
            code,
            message,
            false,
            true,
            Some(&event),
        );
        let _ = crate::store::voice_runs::store_terminal_event(&state.pool, &run_id, &event);
        let _ = sender.send(event);
    }
    state.voice_run_hub.lock().await.remove(&run_id);
}

async fn start_or_subscribe_ws_run(
    state: AppState,
    start: PolishWsStart,
) -> Result<broadcast::Receiver<Value>, Value> {
    let run_id = start.run_id.trim().to_string();
    if run_id.is_empty() || run_id.len() > 128 {
        return Err(ws_error(
            None,
            "invalid_run_id",
            "run_id must be 1-128 characters",
        ));
    }
    let transcript = start.transcript.trim().to_string();
    if transcript.is_empty() {
        return Err(ws_error(
            Some(&run_id),
            "local_transcript_required",
            "local speech transcript is required before voice polish",
        ));
    }
    {
        let hub = state.voice_run_hub.lock().await;
        if let Some(sender) = hub.get(&run_id) {
            return Ok(sender.subscribe());
        }
    }
    if crate::store::voice_runs::get_voice_run(&state.pool, &run_id).is_some() {
        return Err(ws_error(
            Some(&run_id),
            "run_already_exists",
            "run already exists; resume it instead of starting it again",
        ));
    }

    let (sender, receiver, is_new) = {
        let mut hub = state.voice_run_hub.lock().await;
        if let Some(sender) = hub.get(&run_id) {
            (sender.clone(), sender.subscribe(), false)
        } else {
            let (sender, receiver) = broadcast::channel(512);
            hub.insert(run_id.clone(), sender.clone());
            (sender, receiver, true)
        }
    };
    if !is_new {
        return Ok(receiver);
    }

    let response = polish_with_input(
        state.clone(),
        VoicePolishInput {
            wav_data: Vec::new(),
            target_app: start.target_app,
            pre_transcript: Some(transcript),
            pre_transcript_meta: start.pre_transcript_meta,
            repair_mode: start.repair_mode,
            screen_context: start.screen_context,
            message_polish_mode: start.message_polish_mode,
            client_run_id: Some(run_id.clone()),
            client_trace_json: start.client_trace_json,
        },
    )
    .await;
    if !response.status().is_success() {
        let event = ws_error(
            Some(&run_id),
            "polish_start_failed",
            format!(
                "local polish request was rejected with {}",
                response.status()
            ),
        );
        let _ = crate::store::voice_runs::store_terminal_event(&state.pool, &run_id, &event);
        let _ = sender.send(event.clone());
        state.voice_run_hub.lock().await.remove(&run_id);
        return Err(event);
    }

    let attempt = crate::store::voice_runs::get_voice_run(&state.pool, &run_id)
        .map(|run| run.attempt_count)
        .unwrap_or(1);
    let _ = sender.send(ws_envelope(
        "run.accepted",
        &run_id,
        0,
        json!({ "attempt": attempt }),
    ));
    tokio::spawn(forward_polish_sse_to_ws(state, run_id, sender, response));
    Ok(receiver)
}

pub async fn polish_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    if !crate::store::users::has_enterprise_auth(&state.pool, &state.default_user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| handle_polish_ws(state, socket))
        .into_response()
}

async fn handle_polish_ws(state: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    if !send_ws_json(
        &mut sink,
        &json!({ "type": "polish.connected", "protocol_version": 1 }),
    )
    .await
    {
        return;
    }

    let mut active_run: Option<String> = None;
    let mut subscription: Option<broadcast::Receiver<Value>> = None;
    loop {
        tokio::select! {
            inbound = stream.next() => {
                let Some(Ok(inbound)) = inbound else { return };
                match inbound {
                    WsMessage::Text(raw) => {
                        let value = match serde_json::from_str::<Value>(&raw) {
                            Ok(value) => value,
                            Err(_) => {
                                if !send_ws_json(&mut sink, &ws_error(None, "invalid_json", "invalid WebSocket JSON")).await { return; }
                                continue;
                            }
                        };
                        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
                        match kind {
                            "ping" => {
                                if !send_ws_json(&mut sink, &json!({ "type": "pong", "protocol_version": 1 })).await { return; }
                            }
                            "polish.start" => {
                                if active_run.is_some() {
                                    if !send_ws_json(&mut sink, &ws_error(active_run.as_deref(), "run_already_active", "finish or reconnect the active run first")).await { return; }
                                    continue;
                                }
                                let start = match serde_json::from_value::<PolishWsStart>(value) {
                                    Ok(start) => start,
                                    Err(_) => {
                                        if !send_ws_json(&mut sink, &ws_error(None, "invalid_start", "invalid polish.start payload")).await { return; }
                                        continue;
                                    }
                                };
                                let run_id = start.run_id.trim().to_string();
                                match start_or_subscribe_ws_run(state.clone(), start).await {
                                    Ok(receiver) => {
                                        active_run = Some(run_id);
                                        subscription = Some(receiver);
                                    }
                                    Err(event) => {
                                        if !send_ws_json(&mut sink, &event).await { return; }
                                    }
                                }
                            }
                            "run.resume" => {
                                let Some(run_id) = value.get("run_id").and_then(Value::as_str).map(str::trim).filter(|id| !id.is_empty()) else {
                                    if !send_ws_json(&mut sink, &ws_error(None, "invalid_run_id", "run.resume requires a run_id")).await { return; }
                                    continue;
                                };
                                if active_run.is_some() {
                                    if !send_ws_json(&mut sink, &ws_error(active_run.as_deref(), "run_already_active", "finish or reconnect the active run first")).await { return; }
                                    continue;
                                }
                                let receiver = {
                                    let hub = state.voice_run_hub.lock().await;
                                    hub.get(run_id).map(broadcast::Sender::subscribe)
                                };
                                if let Some(receiver) = receiver {
                                    active_run = Some(run_id.to_string());
                                    subscription = Some(receiver);
                                    if !send_ws_json(&mut sink, &ws_envelope("run.resumed", run_id, 0, json!({}))).await { return; }
                                } else {
                                    let event = durable_ws_resume_event(&state, run_id);
                                    if !send_ws_json(&mut sink, &event).await { return; }
                                }
                            }
                            _ => {
                                if !send_ws_json(&mut sink, &ws_error(None, "unknown_message", "unknown polish WebSocket message")).await { return; }
                            }
                        }
                    }
                    WsMessage::Ping(payload)
                        if sink.send(WsMessage::Pong(payload.clone())).await.is_err() =>
                    {
                        return;
                    }
                    WsMessage::Ping(_) => {}
                    WsMessage::Close(_) => return,
                    _ => {}
                }
            }
            event = async { subscription.as_mut().expect("subscription guarded").recv().await }, if subscription.is_some() => {
                match event {
                    Ok(event) => {
                        let terminal = matches!(event.get("type").and_then(Value::as_str), Some("done" | "error"));
                        if !send_ws_json(&mut sink, &event).await { return; }
                        if terminal {
                            active_run = None;
                            subscription = None;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !send_ws_json(&mut sink, &json!({ "type": "run.resync_required", "protocol_version": 1, "run_id": active_run })).await { return; }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        subscription = None;
                        active_run = None;
                    }
                }
            }
        }
    }
}

pub async fn problem_transcribe(
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut _audio_seen = false;
    let mut pre_transcript: Option<String> = None;
    let mut pre_transcript_meta: Option<TranscriptMeta> = None;
    let mut client_run_id: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("audio") => {
                match field.bytes().await {
                    Ok(b) => _audio_seen = !b.is_empty(),
                    Err(e) => {
                        warn!("[problem] failed to read audio field: {e}");
                        return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        Json(json!({"error_code": "audio_too_large", "message": "audio too large"})),
                    )
                        .into_response();
                    }
                }
            }
            Some("pre_transcript") => {
                pre_transcript = field.text().await.ok().filter(|s| !s.trim().is_empty());
            }
            Some("pre_transcript_meta") => {
                pre_transcript_meta = field
                    .text()
                    .await
                    .ok()
                    .and_then(|s| serde_json::from_str::<TranscriptMeta>(&s).ok());
            }
            Some("client_run_id") => {
                client_run_id = field.text().await.ok().filter(|s| !s.trim().is_empty());
            }
            _ => {}
        }
    }

    if pre_transcript
        .as_deref()
        .is_none_or(|t| t.trim().is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error_code": "local_transcript_required",
                "message": "local speech transcript is required"
            })),
        )
            .into_response();
    }

    let transcript_input = pre_transcript.unwrap_or_default();
    let transcript_plain = strip_confidence_markers(&transcript_input);
    let word_count = transcript_plain.split_whitespace().count();
    let meta = pre_transcript_meta.unwrap_or_else(|| TranscriptMeta {
        enriched_transcript: transcript_input.clone(),
        confidence: 0.95,
        mean_word_confidence: 0.95,
        word_count,
        origin: TranscriptOrigin::DictationLocal,
        model: said_core::stt::telemetry_speech_model().to_string(),
        ..TranscriptMeta::default()
    });
    let chosen = TranscriptCandidate {
        transcript: transcript_plain,
        meta: TranscriptMeta {
            enriched_transcript: transcript_input,
            ..meta
        },
        source: "problem_local".to_string(),
    };

    let transcript = chosen.transcript.trim().to_string();
    if transcript.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error_code": "no_speech_detected",
                "message": "no speech detected — try speaking again"
            })),
        )
            .into_response();
    }

    let latency_ms = start.elapsed().as_millis() as i64;
    info!(
        "[problem] transcribe done run_id={} source={} chars={} words={} confidence={:.2} latency_ms={}",
        client_run_id.as_deref().unwrap_or("none"),
        chosen.source,
        transcript.chars().count(),
        chosen.meta.word_count,
        chosen.meta.confidence,
        latency_ms,
    );

    Json(ProblemTranscribeResponse {
        transcript,
        source: chosen.source,
        confidence: chosen.meta.confidence,
        word_count: chosen.meta.word_count,
        latency_ms,
    })
    .into_response()
}

pub async fn repair_transcript(
    State(state): State<AppState>,
    Json(req): Json<VoiceRepairRequest>,
) -> impl IntoResponse {
    if !crate::store::users::has_enterprise_auth(&state.pool, &state.default_user_id) {
        return (
            StatusCode::FORBIDDEN,
            json!({"error": "workspace connection required — sign in to your organization in AirNote"}).to_string(),
        )
            .into_response();
    }

    let transcript = req.transcript.trim().to_string();
    let previous_output = req.previous_output.trim().to_string();
    if transcript.is_empty() || previous_output.is_empty() {
        warn!("[voice-repair] received empty transcript or previous output");
        return StatusCode::BAD_REQUEST.into_response();
    }

    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();
    let prefs_opt = crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id).await;
    let http_client = state.http_client.clone();

    let stream = async_stream::stream! {
        let total_start = Instant::now();
        let prefs = match prefs_opt {
            Some(p) => p,
            None => {
                yield Ok::<Event, Infallible>(voice_error_event(
                    "preferences not found",
                    req.audio_id.as_deref(),
                    Some("preferences_not_found"),
                ));
                return;
            }
        };

        let output_language = req
            .output_language
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| prefs.output_language.clone());
        let hints = derive_repair_hints(&transcript, &previous_output, req.enriched_transcript.as_deref(), &output_language);
        let system_prompt = build_voice_repair_system_prompt(&output_language, &hints);
        let user_message = build_voice_repair_user_message(&transcript, &previous_output, &output_language);

        yield Ok(Event::default().event("status")
            .data(json!({"phase": "polishing", "transcript": transcript}).to_string()));

        let gateway_key = prefs.gateway_api_key.clone()
            .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
            .or_else(|| { let k = said_core::api_key(); if k.is_empty() { None } else { Some(k.to_string()) } })
            .unwrap_or_default();
        let gemini_key = prefs.gemini_api_key.clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        let groq_key = prefs.groq_api_key.clone()
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();
        let deepinfra_key = prefs.deepinfra_api_key.clone()
            .or_else(|| std::env::var("DEEPINFRA_API_KEY").ok())
            .unwrap_or_default();
        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let sys_p = system_prompt.clone();
        let usr_m = user_message.clone();
        let client_c = http_client.clone();
        let groq_key_for_recovery = groq_key.clone();
        let llm_provider = prefs.llm_provider.clone();
        let route = crate::llm::polish_dispatch::voice_polish_route(&prefs.selected_model);
        let openai_token_opt = if llm_provider == "openai_codex" {
            let pool_tok = pool.clone();
            let uid_tok = user_id.clone();
            let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
                .await
                .unwrap_or(None);
            tok.map(|t| t.access_token)
        } else {
            None
        };
        let llm_provider_for_task = llm_provider.clone();
        let actual_model_used = route.label();

        let llm_task = tokio::spawn(async move {
            crate::llm::polish_dispatch::stream_polish_routed(
                &client_c,
                &route,
                &groq_key,
                &gateway_key,
                &gemini_key,
                &deepinfra_key,
                openai_token_opt.as_deref(),
                &llm_provider_for_task,
                &sys_p,
                &usr_m,
                token_tx,
            )
            .await
        });

        let enforce_roman_hinglish = output_language == "hinglish";
        while let Some(token) = token_rx.recv().await {
            let token = if enforce_roman_hinglish && script::contains_devanagari(&token) {
                script::enforce_roman_hinglish(&token)
            } else {
                token
            };
            yield Ok(Event::default().event("token")
                .data(json!({"token": token}).to_string()));
        }

        let mut llm_result = match llm_task.await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let message = if invalidate_openai_session_on_auth_error(&pool, &user_id, &llm_provider, &e) {
                    "OpenAI not connected — go to Settings to connect your account".to_string()
                } else {
                    e.clone()
                };
                warn!("[voice-repair] LLM error: {e}");
                yield Ok(voice_error_event(message, req.audio_id.as_deref(), None));
                return;
            }
            Err(e) => {
                warn!("[voice-repair] LLM task panicked: {e}");
                yield Ok(voice_error_event(
                    "internal error",
                    req.audio_id.as_deref(),
                    Some("internal_error"),
                ));
                return;
            }
        };

        let scrubbed = strip_confidence_markers(&llm_result.polished);
        if scrubbed != llm_result.polished {
            llm_result.polished = scrubbed;
        }
        let scrubbed = scrub_repair_output(&llm_result.polished, &transcript);
        if scrubbed != llm_result.polished {
            warn!(
                "[voice-repair] scrubbed diagnostic repair output {} → {} chars",
                llm_result.polished.len(),
                scrubbed.len(),
            );
            llm_result.polished = scrubbed;
        }
        if enforce_roman_hinglish && script::contains_devanagari(&llm_result.polished) {
            llm_result.polished = match crate::llm::devanagari_recovery::recover(
                &http_client, &groq_key_for_recovery, &llm_result.polished,
            ).await {
                Ok(recovered) => {
                    info!("[voice-repair] Devanagari LLM recovery succeeded");
                    recovered
                }
                Err(e) => {
                    warn!("[voice-repair] Devanagari LLM recovery failed ({e}) — mechanical fallback");
                    script::enforce_roman_hinglish(&llm_result.polished)
                }
            };
        }

        let repair_transcript_wc = transcript.split_whitespace().count();
        let repair_polished_wc = llm_result.polished.split_whitespace().count();
        if repair_transcript_wc > 4 && repair_polished_wc < repair_transcript_wc / 2 {
            warn!(
                "[voice-repair] short repair output observed but preserved: transcript={} words → polished={} words",
                repair_transcript_wc, repair_polished_wc,
            );
        }

        let total_ms = total_start.elapsed().as_millis() as i64;
        let recording_id = Uuid::new_v4().to_string();
        let word_count = llm_result.polished.split_whitespace().count() as i64;
        {
            let pool2 = pool.clone();
            let id2 = recording_id.clone();
            let uid2 = user_id.clone();
            let t2 = transcript.clone();
            let p2 = llm_result.polished.clone();
            let ta2 = req.target_app.clone();
            let aid2 = req.audio_id.clone();
            let model2 = actual_model_used.clone();
            let p_ms = llm_result.polish_ms as i64;
            let enr2 = req.enriched_transcript.clone();
            tokio::spawn(async move {
                let rec = InsertRecording {
                    id: &id2, user_id: &uid2,
                    transcript: &t2, polished: &p2,
                    word_count, recording_seconds: estimated_secs(word_count),
                    model_used: &model2,
                    confidence: None,
                    transcribe_ms: None,
                    embed_ms: None,
                    polish_ms: Some(p_ms),
                    target_app: ta2.as_deref(),
                    source: "voice_repair",
                    audio_id: aid2.as_deref(),
                    enriched_transcript: enr2.as_deref(),
                    raw_transcript: Some(&t2),
                    local_corrected_transcript: Some(&t2),
                    polished_output: Some(&p2),
                    trace_json: None,
                };
                crate::observability::after_recording_insert(
                    &pool2,
                    &uid2,
                    &rec,
                    crate::observability::observability_extras(None),
                );
                insert_recording(&pool2, rec);
            });
        }

        yield Ok(Event::default().event("done").data(
            json!({
                "recording_id": recording_id,
                "transcript": transcript,
                "polished": llm_result.polished,
                "model_used": actual_model_used,
                "confidence": null,
                "audio_id": req.audio_id,
                "source": "voice_repair",
                "target_app": req.target_app,
                "output_language": output_language,
                "latency_ms": {
                    "transcribe": 0,
                    "embed": 0,
                    "retrieve": 0,
                    "polish": llm_result.polish_ms,
                    "total": total_ms,
                },
                "examples_used": 0,
                "reason": req.reason,
            }).to_string()
        ));
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn run_server_runtime_voice_stream(
    http_client: reqwest::Client,
    pool: crate::store::DbPool,
    user_id: String,
    client_run_id: Option<String>,
    transcript: String,
    output_language: String,
    selected_model: String,
    screen_context: Option<String>,
    vocab_entries: Vec<VocabEntry>,
    target_app: Option<String>,
    recent_speech_hints: Vec<String>,
    token_tx: mpsc::Sender<String>,
) -> Result<(crate::llm::PolishResult, String, ServerRuntimeTraceMeta), String> {
    let setup_start = Instant::now();
    let Some(user) = crate::store::users::get_user(&pool, &user_id) else {
        return Err("local user not found".to_string());
    };
    let token = user
        .cloud_token
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "server runtime requires AirNote sign-in".to_string())?;
    let base_url = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://airnote.emiactech.com")
        .to_string();

    let vocab_cards = server_vocab_cards(&vocab_entries);
    let safe_vocab_terms = vocab_cards
        .iter()
        .map(|card| card.term.clone())
        .collect::<Vec<_>>();

    let req = ServerRuntimeVoiceRequest {
        transcript,
        output_language,
        selected_model,
        screen_context: screen_context.map(|s| s.chars().take(500).collect()),
        safe_vocab_terms,
        vocab_cards,
        recent_speech_hints,
        target_app,
        client_run_id: client_run_id
            .filter(|s| !s.trim().is_empty())
            .or_else(|| Some(Uuid::new_v4().to_string())),
    };

    let url = format!(
        "{}/v1/runtime/voice/polish/stream",
        base_url.trim_end_matches('/')
    );
    let start = Instant::now();
    info!(
        "[voice] server runtime stream start run_id={} url={} transcript_chars={} words={} selected_model={} output_language={} safe_vocab_terms={} vocab_cards={} recent_speech_hints={} target_app={} screen_context_chars={} setup_ms={}",
        req.client_run_id.as_deref().unwrap_or("none"),
        url,
        req.transcript.chars().count(),
        req.transcript.split_whitespace().count(),
        req.selected_model,
        req.output_language,
        req.safe_vocab_terms.len(),
        req.vocab_cards.len(),
        req.recent_speech_hints.len(),
        req.target_app.as_deref().unwrap_or("none"),
        req.screen_context
            .as_ref()
            .map(|s| s.chars().count())
            .unwrap_or(0),
        setup_start.elapsed().as_millis(),
    );

    write_backend_ai_payload_log(&url, &req).await;

    let resp = crate::cp_client::with_org_context(
        http_client
            .post(&url)
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .json(&req)
            .timeout(std::time::Duration::from_secs(30)),
        Some(&user),
    )
    .send()
    .await
    .map_err(|e| format!("server runtime stream request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "server runtime stream returned {status}: {}",
            said_core::text::truncate_utf8(&body, 240)
        ));
    }

    let mut byte_stream = resp.bytes_stream();
    let mut line_buffer = Utf8LineBuffer::default();
    let mut event_name = String::from("message");
    let mut data_lines: Vec<String> = Vec::new();
    let mut parsed_done: Option<ServerRuntimeVoiceResponse> = None;
    let mut token_count = 0usize;
    let mut first_token_ms: Option<u128> = None;

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.map_err(|e| format!("server runtime stream read failed: {e}"))?;

        // HTTP chunks can split a multi-byte UTF-8 character. Decode only
        // after a complete SSE line has arrived.
        for mut line in line_buffer
            .push(&chunk)
            .map_err(|e| format!("server runtime stream contained invalid UTF-8: {e}"))?
        {
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if !data_lines.is_empty() {
                    let data = data_lines.join("\n");
                    match event_name.as_str() {
                        "token" => {
                            let value = serde_json::from_str::<serde_json::Value>(&data)
                                .map_err(|e| format!("server runtime token parse failed: {e}"))?;
                            if let Some(token) = value.get("token").and_then(|v| v.as_str()) {
                                if !token.is_empty() {
                                    first_token_ms
                                        .get_or_insert_with(|| start.elapsed().as_millis());
                                    token_count += 1;
                                    token_tx.send(token.to_string()).await.map_err(|_| {
                                        "server runtime token receiver closed".to_string()
                                    })?;
                                }
                            }
                        }
                        "done" => {
                            parsed_done = Some(
                                serde_json::from_str::<ServerRuntimeVoiceResponse>(&data).map_err(
                                    |e| format!("server runtime done parse failed: {e}"),
                                )?,
                            );
                        }
                        "error" => {
                            let value = serde_json::from_str::<serde_json::Value>(&data)
                                .unwrap_or_else(|_| json!({ "message": data }));
                            let message = value
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("server runtime stream failed");
                            return Err(message.to_string());
                        }
                        "status" | "message" => {}
                        other => {
                            debug!("[voice] ignoring server runtime stream event={other}");
                        }
                    }
                }
                event_name.clear();
                event_name.push_str("message");
                data_lines.clear();
                continue;
            }

            if let Some(rest) = line.strip_prefix("event:") {
                event_name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start().to_string());
            }
        }
    }

    let parsed =
        parsed_done.ok_or_else(|| "server runtime stream ended without done".to_string())?;
    let measured_ms = start.elapsed().as_millis() as u64;
    let server_ms = parsed.latency_ms.total.max(0) as u64;
    let polish_ms = measured_ms.max(server_ms);
    info!(
        "[voice] server runtime stream done run_id={} model={} measured_roundtrip_ms={} server_total_ms={} server_prompt_ms={} server_model_ms={} first_token_ms={:?} tokens={} output_chars={} overhead_ms={}",
        req.client_run_id.as_deref().unwrap_or("none"),
        parsed.model_used,
        measured_ms,
        server_ms,
        parsed.latency_ms.prompt,
        parsed.latency_ms.model,
        first_token_ms,
        token_count,
        parsed.output.chars().count(),
        measured_ms.saturating_sub(server_ms),
    );
    let trace_meta = ServerRuntimeTraceMeta {
        roundtrip_ms: measured_ms,
        server_total_ms: server_ms,
        server_prompt_ms: parsed.latency_ms.prompt,
        server_model_ms: parsed.latency_ms.model,
        first_token_ms,
        token_count,
    };

    Ok((
        crate::llm::PolishResult {
            polished: parsed.output,
            polish_ms,
        },
        format!("server-runtime:{}", parsed.model_used),
        trace_meta,
    ))
}

async fn run_local_voice_polish_no_stream(
    http_client: reqwest::Client,
    pool: crate::store::DbPool,
    user_id: String,
    llm_provider: String,
    selected_model: String,
    gateway_key: String,
    gemini_key: String,
    groq_key: String,
    deepinfra_key: String,
    system_prompt: String,
    user_message: String,
) -> Result<(crate::llm::PolishResult, String), String> {
    let route = crate::llm::polish_dispatch::voice_polish_route(&selected_model);
    let openai_token_opt = if llm_provider == "openai_codex" {
        let pool_tok = pool.clone();
        let uid_tok = user_id.clone();
        let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
            .await
            .unwrap_or(None);
        tok.map(|t| t.access_token)
    } else {
        None
    };

    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
    let drain = tokio::spawn(async move { while token_rx.recv().await.is_some() {} });

    let result = crate::llm::polish_dispatch::stream_polish_routed(
        &http_client,
        &route,
        &groq_key,
        &gateway_key,
        &gemini_key,
        &deepinfra_key,
        openai_token_opt.as_deref(),
        &llm_provider,
        &system_prompt,
        &user_message,
        token_tx,
    )
    .await;

    let _ = drain.await;
    result.map(|r| (r, route.label()))
}

struct PcmWav {
    pcm: Vec<u8>,
    sample_rate: u32,
}

fn extract_pcm16_wav(wav: &[u8]) -> Result<PcmWav, String> {
    if wav.len() < 44 || wav.get(0..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
        return Err("RIFF/WAVE audio is required".to_string());
    }

    let mut offset = 12usize;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut data = None;

    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let size = u32::from_le_bytes([
            wav[offset + 4],
            wav[offset + 5],
            wav[offset + 6],
            wav[offset + 7],
        ]) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| "invalid WAV chunk size".to_string())?;
        if end > wav.len() {
            return Err("invalid WAV chunk length".to_string());
        }

        if id == b"fmt " {
            if size < 16 {
                return Err("invalid WAV fmt chunk".to_string());
            }
            audio_format = Some(u16::from_le_bytes([wav[start], wav[start + 1]]));
            channels = Some(u16::from_le_bytes([wav[start + 2], wav[start + 3]]));
            sample_rate = Some(u32::from_le_bytes([
                wav[start + 4],
                wav[start + 5],
                wav[start + 6],
                wav[start + 7],
            ]));
            bits_per_sample = Some(u16::from_le_bytes([wav[start + 14], wav[start + 15]]));
        } else if id == b"data" {
            data = Some(wav[start..end].to_vec());
        }

        offset = end + (size % 2);
    }

    let channels = channels.ok_or_else(|| "WAV missing channel count".to_string())?;
    let sample_rate = sample_rate.ok_or_else(|| "WAV missing sample rate".to_string())?;
    let bits_per_sample = bits_per_sample.ok_or_else(|| "WAV missing bit depth".to_string())?;
    let audio_format = audio_format.ok_or_else(|| "WAV missing audio format".to_string())?;
    let data = data.ok_or_else(|| "WAV missing data chunk".to_string())?;

    if audio_format != 1 {
        return Err(format!(
            "only PCM WAV is supported, got format {audio_format}"
        ));
    }
    if bits_per_sample != 16 {
        return Err(format!(
            "only 16-bit PCM WAV is supported, got {bits_per_sample}"
        ));
    }
    match channels {
        1 => Ok(PcmWav {
            pcm: data,
            sample_rate,
        }),
        2 => Ok(PcmWav {
            pcm: downmix_stereo_i16_to_mono(&data)?,
            sample_rate,
        }),
        _ => Err(format!(
            "only mono/stereo WAV is supported, got {channels} channels"
        )),
    }
}

fn downmix_stereo_i16_to_mono(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() % 4 != 0 {
        return Err("invalid stereo PCM byte length".to_string());
    }
    let mut out = Vec::with_capacity(data.len() / 2);
    for frame in data.chunks_exact(4) {
        let left = i16::from_le_bytes([frame[0], frame[1]]) as i32;
        let right = i16::from_le_bytes([frame[2], frame[3]]) as i32;
        let mixed = ((left + right) / 2).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        out.extend_from_slice(&mixed.to_le_bytes());
    }
    Ok(out)
}

async fn polish_with_input(state: AppState, input: VoicePolishInput) -> Response {
    let pre_stream_start = Instant::now();
    let VoicePolishInput {
        wav_data,
        target_app,
        pre_transcript,
        pre_transcript_meta,
        repair_mode,
        screen_context,
        message_polish_mode,
        client_run_id,
        client_trace_json,
    } = input;
    let mut dictation_trace =
        said_core::dictation_trace::parse_trace_value(client_trace_json.as_ref())
            .unwrap_or_default();
    dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
        stage: "backend.voice.input",
        component: "backend",
        function: "routes::voice::polish_with_input",
        metadata: json!({
            "wav_bytes": wav_data.len(),
            "pre_transcript_present": pre_transcript.is_some(),
            "message_polish": message_polish_mode,
            "repair_mode": repair_mode.is_some(),
            "screen_context_chars": screen_context.as_ref().map(|s| s.chars().count()).unwrap_or(0),
            "client_run_id": client_run_id.as_deref(),
        }),
        ..Default::default()
    });

    // Allow empty WAV when the caller supplied a pre_transcript (P5 / WS path).
    if wav_data.is_empty() && pre_transcript.is_none() {
        warn!("[voice] received empty audio and no pre_transcript");
        return StatusCode::BAD_REQUEST.into_response();
    }
    info!(
        "[voice] input accepted wav_bytes={} pre_transcript_present={} pre_chars={} pre_words={} message_polish={} repair_mode={} screen_context_chars={} client_run_id={}",
        wav_data.len(),
        pre_transcript.is_some(),
        pre_transcript
            .as_ref()
            .map(|t| t.chars().count())
            .unwrap_or(0),
        pre_transcript
            .as_ref()
            .map(|t| t.split_whitespace().count())
            .unwrap_or(0),
        message_polish_mode,
        repair_mode.is_some(),
        screen_context
            .as_ref()
            .map(|s| s.chars().count())
            .unwrap_or(0),
        client_run_id.as_deref().unwrap_or("none"),
    );

    // Save audio to disk (1-day retention) before exposing audio_id in history.
    // This costs only a few ms, and prevents UI play/download buttons from
    // pointing at a WAV file that failed to save.
    let audio_id = Uuid::new_v4().to_string();
    let save_start = Instant::now();
    let saved_audio_id = if !wav_data.is_empty() {
        let aid = audio_id.clone();
        let data = wav_data.clone();
        match tokio::task::spawn_blocking(move || save_audio(&aid, &data).is_some()).await {
            Ok(true) => Some(audio_id.clone()),
            Ok(false) => {
                warn!("[voice] failed to save audio");
                None
            }
            Err(e) => {
                warn!("[voice] audio save task failed: {e}");
                None
            }
        }
    } else {
        None
    };
    info!(
        "[voice] pre-stream audio save done in {}ms saved_audio={} audio_id={} wav_bytes={}",
        save_start.elapsed().as_millis(),
        saved_audio_id.is_some(),
        saved_audio_id.as_deref().unwrap_or("none"),
        wav_data.len(),
    );

    let audio_secs = wav_duration_secs(&wav_data);
    let voice_run_id = client_run_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let voice_run_mode = if message_polish_mode {
        "message_polish"
    } else if repair_mode.is_some() {
        "repair"
    } else {
        "normal"
    };

    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();
    let voice_run_created = crate::store::voice_runs::create_voice_run_captured(
        &pool,
        crate::store::voice_runs::CapturedVoiceRun {
            run_id: &voice_run_id,
            user_id: &user_id,
            audio_id: saved_audio_id.as_deref(),
            mode: voice_run_mode,
            target_app: target_app.as_deref(),
            wav_bytes: wav_data.len() as i64,
            duration_ms: (audio_secs * 1000.0).round() as i64,
            pre_transcript: pre_transcript.as_deref(),
        },
    )
    .is_some();
    info!(
        "[voice-run] captured run_id={} created={} mode={} audio_id={} wav_bytes={} duration_ms={} pre_transcript_present={}",
        voice_run_id,
        voice_run_created,
        voice_run_mode,
        saved_audio_id.as_deref().unwrap_or("none"),
        wav_data.len(),
        (audio_secs * 1000.0).round() as i64,
        pre_transcript.is_some(),
    );

    let http_client = state.http_client.clone();

    // ── Pre-fetch all DB-backed data in parallel, BEFORE opening the SSE stream ──
    // Prefs (async RwLock), lexicon (async RwLock), and vocab terms (spawn_blocking)
    // run concurrently so total wait ≈ max(each) instead of their sum (~8 ms saved).
    let vocab_task = {
        let pool_c = pool.clone();
        let uid_c = user_id.clone();
        // Load full VocabTerm rows so we can carry example_context into the
        // polish prompt — the foundational signal that lets the LLM do
        // context-aware recognition of unseen STT mishearings.
        tokio::task::spawn_blocking(move || {
            let mut terms = vocabulary::top_terms(&pool_c, &uid_c, 100);
            let company_terms = company_vocab::load_terms(&pool_c, &uid_c, 100);
            for term in company_terms {
                if !terms
                    .iter()
                    .any(|t| t.term.eq_ignore_ascii_case(&term.term))
                {
                    terms.push(term);
                }
            }
            terms
        })
    };
    let prefetch_start = Instant::now();
    let (prefs_opt, (word_corrections, mut stt_replacement_rules), vocab_full) = tokio::join!(
        crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id),
        crate::get_lexicon_cached(&state.lexicon_cache, &pool, &user_id),
        async { vocab_task.await.unwrap_or_default() },
    );
    info!(
        "[voice] pre-stream prefs/lexicon/vocab fetched in {}ms prefs_found={} corrections={} stt_rules={} vocab_terms={}",
        prefetch_start.elapsed().as_millis(),
        prefs_opt.is_some(),
        word_corrections.len(),
        stt_replacement_rules.len(),
        vocab_full.len(),
    );
    let company_aliases = company_vocab::load_aliases(&pool, &user_id);
    for rule in company_aliases {
        if !stt_replacement_rules.iter().any(|r| {
            r.transcript_form
                .eq_ignore_ascii_case(&rule.transcript_form)
        }) {
            stt_replacement_rules.push(rule);
        }
    }
    let Some(prefs_for_guard) = prefs_opt.as_ref() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    info!(
        "[voice] key guard pre_transcript_present={} message_polish={}",
        pre_transcript.is_some(),
        message_polish_mode,
    );
    let missing =
        crate::routes::key_guard::missing_voice_api_keys(&pool, &user_id, prefs_for_guard);
    if !missing.is_empty() {
        let message = "API keys required";
        let payload = voice_error_payload(
            message,
            Some(&voice_run_id),
            saved_audio_id.as_deref(),
            Some("missing_api_keys"),
        );
        let _ = crate::store::voice_runs::mark_voice_run_failed(
            &pool,
            &voice_run_id,
            "missing_api_keys",
            message,
            saved_audio_id.is_some(),
            false,
            Some(&payload),
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error_code": "missing_api_keys",
                "message": message,
                "missing": missing,
                "run_id": voice_run_id,
                "audio_id": saved_audio_id,
                "retryable": saved_audio_id.is_some(),
                "owned_by_airnote": false,
                "diagnostic": payload.get("diagnostic").and_then(Value::as_str).unwrap_or(""),
            })),
        )
            .into_response();
    }
    // The polish-prompt vocab slice is computed below, AFTER the transcript
    // embedding lands, so we can do relevance retrieval.

    // ── Build SSE stream ───────────────────────────────────────────────────────
    let audio_id_ref = saved_audio_id.clone();
    let stream = async_stream::stream! {
        let total_start = Instant::now();
        let aid = audio_id_ref.as_deref();
        let voice_run_id = voice_run_id.clone();
        let processing_attempt = crate::store::voice_runs::mark_voice_run_processing(&pool, &voice_run_id);
        info!(
            "[voice-run] processing run_id={} attempt={} audio_id={}",
            voice_run_id,
            processing_attempt.unwrap_or(0),
            aid.unwrap_or("none"),
        );
        if chaos_voice_fail_after_save_enabled() {
            warn!(
                "[chaos] voice fail-after-save triggered run_id={} audio_id={}",
                voice_run_id,
                aid.unwrap_or("none"),
            );
            yield Ok(voice_run_failed_event(
                &pool,
                &voice_run_id,
                "chaos: failed after audio save",
                aid,
                Some("chaos_after_audio_save"),
            ));
            return;
        }

        let prefs = match prefs_opt {
            Some(p) => p,
            None => {
                yield Ok::<Event, Infallible>(
                    voice_run_failed_event(&pool, &voice_run_id, "preferences not found", aid, Some("preferences_not_found"))
                );
                return;
            }
        };

        let gemini_key = prefs.gemini_api_key.clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        let gateway_key = prefs.gateway_api_key.clone()
            .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
            .or_else(|| { let k = said_core::api_key(); if k.is_empty() { None } else { Some(k.to_string()) } })
            .unwrap_or_default();
        let groq_key = prefs.groq_api_key.clone()
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();
        let deepinfra_key = prefs.deepinfra_api_key.clone()
            .or_else(|| std::env::var("DEEPINFRA_API_KEY").ok())
            .unwrap_or_default();

        info!(
            "[voice] SSE stream start after_pre_stream={}ms selected_model={} output_language={} server_runtime={} wav_bytes={} audio_seconds={:.2} pre_transcript_present={} pre_chars={} pre_words={} message_polish={} client_run_id={}",
            pre_stream_start.elapsed().as_millis(),
            prefs.selected_model,
            prefs.output_language,
            prefs.server_runtime_enabled,
            wav_data.len(),
            audio_secs,
            pre_transcript.is_some(),
            pre_transcript.as_ref().map(|t| t.chars().count()).unwrap_or(0),
            pre_transcript
                .as_ref()
                .map(|t| t.split_whitespace().count())
                .unwrap_or(0),
            message_polish_mode,
            client_run_id.as_deref().unwrap_or("none"),
        );

        // ── Pipeline-start summary ───────────────────────────────────────────────
        let bg_active = crate::BG_TASK_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        info!(
            "[pipeline] start — learning={} vocab={} stt_rules={} bg_tasks={}",
            if prefs.learning_enabled { "ON" } else { "OFF" },
            vocab_full.len(),
            stt_replacement_rules.len(),
            bg_active,
        );

        let Some(local_transcript) = pre_transcript.clone().filter(|t| !t.trim().is_empty()) else {
            yield Ok(voice_run_failed_event(
                &pool,
                &voice_run_id,
                "local speech transcript is required before voice polish",
                aid,
                Some("local_transcript_required"),
            ));
            return;
        };
        let stt_transcript_raw = strip_confidence_markers(&local_transcript);
        if stt_transcript_raw.trim().is_empty() {
            yield Ok(voice_run_failed_event(
                &pool,
                &voice_run_id,
                "no speech detected — try speaking again",
                aid,
                Some("no_speech_detected"),
            ));
            return;
        }
        let word_count = stt_transcript_raw.split_whitespace().count();
        let local_meta = pre_transcript_meta.clone().unwrap_or_else(|| TranscriptMeta {
            enriched_transcript: local_transcript.clone(),
            confidence: 0.95,
            mean_word_confidence: 0.95,
            word_count,
            model: said_core::stt::telemetry_speech_model().to_string(),
            origin: TranscriptOrigin::DictationLocal,
            ..TranscriptMeta::default()
        });
        let enriched_raw = if local_meta.enriched_transcript.trim().is_empty() {
            local_transcript.clone()
        } else {
            local_meta.enriched_transcript.clone()
        };
        let stt_confidence = if local_meta.confidence > 0.0 {
            local_meta.confidence
        } else {
            0.95
        };
        let transcribe_ms = local_meta.duration_ms as i64;
        let audio_seconds = audio_secs;
        info!(
            "[voice] local transcript accepted chars={} words={} confidence={:.2} model={} origin={:?}",
            stt_transcript_raw.chars().count(),
            word_count,
            stt_confidence,
            local_meta.model,
            local_meta.origin,
        );

        if message_polish_mode {
            yield Ok(Event::default().event("status")
                .data(json!({"phase": "message_polishing", "transcript": stt_transcript_raw}).to_string()));

            match crate::routes::message_polish::run_server_message_polish(
                &http_client,
                &pool,
                &user_id,
                &stt_transcript_raw,
                client_run_id.as_deref(),
                "polish",
            ).await {
                Ok((llm_result, model_used)) => {
                    let total_ms = total_start.elapsed().as_millis() as i64;
                    let recording_id = Uuid::new_v4().to_string();
                    let word_count = llm_result.polished.split_whitespace().count() as i64;
                    let audio_secs = wav_duration_secs(&wav_data);

                    let pool2 = pool.clone();
                    let id2 = recording_id.clone();
                    let uid2 = user_id.clone();
                    let t2 = stt_transcript_raw.clone();
                    let p2 = llm_result.polished.clone();
                    let ta2 = target_app.clone();
                    let model2 = model_used.clone();
                    let p_ms = llm_result.polish_ms as i64;
                    let aid2 = saved_audio_id.clone();
                    let crid2 = client_run_id.clone();
                    let run_id2 = voice_run_id.clone();
                    tokio::task::spawn_blocking(move || {
                        let rec = InsertRecording {
                            id: &id2,
                            user_id: &uid2,
                            transcript: &t2,
                            polished: &p2,
                            word_count,
                            recording_seconds: if audio_secs > 0.0 { audio_secs } else { estimated_secs(word_count) },
                            model_used: &model2,
                            confidence: None,
                            transcribe_ms: Some(transcribe_ms),
                            embed_ms: Some(0),
                            polish_ms: Some(p_ms),
                            target_app: ta2.as_deref(),
                            source: "voice",
                            audio_id: aid2.as_deref(),
                            enriched_transcript: Some(&t2),
                            raw_transcript: Some(&t2),
                            local_corrected_transcript: None,
                            polished_output: Some(&p2),
                            trace_json: None,
                        };
                        crate::observability::after_recording_insert(
                            &pool2,
                            &uid2,
                            &rec,
                            crate::observability::observability_extras(crid2.as_deref()),
                        );
                        if insert_recording(&pool2, rec).is_some() {
                            let _ = crate::store::voice_runs::mark_voice_run_completed(
                                &pool2,
                                &run_id2,
                                &id2,
                                None,
                            );
                        }
                    });

                    yield Ok(Event::default().event("done").data(
                        json!({
                            "recording_id": recording_id,
                            "transcript": stt_transcript_raw,
                            "audio_id": saved_audio_id,
                            "source": "voice",
                            "target_app": target_app,
                            "output_language": "english",
                            "polished": llm_result.polished,
                            "model_used": model_used,
                            "confidence": null,
                            "latency_ms": {
                                "transcribe": transcribe_ms,
                                "embed": 0,
                                "retrieve": 0,
                                "polish": llm_result.polish_ms,
                                "total": total_ms,
                            },
                            "examples_used": 0,
                        }).to_string()
                    ));
                }
                Err(e) => {
                    warn!("[voice] server message polish failed: {e}");
                    yield Ok(voice_run_failed_event(&pool, &voice_run_id, e, aid, None));
                }
            }
            return;
        }
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "stt.selected_transcript",
            component: "backend",
            function: "desktop::local_asr",
            output: Some(&stt_transcript_raw),
            duration_ms: Some(transcribe_ms),
            reason: Some("local speech transcript selected for polish"),
            risk: Some("stt_selection"),
            metadata: json!({
                "provider": "local_whisper",
                "model": local_meta.model,
                "origin": format!("{:?}", local_meta.origin),
                "confidence": stt_confidence,
                "audio_seconds": audio_seconds,
            }),
            ..Default::default()
        });

        // Pre-LLM: number normalization + tier2 EVIDENCE COLLECTION (read-only).
        // Tier2 does NOT modify the transcript — it only identifies which tokens
        // might be vocabulary terms. The LLM uses these hints + context to decide
        // what to replace (contextual disambiguation).
        let (stt_transcript, enriched_for_hints, alias_result) = {
            let pool_t = pool.clone();
            let uid_t = user_id.clone();
            let number_t0 = Instant::now();
            let numeric_t = crate::number_format::apply(&stt_transcript_raw);
            let number_ms = number_t0.elapsed().as_millis() as i64;
            let original_transcript = numeric_t.clone();
            let rules_t = stt_replacement_rules.clone();
            let vocab_t = vocab_full.clone();
            if numeric_t != stt_transcript_raw {
                info!(
                    "[voice] deterministic number format before LLM: {:?} → {:?}",
                    stt_transcript_raw, numeric_t
                );
            }
            dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
                stage: "pre_llm.number_format",
                component: "backend",
                function: "number_format::apply",
                input: Some(&stt_transcript_raw),
                output: Some(&numeric_t),
                duration_ms: Some(number_ms),
                reason: Some("normalize spoken numbers before prompt"),
                risk: Some("pre_model_mutation"),
                metadata: json!({}),
                ..Default::default()
            });
            let tier2_t0 = Instant::now();
            let evidence = tokio::task::spawn_blocking(move || {
                crate::tier2::collect_evidence_with_store(
                    &pool_t,
                    &uid_t,
                    &numeric_t,
                    &rules_t,
                    &vocab_t,
                )
            }).await.unwrap_or_else(|e| {
                warn!("[voice] tier2 evidence collection failed: {e}");
                crate::tier2::EvidenceResult {
                    source_text: original_transcript.clone(),
                    evidence: vec![],
                    matches: vec![],
                    traces: vec![],
                }
            });
            let tier2_ms = tier2_t0.elapsed().as_millis() as i64;
            dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
                stage: "pre_llm.tier2_evidence",
                component: "backend",
                function: "tier2::collect_evidence_with_store",
                input: Some(&original_transcript),
                output: Some(&original_transcript),
                duration_ms: Some(tier2_ms),
                reason: Some("collect read-only alias/vocabulary evidence before prompt"),
                risk: Some("prompt_context_bias"),
                metadata: json!({
                    "matches": evidence.matches.len(),
                    "evidence_items": evidence.evidence.len(),
                    "trace_items": evidence.traces.len(),
                }),
                ..Default::default()
            });
            if !evidence.matches.is_empty() {
                info!(
                    "[voice] tier2 evidence (read-only): {} match(es): {}",
                    evidence.matches.len(),
                    evidence.matches
                        .iter()
                        .map(|m| format!("{:?}→{} ({:?})", m.transcript_form, m.correct_form, m.kind))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            // Pass raw transcript (not corrected) to LLM — let LLM disambiguate
            (original_transcript, enriched_raw.clone(), evidence.as_apply_result())
        };

        let status_payload = json!({"phase": "polishing", "transcript": &stt_transcript}).to_string();
        yield Ok(Event::default().event("status").data(status_payload));

        // ── STEP 2: Embed cache lookup only ───────────────────────────────────────
        // Never wait on a fresh Gemini call in the dictation hot path. The desktop's
        // /v1/pre-embed hook populates this cache opportunistically for future runs.
        // The cached vector is only used to narrow vocabulary candidates; full
        // past-edit RAG examples are intentionally not injected into voice prompts.
        let embed_t0 = tokio::time::Instant::now();
        let embedding = gemini::cached(&pool, &stt_transcript).await;
        let embed_ms = embed_t0.elapsed().as_millis() as i64;
        info!("[timing] embed={}ms ({})", embed_ms, if embedding.is_some() { "cache-hit" } else { "cache-miss/nonblocking" });
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "context.embed_cache_lookup",
            component: "backend",
            function: "gemini::cached",
            input: Some(&stt_transcript),
            output: Some(&stt_transcript),
            duration_ms: Some(embed_ms),
            reason: Some("look up cached transcript embedding for vocab relevance without blocking"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "cache_hit": embedding.is_some(),
            }),
            ..Default::default()
        });

        // ── STEP 3: Meaning-first vocabulary cards ───────────────────────────────
        // Retrieve a tiny evidence-backed card set. The retriever never rewrites
        // the transcript and never calls the network; the polish LLM gets soft
        // cards only when the current transcript has sound/meaning support.
        let vocab_t0 = Instant::now();
        let (resolved_transcript, vocab_entries): (String, Vec<VocabEntry>) = {
            let pool_v   = pool.clone();
            let uid_v    = user_id.clone();
            let lang_v   = prefs.output_language.clone();
            let emb_v    = embedding.clone();
            let txt_v = alias_result.text.clone();
            let target_app_v = target_app.clone();
            let screen_context_v = screen_context.clone();
            let cards = tokio::task::spawn_blocking(move || {
                vocab_retrieval::retrieve_after_transcription(
                    &pool_v,
                    VocabRetrievalRequest {
                        user_id: uid_v,
                        transcript: txt_v,
                        output_language: lang_v,
                        target_app: target_app_v,
                        bucket: None,
                        screen_context: screen_context_v,
                        transcript_embedding: emb_v,
                        limit: 8,
                    },
                )
            }).await.unwrap_or_default();

            if cards.is_empty() {
                info!(
                    "[voice] vocab retriever picked 0/{} entries — no transcript evidence",
                    vocab_full.len(),
                );
                (alias_result.text.clone(), vec![])
            } else {
                info!(
                    "[voice] vocab retriever selected {} card(s): {}",
                    cards.len(),
                    cards
                        .iter()
                        .map(|card| {
                            let evidence = card
                                .evidence
                                .iter()
                                .map(|e| format!("{:?}", e.kind))
                                .collect::<Vec<_>>()
                                .join("+");
                            format!("{}[{:.1}:{evidence}]", card.term, card.score)
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                let entries = vocab_retrieval::cards_to_vocab_entries(cards);
                (alias_result.text.clone(), entries)
            }
        };
        let vocab_ms = vocab_t0.elapsed().as_millis() as i64;
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "vocab.retrieve_after_transcription",
            component: "backend",
            function: "vocab_retrieval::retrieve_after_transcription",
            input: Some(&stt_transcript),
            output: Some(&resolved_transcript),
            duration_ms: Some(vocab_ms),
            reason: Some("retrieve meaning-first vocabulary cards for prompt"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "candidate_terms_total": vocab_full.len(),
                "selected_terms": vocab_entries.len(),
                "terms": vocab_entries.iter().take(20).map(|v| v.term.clone()).collect::<Vec<_>>(),
                "evidence": vocab_entries.iter().take(20).map(|v| v.evidence.clone()).collect::<Vec<_>>(),
            }),
            ..Default::default()
        });
        let profile_summary_t0 = Instant::now();
        let client_profile_summary = {
            let pool_profile = pool.clone();
            let uid_profile = user_id.clone();
            tokio::task::spawn_blocking(move || {
                crate::store::profile_summary::ensure_current(&pool_profile, &uid_profile)
            })
            .await
            .unwrap_or(None)
        };
        let profile_summary_ms = profile_summary_t0.elapsed().as_millis() as i64;
        let client_profile_markdown = client_profile_summary
            .as_ref()
            .map(|summary| summary.profile_markdown.as_str());
        let client_profile_version = client_profile_summary.as_ref().map(|summary| summary.version);
        info!(
            "[profile-summary] voice prompt profile version={} chars={} injected={}",
            client_profile_summary
                .as_ref()
                .map(|summary| summary.version)
                .unwrap_or(0),
            client_profile_markdown
                .map(|profile| profile.chars().count())
                .unwrap_or(0),
            client_profile_markdown.is_some(),
        );
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "prompt.profile_summary",
            component: "backend",
            function: "profile_summary::ensure_current",
            input: Some(&resolved_transcript),
            output: Some(&resolved_transcript),
            duration_ms: Some(profile_summary_ms),
            reason: Some("load the local learned profile used as prompt context"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "profile_version": client_profile_version,
                "profile_chars": client_profile_markdown.map(|p| p.chars().count()).unwrap_or(0),
                "injected": client_profile_markdown.is_some(),
            }),
            ..Default::default()
        });
        let recent_hints_t0 = Instant::now();
        let recent_speech_suppressed = !recent_speech_hints_allowed(&vocab_entries);
        let recent_speech_hints = if recent_speech_suppressed {
            Vec::new()
        } else {
            let pool_recent = pool.clone();
            let uid_recent = user_id.clone();
            let run_recent = voice_run_id.clone();
            let app_recent = target_app.clone();
            tokio::task::spawn_blocking(move || {
                let transcripts = crate::store::voice_runs::recent_successful_normal_transcripts_for_app(
                    &pool_recent,
                    &uid_recent,
                    app_recent.as_deref(),
                    &run_recent,
                    crate::store::now_ms(),
                    crate::recent_speech_context::RECENT_SPEECH_TTL_MS,
                    crate::recent_speech_context::RECENT_SPEECH_RUN_LIMIT,
                );
                crate::recent_speech_context::extract_recent_speech_hints(&transcripts)
            })
            .await
            .unwrap_or_default()
        };
        let recent_hints_ms = recent_hints_t0.elapsed().as_millis() as i64;
        if recent_speech_suppressed {
            info!(
                "[voice] recent speech hints suppressed in {}ms app={} reason=no_evidence_backed_vocab",
                recent_hints_ms,
                target_app.as_deref().unwrap_or("none"),
            );
        } else {
            info!(
                "[voice] recent speech hints loaded in {}ms app={} hints={} terms={:?}",
                recent_hints_ms,
                target_app.as_deref().unwrap_or("none"),
                recent_speech_hints.len(),
                recent_speech_hints
            );
        }
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "context.recent_speech_hints",
            component: "backend",
            function: "recent_speech_context::extract_recent_speech_hints",
            input: Some(&resolved_transcript),
            output: Some(&resolved_transcript),
            duration_ms: Some(recent_hints_ms),
            reason: Some("load short-lived same-app terms for spelling disambiguation"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "target_app": target_app.as_deref(),
                "hint_count": recent_speech_hints.len(),
                "hints": &recent_speech_hints,
                "suppressed": recent_speech_suppressed,
                "ttl_ms": crate::recent_speech_context::RECENT_SPEECH_TTL_MS,
                "run_limit": crate::recent_speech_context::RECENT_SPEECH_RUN_LIMIT,
            }),
            ..Default::default()
        });
        let prompt_build_t0 = Instant::now();
        let low_conf = keep_low_confidence_markers(&enriched_for_hints, 80.0);
        let low_conf_ref = if low_conf != resolved_transcript {
            Some(low_conf.as_str())
        } else {
            None
        };
        let user_message = build_user_message_with_hints(
            &resolved_transcript,
            &prefs.output_language,
            low_conf_ref,
        );

        let prompt_body = default_voice_prompt_template();
        let relevant_corrections = crate::store::corrections::filter_relevant(
            &word_corrections, &resolved_transcript, 2, 10,
        );
        let mut base_system_prompt = render_voice_system_prompt_template_with_profile_and_recent(
            &prompt_body,
            &prefs,
            &[],
            &relevant_corrections,
            &vocab_entries,
            client_profile_markdown,
            &recent_speech_hints,
        );
        let prompt_build_ms = prompt_build_t0.elapsed().as_millis() as i64;
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "prompt.build",
            component: "backend",
            function: "render_voice_system_prompt_template_with_profile_and_recent",
            input: Some(&resolved_transcript),
            output: Some(&base_system_prompt),
            duration_ms: Some(prompt_build_ms),
            reason: Some("system prompt rendered with typed compact context"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "profile_version": client_profile_version,
                "profile_chars": client_profile_markdown.map(|p| p.chars().count()).unwrap_or(0),
                "past_edit_examples": 0,
                "past_edit_examples_disabled": true,
                "corrections": relevant_corrections.len(),
                "vocab_entries": vocab_entries.len(),
                "recent_speech_hints": recent_speech_hints.len(),
            }),
            ..Default::default()
        });

        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "prompt.history_examples",
            component: "backend",
            function: "disabled",
            input: Some(&resolved_transcript),
            output: Some(&resolved_transcript),
            duration_ms: Some(0),
            reason: Some("dynamic full-text examples disabled for hallucination resistance"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "fewshot_examples": 0,
                "disabled": true,
            }),
            ..Default::default()
        });

        if llm_debug_enabled() {
            let debug_msg = format!(
                "━━━ LLM INPUT ━━━\ntranscript: {:?}\nvocab_count: {}\ncorrections: {}\n{}{}━━━━━━━━━━━━━━━━━",
                &resolved_transcript,
                vocab_entries.len(),
                relevant_corrections.len(),
                vocab_entries.iter().map(|ve| format!(
                    "  VOCAB: {:?} type={:?} aliases={:?}\n",
                    ve.term,
                    ve.term_type,
                    ve.stt_aliases.iter().map(|(a, c)| format!("{a}({c})")).collect::<Vec<_>>(),
                )).collect::<String>(),
                if vocab_entries.is_empty() { "  *** NO VOCAB IN PROMPT ***\n".to_string() } else { String::new() },
            );
            let vocab_in_prompt = if let Some(start) = base_system_prompt.find("VOCAB:") {
                let end = base_system_prompt[start..].find("\n\n\n").map(|i| start + i).unwrap_or(base_system_prompt.len().min(start + 800));
                &base_system_prompt[start..end]
            } else {
                "*** NO VOCAB BLOCK IN PROMPT ***"
            };
            let full_debug = format!("{debug_msg}\n\nPROMPT VOCAB SECTION:\n{vocab_in_prompt}");
            tracing::debug!("{full_debug}");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true).append(true)
                .open(std::env::temp_dir().join("said-llm-debug.log"))
            {
                use std::io::Write;
                let _ = writeln!(f, "\n{full_debug}");
            }
        }

        let prompt_final_t0 = Instant::now();
        if let Some(ref ctx) = screen_context {
            let block = said_core::polish::prompt::render_screen_context_block(ctx);
            if !block.is_empty() {
                info!(
                    "[voice] screen context: {} chars",
                    ctx.chars().count().min(said_core::polish::prompt::SCREEN_CONTEXT_MAX_CHARS)
                );
                base_system_prompt.push_str(&block);
            }
        }

        let system_prompt = if repair_mode.as_deref() == Some("preserve_recall") {
            format!(
                "{}\n\nREPAIR OVERRIDE:\n- The user explicitly asked to reprocess this recording because the previous output likely missed words or drifted in language.\n- Be extra conservative about deleting words.\n- Prefer keeping uncertain transcript content over compressing it.\n- Preserve numbers, names, acronyms, dates, and mixed Hindi-English spans.",
                base_system_prompt
            )
        } else {
            base_system_prompt
        };
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "prompt.final",
            component: "backend",
            function: "routes::voice::system_prompt",
            input: Some(&resolved_transcript),
            output: Some(&system_prompt),
            duration_ms: Some(prompt_final_t0.elapsed().as_millis() as i64),
            reason: Some("final system prompt sent to polish model"),
            risk: Some("prompt_context_bias"),
            metadata: json!({
                "prompt_chars": system_prompt.chars().count(),
                "screen_context": screen_context.as_ref().is_some_and(|s| !s.trim().is_empty()),
                "repair_mode": repair_mode.as_deref(),
            }),
            ..Default::default()
        });

        // ── STEP 5: LLM polish ───────────────────────────────────────────────────
        let enforce_roman_hinglish = prefs.output_language == "hinglish";
        let llm_start = Instant::now();
        let (llm_result, actual_model_used, server_runtime_trace) = if crate::store::prefs::server_runtime_forced() {
            yield Ok(Event::default().event("status")
                .data(json!({"phase": "server_polishing", "transcript": &resolved_transcript}).to_string()));
            info!("[timing] LLM start — provider=server_runtime selected_model={:?}", prefs.selected_model);
            let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
            let runtime_task = tokio::spawn(run_server_runtime_voice_stream(
                http_client.clone(),
                pool.clone(),
                user_id.clone(),
                client_run_id.clone(),
                resolved_transcript.clone(),
                prefs.output_language.clone(),
                prefs.selected_model.clone(),
                screen_context.clone(),
                vocab_entries.clone(),
                target_app.clone(),
                recent_speech_hints.clone(),
                token_tx,
            ));

            while let Some(raw_token) = token_rx.recv().await {
                yield Ok(Event::default().event("token")
                    .data(json!({"token": raw_token}).to_string()));
            }

            match runtime_task.await {
                Ok(Ok((result, model, trace_meta))) => {
                    info!(
                        "[voice] server runtime stream returned {} chars using {model}",
                        result.polished.len()
                    );
                    (result, model, Some(trace_meta))
                }
                Ok(Err(e)) => {
                    warn!("[voice] server runtime stream failed; falling back to local polish: {e}");
                    yield Ok(Event::default().event("status")
                        .data(json!({"phase": "server_runtime_fallback", "transcript": &resolved_transcript}).to_string()));

                    let fallback_provider = prefs.llm_provider.clone();
                    match run_local_voice_polish_no_stream(
                        http_client.clone(),
                        pool.clone(),
                        user_id.clone(),
                        fallback_provider.clone(),
                        prefs.selected_model.clone(),
                        gateway_key.clone(),
                        gemini_key.clone(),
                        groq_key.clone(),
                        deepinfra_key.clone(),
                        system_prompt.clone(),
                        user_message.clone(),
                    )
                    .await {
                        Ok((result, model)) => {
                            info!(
                                "[voice] server runtime fallback succeeded locally using {model} ({} chars)",
                                result.polished.len()
                            );
                            (
                                result,
                                format!("server-runtime-fallback:{model}"),
                                None,
                            )
                        }
                        Err(local_e) => {
                            let message = if invalidate_openai_session_on_auth_error(
                                &pool,
                                &user_id,
                                &fallback_provider,
                                &local_e,
                            ) {
                                "OpenAI not connected — go to Settings to connect your account"
                                    .to_string()
                            } else {
                                format!(
                                    "server runtime failed ({e}); local fallback failed ({local_e})"
                                )
                            };
                            warn!("[voice] server runtime fallback failed: {local_e}");
                            yield Ok(voice_run_failed_event(&pool, &voice_run_id, message, aid, None));
                            return;
                        }
                    }
                }
                Err(e) => {
                    warn!("[voice] server runtime stream task panicked: {e}");
                    yield Ok(Event::default().event("status")
                        .data(json!({"phase": "server_runtime_fallback", "transcript": &resolved_transcript}).to_string()));

                    let fallback_provider = prefs.llm_provider.clone();
                    match run_local_voice_polish_no_stream(
                        http_client.clone(),
                        pool.clone(),
                        user_id.clone(),
                        fallback_provider.clone(),
                        prefs.selected_model.clone(),
                        gateway_key.clone(),
                        gemini_key.clone(),
                        groq_key.clone(),
                        deepinfra_key.clone(),
                        system_prompt.clone(),
                        user_message.clone(),
                    )
                    .await {
                        Ok((result, model)) => {
                            info!(
                                "[voice] server runtime fallback succeeded locally using {model} ({} chars)",
                                result.polished.len()
                            );
                            (
                                result,
                                format!("server-runtime-fallback:{model}"),
                                None,
                            )
                        }
                        Err(local_e) => {
                            let message = if invalidate_openai_session_on_auth_error(
                                &pool,
                                &user_id,
                                &fallback_provider,
                                &local_e,
                            ) {
                                "OpenAI not connected — go to Settings to connect your account"
                                    .to_string()
                            } else {
                                format!(
                                    "server runtime task failed ({e}); local fallback failed ({local_e})"
                                )
                            };
                            warn!("[voice] server runtime fallback failed: {local_e}");
                            yield Ok(voice_run_failed_event(&pool, &voice_run_id, message, aid, None));
                            return;
                        }
                    }
                }
            }
        } else {
            let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
            let sys_p       = system_prompt.clone();
            let usr_m       = user_message.clone();
            let client_c    = http_client.clone();

            let llm_provider = prefs.llm_provider.clone();
            let route = crate::llm::polish_dispatch::voice_polish_route(&prefs.selected_model);
            let openai_token_opt = if llm_provider == "openai_codex" {
                let pool_tok = pool.clone();
                let uid_tok  = user_id.clone();
                let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
                    .await
                    .unwrap_or(None);
                tok.map(|t| t.access_token)
            } else {
                None
            };
            let llm_provider_for_task = llm_provider.clone();

            let gk          = gateway_key.clone();
            let gk_gemini   = gemini_key.clone();
            let gk_groq     = groq_key.clone();
            let gk_deepinfra = deepinfra_key.clone();

            info!("[timing] LLM start — route={:?}", route.label());
            let actual_model_used = route.label();

            let llm_task = tokio::spawn(async move {
                crate::llm::polish_dispatch::stream_polish_routed(
                    &client_c,
                    &route,
                    &gk_groq,
                    &gk,
                    &gk_gemini,
                    &gk_deepinfra,
                    openai_token_opt.as_deref(),
                    &llm_provider_for_task,
                    &sys_p,
                    &usr_m,
                    token_tx,
                )
                .await
            });

            while let Some(raw_token) = token_rx.recv().await {
                yield Ok(Event::default().event("token")
                    .data(json!({"token": raw_token}).to_string()));
            }

            let llm_result = match llm_task.await {
                Ok(Ok(r))   => r,
                Ok(Err(e))  => {
                    let auth_err = invalidate_openai_session_on_auth_error(&pool, &user_id, &llm_provider, &e);
                    let message = if auth_err {
                        "OpenAI not connected — go to Settings to connect your account".to_string()
                    } else {
                        e.clone()
                    };
                    warn!("[voice] LLM error: {e}");
                    // Raw fallback: for TRANSIENT polish failures (rate-limit / timeout
                    // / overloaded) paste the raw STT transcript rather than DROP the
                    // dictation. Auth/config errors are NOT fallen back — they keep
                    // surfacing so a bad/expired key isn't silently swallowed. The raw
                    // text is script-guarded (no Devanagari leak), and the desktop
                    // reconciles any streamed tokens against this `done`, so there is
                    // no double-typing.
                    let lower = e.to_lowercase();
                    // Gate the raw fallback on the HTTP status: retry only on transient
                    // server/rate codes, NEVER on auth/bad-key (401/403). This avoids
                    // misclassifying a billing/quota 401 whose body mentions "rate" as
                    // transient and silently pasting unpolished text instead of telling
                    // the user to fix their key.
                    let auth_failure = lower.contains("401")
                        || lower.contains("403")
                        || lower.contains("invalid_api_key")
                        || lower.contains("invalid api key")
                        || lower.contains("unauthorized")
                        || lower.contains("forbidden");
                    let transient = !auth_err
                        && !auth_failure
                        && (lower.contains("429")
                            || lower.contains("500")
                            || lower.contains("502")
                            || lower.contains("503")
                            || lower.contains("504")
                            || lower.contains("408")
                            || lower.contains("timeout")
                            || lower.contains("timed out")
                            || lower.contains("overloaded")
                            || lower.contains("temporarily"));
                    let fallback_text = if transient {
                        if enforce_roman_hinglish {
                            let t = if script::contains_devanagari(&resolved_transcript) {
                                script::enforce_roman_hinglish(&resolved_transcript)
                            } else {
                                resolved_transcript.clone()
                            };
                            script::strip_non_latin_scripts(&t)
                        } else {
                            resolved_transcript.clone()
                        }
                    } else {
                        String::new()
                    };
                    if transient && !fallback_text.trim().is_empty() {
                        warn!("[voice] transient polish failure — pasting raw transcript as fallback");
                        let total_ms = total_start.elapsed().as_millis() as i64;
                        let _ = crate::store::voice_runs::mark_voice_run_completed_unlinked(
                            &pool,
                            &voice_run_id,
                        );
                        yield Ok(Event::default().event("done").data(
                            json!({
                                "recording_id": Uuid::new_v4().to_string(),
                                "transcript":   resolved_transcript,
                                "audio_id":     saved_audio_id,
                                "source":       "voice",
                                "target_app":   target_app,
                                "output_language": prefs.output_language,
                                "enriched_transcript": enriched_raw,
                                "polished":     fallback_text,
                                "model_used":   "raw_fallback",
                                "confidence":   stt_confidence,
                                "latency_ms": {
                                    "transcribe": transcribe_ms,
                                    "embed":      embed_ms,
                                    "retrieve":   0,
                                    "polish":     0,
                                    "total":      total_ms,
                                },
                                "examples_used": 0,
                            })
                            .to_string()
                        ));
                    } else {
                        yield Ok(voice_run_failed_event(&pool, &voice_run_id, message, aid, None));
                    }
                    return;
                }
                Err(e) => {
                    warn!("[voice] LLM task panicked: {e}");
                    yield Ok(voice_run_failed_event(&pool, &voice_run_id, "internal error", aid, Some("internal_error")));
                    return;
                }
            };
            (llm_result, actual_model_used, None)
        };

        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "llm.raw_output",
            component: "backend",
            function: "polish_dispatch::stream_polish_routed",
            input: Some(&resolved_transcript),
            output: Some(&llm_result.polished),
            duration_ms: Some(llm_result.polish_ms as i64),
            reason: Some("model output streamed directly to the desktop"),
            risk: Some("model_output"),
            metadata: json!({
                "model": actual_model_used.as_str(),
                "server_runtime": server_runtime_trace.as_ref().map(|m| json!({
                    "roundtrip_ms": m.roundtrip_ms,
                    "server_total_ms": m.server_total_ms,
                    "server_prompt_ms": m.server_prompt_ms,
                    "server_model_ms": m.server_model_ms,
                    "first_token_ms": m.first_token_ms,
                    "token_count": m.token_count,
                })),
            }),
        });

        let llm_ms   = llm_start.elapsed().as_millis() as i64;
        let total_ms = total_start.elapsed().as_millis() as i64;

        let word_count = llm_result.polished.split_whitespace().count() as i64;
        info!("[timing] LLM={}ms (TTFT inside) | total={}ms ← STT={}ms embed={}ms vocab={}ms llm={}ms",
            llm_ms, total_ms, transcribe_ms, embed_ms, vocab_ms, llm_ms);

        let recording_id = Uuid::new_v4().to_string();
        dictation_trace.set_summary_field("model", json!(actual_model_used.as_str()));
        dictation_trace.set_summary_field("recording_id", json!(recording_id.as_str()));
        dictation_trace.set_summary_field("final_output_chars", json!(llm_result.polished.chars().count()));
        let trace_json_string = serde_json::to_string(&dictation_trace).ok();

        // 7. Persist recording before emitting `done`, so the UI refresh that
        // follows the done event can see both the row and its audio_id.
        {
            let pool2   = pool.clone();
            let id2     = recording_id.clone();
            let uid2    = user_id.clone();
            let t2      = resolved_transcript.clone();
            let p2      = llm_result.polished.clone();
            let ta2     = target_app.clone();
            let model2  = actual_model_used.clone();
            let conf    = stt_confidence;
            let t_ms    = transcribe_ms;
            let e_ms    = embed_ms;
            let p_ms    = llm_result.polish_ms as i64;
            let aid2    = saved_audio_id.clone();
            let enr2    = enriched_raw.clone();
            let raw2    = stt_transcript_raw.clone();
            let local2  = llm_result.polished.clone();
            let crid2   = client_run_id.clone();
            let trace2  = trace_json_string.clone();
            let inserted = tokio::task::spawn_blocking(move || {
                let rec = InsertRecording {
                    id: &id2, user_id: &uid2,
                    transcript: &t2, polished: &p2,
                    word_count, recording_seconds: if audio_secs > 0.0 { audio_secs } else { estimated_secs(word_count) },
                    model_used: &model2,
                    confidence:    Some(conf),
                    transcribe_ms: Some(t_ms),
                    embed_ms:      Some(e_ms),
                    polish_ms:     Some(p_ms),
                    target_app:    ta2.as_deref(),
                    source:        "voice",
                    audio_id:      aid2.as_deref(),
                    enriched_transcript: Some(&enr2),
                    raw_transcript: Some(&raw2),
                    local_corrected_transcript: Some(&local2),
                    polished_output: Some(&p2),
                    trace_json: trace2.as_deref(),
                };
                crate::observability::after_recording_insert(
                    &pool2,
                    &uid2,
                    &rec,
                    crate::observability::observability_extras(crid2.as_deref()),
                );
                insert_recording(&pool2, rec).is_some()
            }).await.unwrap_or(false);
            if !inserted {
                warn!("[voice] failed to insert recording history row");
            } else {
                let _ = crate::store::voice_runs::mark_voice_run_completed(
                    &pool,
                    &voice_run_id,
                    &recording_id,
                    None,
                );
            }
            if inserted && !alias_result.traces.is_empty() {
                let pool_policy = pool.clone();
                let uid_policy = user_id.clone();
                let recording_policy = recording_id.clone();
                let result_policy = alias_result.clone();
                tokio::task::spawn_blocking(move || {
                    let n = crate::store::tier2_policy::record_decisions(
                        &pool_policy,
                        &uid_policy,
                        &recording_policy,
                        &result_policy,
                    );
                    if n > 0 {
                        tracing::info!(
                            "[voice] recorded {n} tier2 policy decision event(s) for {recording_policy}"
                        );
                    }
                });
            }
            // Record decision events for exact STT alias matches so that
            // mark_removed_feedback can penalise the alias when the user
            // reverts a wrong replacement.
            if inserted && !alias_result.matches.is_empty() {
                let pool_alias = pool.clone();
                let uid_alias = user_id.clone();
                let recording_alias = recording_id.clone();
                let result_alias = alias_result.clone();
                tokio::task::spawn_blocking(move || {
                    let n = crate::store::tier2_policy::record_applied_matches(
                        &pool_alias,
                        &uid_alias,
                        &recording_alias,
                        &result_alias,
                    );
                    if n > 0 {
                        tracing::info!(
                            "[voice] recorded {n} stt_alias decision event(s) for {recording_alias}"
                        );
                    }
                });
            }

            // Reinforcement-on-use: bump last_used + use_count for vocab
            // terms that were in this polish prompt. This is the "use
            // signal" half of the time-decay scoring — terms that get
            // surfaced AND retained (the polish completed without error)
            // get rewarded, freshening their decay clock and pushing them
            // up the rank for future similar transcripts.
            let pool3  = pool.clone();
            let uid3   = user_id.clone();
            let terms3: Vec<String> = vocab_entries.iter().map(|e| e.term.clone()).collect();
            tokio::task::spawn_blocking(move || {
                vocab_embeddings::bump_last_used(&pool3, &uid3, &terms3);
            });
        }

        yield Ok(Event::default().event("done").data(
            json!({
                "recording_id": recording_id,
                "transcript":   resolved_transcript,
                "audio_id":     saved_audio_id,
                "source":       "voice",
                "target_app":   target_app,
                "output_language": prefs.output_language,
                "enriched_transcript": enriched_raw,
                "polished":     llm_result.polished,
                "model_used":   actual_model_used,
                "confidence":   stt_confidence,
                "latency_ms": {
                    "transcribe": transcribe_ms,
                    "embed":      embed_ms,
                    "retrieve":   0,
                    "polish":     llm_ms,
                    "total":      total_ms,
                },
                "examples_used": 0,
            })
            .to_string()
        ));
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Debug, Clone)]
struct TranscriptCandidate {
    transcript: String,
    meta: TranscriptMeta,
    source: String,
}

fn derive_repair_hints(
    transcript: &str,
    previous_output: &str,
    enriched_transcript: Option<&str>,
    output_language: &str,
) -> Vec<String> {
    let transcript_tokens: Vec<&str> = transcript.split_whitespace().collect();
    let output_tokens: Vec<&str> = previous_output.split_whitespace().collect();
    let mut hints = Vec::new();

    if output_tokens.len() + 2 < transcript_tokens.len() {
        hints.push(format!(
            "The previous output is shorter than the transcript ({} vs {} words); recover omitted content.",
            output_tokens.len(),
            transcript_tokens.len()
        ));
    }

    let transcript_numbers = count_numeric_tokens(transcript);
    let output_numbers = count_numeric_tokens(previous_output);
    if transcript_numbers > output_numbers {
        hints.push("Numbers or dates may have been dropped; preserve them explicitly.".into());
    }

    let overlap = token_overlap_ratio(transcript, previous_output);
    if overlap < 0.7 {
        hints.push(
            "Token overlap with the transcript is low; stay closer to the spoken wording.".into(),
        );
    }

    if output_language == "hinglish" {
        let transcript_hindi = count_hindi_like_tokens(transcript);
        let output_hindi = count_hindi_like_tokens(previous_output);
        if transcript_hindi > output_hindi {
            hints.push("Hindi or Hinglish spans appear to have drifted toward English; preserve the speaker's original mix.".into());
        }
    }

    if enriched_transcript
        .map(|t| t.contains('[') && t.contains('?'))
        .unwrap_or(false)
    {
        hints.push("The transcript had low-confidence spans; preserve uncertain words instead of deleting them.".into());
    }

    hints
}

fn count_numeric_tokens(text: &str) -> usize {
    text.split_whitespace()
        .filter(|token| token.chars().any(|c| c.is_ascii_digit()))
        .count()
}

fn count_hindi_like_tokens(text: &str) -> usize {
    text.split_whitespace()
        .filter(|token| {
            token
                .chars()
                .any(|c| ('\u{0900}'..='\u{097F}').contains(&c))
                || matches!(
                    token.to_ascii_lowercase().as_str(),
                    "hai"
                        | "haan"
                        | "tha"
                        | "thi"
                        | "the"
                        | "nahi"
                        | "nhi"
                        | "kya"
                        | "aur"
                        | "ka"
                        | "ki"
                        | "ke"
                        | "mein"
                        | "me"
                        | "yeh"
                        | "woh"
                        | "kyunki"
                )
        })
        .count()
}

fn token_overlap_ratio(a: &str, b: &str) -> f64 {
    let a_tokens: std::collections::BTreeSet<String> = a
        .split_whitespace()
        .map(normalize_token)
        .filter(|t| !t.is_empty())
        .collect();
    let b_tokens: std::collections::BTreeSet<String> = b
        .split_whitespace()
        .map(normalize_token)
        .filter(|t| !t.is_empty())
        .collect();
    if a_tokens.is_empty() {
        return 1.0;
    }
    let shared = a_tokens.intersection(&b_tokens).count();
    shared as f64 / a_tokens.len() as f64
}

fn normalize_token(token: &str) -> String {
    token
        .trim_matches(|c: char| !c.is_alphanumeric() && !('\u{0900}'..='\u{097F}').contains(&c))
        .to_ascii_lowercase()
}

fn llm_debug_enabled() -> bool {
    std::env::var("SAID_LLM_DEBUG")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn scrub_repair_output(text: &str, transcript: &str) -> String {
    let lower = text.to_ascii_lowercase();
    for marker in [
        "repaired output:",
        "repaired text:",
        "corrected output:",
        "final repaired text:",
        "final output:",
    ] {
        if let Some(pos) = lower.find(marker) {
            let start = pos + marker.len();
            let rest = &text[start..];
            let rest_lower = &lower[start..];
            let end = [
                "explanation:",
                "reasoning:",
                "previous polished output:",
                "original transcript:",
            ]
            .iter()
            .filter_map(|stop| rest_lower.find(stop))
            .min()
            .unwrap_or(rest.len());
            let candidate = rest[..end].trim();
            if !candidate.is_empty() {
                return scrub_polished_output(candidate, transcript, true);
            }
        }
    }
    scrub_polished_output(text, transcript, true)
}

/// Strip high-confidence markers but KEEP markers below `threshold` so the
/// LLM can see which words ASR was unsure about and use context to fix them.
pub fn keep_low_confidence_markers(s: &str, threshold: f64) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '[' {
            let mut inner = String::new();
            let mut found_close = false;
            for ic in chars.by_ref() {
                if ic == ']' {
                    found_close = true;
                    break;
                }
                inner.push(ic);
            }
            if found_close {
                if let Some((word, conf)) = parse_confidence_marker_with_score(&inner) {
                    if conf < threshold {
                        result.push_str(&format!("[{word}?{conf:.0}%]"));
                    } else {
                        result.push_str(&word);
                    }
                    continue;
                }
                result.push('[');
                result.push_str(&inner);
                result.push(']');
            } else {
                result.push('[');
                result.push_str(&inner);
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn parse_confidence_marker_with_score(inner: &str) -> Option<(String, f64)> {
    let trimmed = inner.trim_end();
    let without_pct = trimmed.strip_suffix('%')?.trim_end();
    let mut split_at = without_pct.len();
    for (i, ch) in without_pct.char_indices().rev() {
        if ch.is_ascii_digit() || ch == '.' {
            split_at = i;
        } else {
            break;
        }
    }
    let pct_str = &without_pct[split_at..];
    let score = pct_str.parse::<f64>().ok()?;
    let word_part =
        without_pct[..split_at].trim_end_matches(|c: char| c == '?' || c.is_whitespace());
    if word_part.is_empty() {
        return None;
    }
    Some((word_part.to_string(), score))
}

/// Strip `[word?XX%]`-style confidence markers from a string.
///
/// Used for two purposes:
///   1. Recovering plain text from an enriched STT transcript (where we
///      add the markers ourselves, so the canonical `word?NN%` form is
///      guaranteed).
///   2. Defensive scrubbing of LLM output, where the model occasionally
///      leaks malformed variants like `[main60%]` (no `?`), `[main 60%]`
///      (space), `[main ?60%]`, etc. The lenient parser below catches all
///      of these by detecting the trailing `NN%` or `NN.NN%` shape inside
///      brackets and treating everything before it (after stripping any
///      `?` and whitespace) as the word.
pub fn strip_confidence_markers(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '[' {
            // Collect bracket content
            let mut inner = String::new();
            let mut found_close = false;
            for ic in chars.by_ref() {
                if ic == ']' {
                    found_close = true;
                    break;
                }
                inner.push(ic);
            }
            if found_close {
                if let Some(word) = parse_confidence_marker(&inner) {
                    // Looked like a confidence marker — emit just the word
                    result.push_str(&word);
                    continue;
                }
                // Not a marker — emit brackets + content unchanged
                result.push('[');
                result.push_str(&inner);
                result.push(']');
            } else {
                result.push('[');
                result.push_str(&inner);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// If `inner` (the content between `[` and `]`) looks like a confidence
/// marker — i.e. ends in `NN%` or `NN.NN%` with at least one non-digit
/// character before it — return the cleaned word part. Otherwise None.
///
/// Accepts all of these (canonical + LLM-leaked variants):
///   "main?60%", "main 60%", "main60%", "main ?60%", "main? 60%",
///   "main ? 60 %", "main?60.5%", "मैं?47%"
///
/// Rejects bracket content that doesn't end in `NN%` or has no word part:
///   "x", "see [1]", "60%", "%60", "main"
fn parse_confidence_marker(inner: &str) -> Option<String> {
    let trimmed = inner.trim_end();
    // Must end with '%'
    let without_pct = trimmed.strip_suffix('%')?.trim_end();
    // Last whitespace-separated number is the percentage. Walk backward
    // collecting digits, decimal point, and optional sign — until we hit
    // anything else.
    let mut split_at = without_pct.len();
    for (i, ch) in without_pct.char_indices().rev() {
        if ch.is_ascii_digit() || ch == '.' {
            split_at = i;
        } else {
            break;
        }
    }
    let pct_str = &without_pct[split_at..];
    if pct_str.is_empty() || pct_str.parse::<f64>().is_err() {
        return None;
    }
    // Word part = everything before the percentage, with any '?' and
    // surrounding whitespace stripped.
    let word_part =
        without_pct[..split_at].trim_end_matches(|c: char| c == '?' || c.is_whitespace());
    if word_part.is_empty() {
        return None;
    }
    Some(word_part.to_string())
}

#[cfg(test)]
mod scrub_tests {
    use super::{scrub_repair_output, strip_confidence_markers};

    #[test]
    fn canonical_form_strips_cleanly() {
        // Form we emit ourselves from STT.
        assert_eq!(
            strip_confidence_markers("aaj [kaam?60%] tha"),
            "aaj kaam tha"
        );
        assert_eq!(
            strip_confidence_markers("[main?47%] meeting"),
            "main meeting"
        );
    }

    #[test]
    fn malformed_llm_leaks_get_scrubbed() {
        // The actual user-reported failure: [main60%] with NO question mark.
        assert_eq!(
            strip_confidence_markers("hello [main60%] there"),
            "hello main there"
        );
        // Space instead of '?'
        assert_eq!(strip_confidence_markers("[main 60%] hai"), "main hai");
        // Both space and '?'
        assert_eq!(strip_confidence_markers("[main ?60%] hai"), "main hai");
        assert_eq!(strip_confidence_markers("[main? 60%] hai"), "main hai");
        // Decimal percentage
        assert_eq!(strip_confidence_markers("[main?60.5%] hai"), "main hai");
        // Devanagari word inside marker
        assert_eq!(strip_confidence_markers("[मैं?47%] tired"), "मैं tired");
        // Trailing whitespace inside brackets
        assert_eq!(strip_confidence_markers("[main 60% ] hai"), "main hai");
    }

    #[test]
    fn non_marker_brackets_preserved() {
        // Plain footnote-style — must NOT be scrubbed.
        assert_eq!(
            strip_confidence_markers("see [1] for context"),
            "see [1] for context"
        );
        assert_eq!(strip_confidence_markers("[note]"), "[note]");
        // Bracketed text with no trailing percentage stays.
        assert_eq!(strip_confidence_markers("[hello world]"), "[hello world]");
        // Just a percentage with no word part — keep brackets, not a marker.
        assert_eq!(strip_confidence_markers("[60%]"), "[60%]");
        assert_eq!(strip_confidence_markers("[%60]"), "[%60]");
    }

    #[test]
    fn unclosed_bracket_doesnt_eat_rest_of_string() {
        // If the bracket never closes, emit it as-is — don't gobble the tail.
        assert_eq!(
            strip_confidence_markers("hello [main60% rest"),
            "hello [main60% rest"
        );
    }

    #[test]
    fn multiple_markers_in_one_string() {
        assert_eq!(
            strip_confidence_markers("[hello?80%] [world?70%]"),
            "hello world",
        );
    }

    #[test]
    fn repair_diagnostic_labels_are_scrubbed() {
        let raw = "Previous polished output: Kitna bhi kaam kar lo kuch nahin hone wala bhai.\nRepaired output: Kitna bhi kaam kar lo kuch nahin hone wala hai bhai.\nExplanation: Added missing hai.";
        assert_eq!(
            scrub_repair_output(
                raw,
                "kitanaa bhee kaam kar lo kuch naheen hone vaalaa bhaaee"
            ),
            "Kitna bhi kaam kar lo kuch nahin hone wala hai bhai."
        );
    }
}

// ── WAV header + timing helpers ───────────────────────────────────────────────
//
// These tests cover the pure, side-effect-free math in wav_duration_secs and
// estimated_secs.  They are a reliability safety net: if the byte offsets in the
// WAV header parser drift, these catch it immediately.

#[cfg(test)]
mod recent_speech_guard_tests {
    use super::{VocabEntry, recent_speech_hints_allowed};

    #[test]
    fn recent_speech_hints_need_vocab_evidence() {
        assert!(!recent_speech_hints_allowed(&[]));
        assert!(recent_speech_hints_allowed(&[VocabEntry::from_term(
            "Macobs"
        )]));
    }
}

#[cfg(test)]
mod server_vocab_card_tests {
    use super::{VocabEntry, server_vocab_cards};

    #[test]
    fn rich_retrieval_card_is_preserved_for_server_runtime() {
        let mut entry = VocabEntry::from_term(" Macobs ");
        entry.term_type = Some(" proper_noun ".to_string());
        entry.meaning = Some("Internal onboarding workflow".to_string());
        entry.context = Some("Macobs ka onboarding flow".to_string());
        entry.stt_aliases = vec![("main cops".to_string(), 4)];
        entry.evidence = vec!["phonetic(main cops)".to_string()];
        entry.do_not_use_when = Some("makeup products".to_string());

        let cards = server_vocab_cards(&[entry]);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.term, "Macobs");
        assert_eq!(card.term_type.as_deref(), Some("proper_noun"));
        assert_eq!(
            card.meaning.as_deref(),
            Some("Internal onboarding workflow")
        );
        assert_eq!(card.aliases, ["main cops"]);
        assert_eq!(card.evidence, ["phonetic(main cops)"]);
        assert_eq!(card.do_not_use_when.as_deref(), Some("makeup products"));
    }
}

#[cfg(test)]
mod audio_tests {
    use super::{estimated_secs, extract_pcm16_wav, wav_duration_secs};

    fn pcm_wav(channels: u16, sample_rate: u32, data: &[u8]) -> Vec<u8> {
        let byte_rate = sample_rate * channels as u32 * 2;
        let block_align = channels * 2;
        let chunk_size = 36 + data.len() as u32;
        let mut wav = Vec::with_capacity(44 + data.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&chunk_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(data);
        wav
    }

    /// Buffer shorter than a WAV header (44 bytes) must return 0.0, not panic.
    #[test]
    fn wav_too_short_returns_zero() {
        assert_eq!(wav_duration_secs(&[0u8; 20]), 0.0);
        assert_eq!(wav_duration_secs(&[]), 0.0);
    }

    /// A WAV header where byte_rate is 0 must return 0.0 (no divide-by-zero).
    #[test]
    fn wav_zero_byte_rate_returns_zero() {
        let mut header = [0u8; 44];
        // byte_rate @ offset 28-31: leave as 0x00000000
        // data_size  @ offset 40-43: set to non-zero to confirm byte_rate=0 is the guard
        header[40] = 100;
        assert_eq!(wav_duration_secs(&header), 0.0);
    }

    /// A synthetic 44-byte header with known byte_rate and data_size must give the
    /// correct duration.  16 kHz mono 16-bit PCM: byte_rate = 32000, data = 32000 → 1 s.
    #[test]
    fn wav_valid_header_gives_correct_duration() {
        let mut header = [0u8; 44];
        // byte_rate = 32000 (LE u32) at offset 28
        let byte_rate: u32 = 32_000;
        header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        // data_size = 32000 (LE u32) at offset 40 → duration = 1.0 s
        let data_size: u32 = 32_000;
        header[40..44].copy_from_slice(&data_size.to_le_bytes());

        let dur = wav_duration_secs(&header);
        assert!((dur - 1.0_f64).abs() < 1e-9, "expected 1.0 s, got {dur}");
    }

    /// 3-second clip: data_size = byte_rate * 3
    #[test]
    fn wav_three_second_clip() {
        let mut header = [0u8; 44];
        let byte_rate: u32 = 32_000;
        header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        let data_size: u32 = byte_rate * 3;
        header[40..44].copy_from_slice(&data_size.to_le_bytes());
        assert!((wav_duration_secs(&header) - 3.0_f64).abs() < 1e-9);
    }

    /// 0 words → 0 seconds.
    #[test]
    fn estimated_secs_zero_words() {
        assert_eq!(estimated_secs(0), 0.0);
    }

    /// 130 words → exactly 60 seconds at 130 WPM.
    #[test]
    fn estimated_secs_130_words_is_60s() {
        assert!((estimated_secs(130) - 60.0_f64).abs() < 1e-9);
    }

    /// 65 words → 30 seconds.
    #[test]
    fn estimated_secs_65_words_is_30s() {
        assert!((estimated_secs(65) - 30.0_f64).abs() < 1e-9);
    }

    #[test]
    fn server_ws_wav_parser_extracts_mono_pcm_without_header() {
        let pcm = [1u8, 0, 2, 0, 3, 0, 4, 0];
        let wav = pcm_wav(1, 16_000, &pcm);
        let parsed = extract_pcm16_wav(&wav).expect("valid mono wav");
        assert_eq!(parsed.sample_rate, 16_000);
        assert_eq!(parsed.pcm, pcm);
    }

    #[test]
    fn server_ws_wav_parser_downmixes_stereo_pcm() {
        let mut pcm = Vec::new();
        pcm.extend_from_slice(&1000i16.to_le_bytes());
        pcm.extend_from_slice(&3000i16.to_le_bytes());
        pcm.extend_from_slice(&(-2000i16).to_le_bytes());
        pcm.extend_from_slice(&1000i16.to_le_bytes());
        let wav = pcm_wav(2, 48_000, &pcm);
        let parsed = extract_pcm16_wav(&wav).expect("valid stereo wav");
        assert_eq!(parsed.sample_rate, 48_000);
        assert_eq!(
            parsed.pcm,
            [2000i16.to_le_bytes(), (-500i16).to_le_bytes()].concat()
        );
    }
}

#[cfg(test)]
mod polish_ws_tests {
    use super::{
        PolishWsDeadlines, durable_ws_resume_event, forward_polish_sse_to_ws_with_deadlines,
    };
    use crate::{AppState, store, watchdog};
    use axum::{body::Body, response::Response};
    use bytes::Bytes;
    use futures::{SinkExt, StreamExt};
    use std::{collections::HashMap, sync::Arc, time::Duration};
    use tokio::sync::{Mutex, RwLock};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };

    fn test_state() -> AppState {
        let pool = store::open(&std::env::temp_dir().join(format!(
            "airnote-polish-ws-test-{}.sqlite",
            uuid::Uuid::new_v4()
        )));
        let user_id = store::ensure_default_user(&pool);
        AppState {
            pool,
            shared_secret: Arc::new("test-secret".to_string()),
            default_user_id: Arc::new(user_id),
            prefs_cache: Arc::new(RwLock::new(None)),
            lexicon_cache: Arc::new(RwLock::new(None)),
            live_server_runtime_cache: Arc::new(RwLock::new(HashMap::new())),
            voice_run_hub: Arc::new(Mutex::new(HashMap::new())),
            http_client: reqwest::Client::new(),
            watchdog: Arc::new(watchdog::WatchdogState::new()),
        }
    }

    #[test]
    fn resume_returns_the_persisted_terminal_event_after_restart() {
        let state = test_state();
        let terminal = serde_json::json!({
            "type": "error",
            "protocol_version": 1,
            "run_id": "run-42",
            "seq": 3,
            "payload": { "message": "provider unavailable", "error_code": "upstream" },
        });
        store::voice_runs::create_voice_run_captured(
            &state.pool,
            store::voice_runs::CapturedVoiceRun {
                run_id: "run-42",
                user_id: &state.default_user_id,
                audio_id: None,
                mode: "normal",
                target_app: None,
                wav_bytes: 0,
                duration_ms: 1200,
                pre_transcript: Some("hello"),
            },
        )
        .unwrap();
        store::voice_runs::store_terminal_event(&state.pool, "run-42", &terminal).unwrap();

        assert_eq!(durable_ws_resume_event(&state, "run-42"), terminal);
    }

    #[tokio::test]
    async fn authenticated_socket_handshakes_and_answers_heartbeat() {
        let state = test_state();
        store::users::update_enterprise_auth(
            &state.pool,
            &state.default_user_id,
            "cloud-token",
            "team",
            None,
            Some("https://control.example.test"),
            None,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = crate::router_with_state(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let mut request = format!("ws://{address}/v1/voice/polish/ws")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", "Bearer test-secret".parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();

        let connected = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&connected)
                .unwrap()
                .get("type")
                .and_then(serde_json::Value::as_str),
            Some("polish.connected")
        );

        socket
            .send(Message::Text(
                r#"{"type":"ping","protocol_version":1}"#.into(),
            ))
            .await
            .unwrap();
        let pong = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&pong)
                .unwrap()
                .get("type")
                .and_then(serde_json::Value::as_str),
            Some("pong")
        );
    }

    #[tokio::test]
    async fn silent_upstream_becomes_a_durable_terminal_failure() {
        let state = test_state();
        store::voice_runs::create_voice_run_captured(
            &state.pool,
            store::voice_runs::CapturedVoiceRun {
                run_id: "run-silent",
                user_id: &state.default_user_id,
                audio_id: None,
                mode: "normal",
                target_app: None,
                wav_bytes: 0,
                duration_ms: 500,
                pre_transcript: Some("hello"),
            },
        )
        .unwrap();
        let (sender, mut subscriber) = tokio::sync::broadcast::channel(8);
        state
            .voice_run_hub
            .lock()
            .await
            .insert("run-silent".to_string(), sender.clone());
        let response = Response::new(Body::from_stream(futures::stream::pending::<
            Result<Bytes, std::convert::Infallible>,
        >()));

        forward_polish_sse_to_ws_with_deadlines(
            state.clone(),
            "run-silent".to_string(),
            sender,
            response,
            PolishWsDeadlines {
                first_event: Duration::from_millis(15),
                idle: Duration::from_millis(15),
                total: Duration::from_millis(50),
            },
        )
        .await;

        let event = subscriber.recv().await.unwrap();
        assert_eq!(
            event
                .get("payload")
                .and_then(|payload| payload.get("error_code"))
                .and_then(serde_json::Value::as_str),
            Some("polish_stream_first_event_timeout")
        );
        let run = store::voice_runs::get_voice_run(&state.pool, "run-silent").unwrap();
        assert_eq!(run.status, "failed");
        assert!(run.terminal_event_json.is_some());
        assert!(state.voice_run_hub.lock().await.get("run-silent").is_none());
    }
}
