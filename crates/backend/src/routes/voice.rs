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
    extract::{Multipart, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::StreamExt;
use said_core::{
    text::Utf8LineBuffer,
    transcript::{TranscriptMeta, TranscriptOrigin},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
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
        "dictionary": &req.dictionary,
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
                    "[voice] backend AI payload log wrote path={} run_id={} transcript_chars={} dictionary={}",
                    path.display(),
                    req.client_run_id.as_deref().unwrap_or("none"),
                    req.transcript.chars().count(),
                    req.dictionary.len(),
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
    llm::{
        openai_codex,
        prompt::{build_voice_repair_system_prompt, build_voice_repair_user_message},
        script,
        stream_safety::scrub_polished_output,
    },
    store::{
        history::{InsertRecording, insert_recording},
        openai_oauth,
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
    /// The user's words that occur in this transcript.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dictionary: Vec<said_core::polish::dictation::DictionaryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_app: Option<String>,
    client_run_id: Option<String>,
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
    dictionary: Vec<said_core::polish::dictation::DictionaryEntry>,
    target_app: Option<String>,
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

    let req = ServerRuntimeVoiceRequest {
        transcript,
        output_language,
        selected_model,
        dictionary,
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
        "[voice] server runtime stream start run_id={} url={} transcript_chars={} words={} selected_model={} output_language={} dictionary={} target_app={} setup_ms={}",
        req.client_run_id.as_deref().unwrap_or("none"),
        url,
        req.transcript.chars().count(),
        req.transcript.split_whitespace().count(),
        req.selected_model,
        req.output_language,
        req.dictionary.len(),
        req.target_app.as_deref().unwrap_or("none"),
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
    let prefs_opt = crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id).await;
    if prefs_opt.is_none() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Whisper's transcript is typed as it is; with polish on, the model's reply
    // is typed as it is. Nothing runs on the text in between.
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

        let Some(prefs) = prefs_opt else {
            yield Ok::<Event, Infallible>(
                voice_run_failed_event(&pool, &voice_run_id, "preferences not found", aid, Some("preferences_not_found"))
            );
            return;
        };

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
        let transcript = local_transcript.trim().to_string();
        let local_meta = pre_transcript_meta.clone().unwrap_or_else(|| TranscriptMeta {
            enriched_transcript: local_transcript.clone(),
            confidence: 0.95,
            mean_word_confidence: 0.95,
            word_count: transcript.split_whitespace().count(),
            model: said_core::stt::telemetry_speech_model().to_string(),
            origin: TranscriptOrigin::DictationLocal,
            ..TranscriptMeta::default()
        });
        let stt_transcript_raw = transcript.clone();
        let stt_confidence = if local_meta.confidence > 0.0 {
            local_meta.confidence
        } else {
            0.95
        };
        let transcribe_ms = local_meta.duration_ms as i64;
        info!(
            "[voice] local transcript accepted chars={} words={} confidence={:.2} model={} origin={:?} polish={} message_polish={}",
            transcript.chars().count(),
            transcript.split_whitespace().count(),
            stt_confidence,
            local_meta.model,
            local_meta.origin,
            prefs.polish_enabled,
            message_polish_mode,
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
            output: Some(&transcript),
            duration_ms: Some(transcribe_ms),
            reason: Some("local speech transcript"),
            risk: Some("stt_selection"),
            metadata: json!({
                "provider": "local_whisper",
                "model": local_meta.model,
                "origin": format!("{:?}", local_meta.origin),
                "confidence": stt_confidence,
                "audio_seconds": audio_secs,
            }),
            ..Default::default()
        });
        let dictation = FinishedDictation {
            user_id: user_id.clone(),
            voice_run_id: voice_run_id.clone(),
            client_run_id: client_run_id.clone(),
            transcript: transcript.clone(),
            target_app: target_app.clone(),
            audio_id: saved_audio_id.clone(),
            output_language: prefs.output_language.clone(),
            audio_secs,
            confidence: stt_confidence,
            transcribe_ms,
        };

        if !prefs.polish_enabled {
            info!("[voice] polish off — typing the transcript as spoken");
            let total_ms = total_start.elapsed().as_millis() as i64;
            yield Ok(finish_dictation(&pool, dictation, transcript.clone(), "polish_disabled", 0, total_ms, None).await);
            return;
        }

        let dictionary = {
            let pool_d = pool.clone();
            let uid_d = user_id.clone();
            let text_d = transcript.clone();
            tokio::task::spawn_blocking(move || {
                crate::store::dictionary::for_transcript(&pool_d, &uid_d, &text_d)
            })
            .await
            .unwrap_or_default()
        };
        if !dictionary.is_empty() {
            info!(
                "[voice] word list for this transcript: {}",
                dictionary
                    .iter()
                    .map(|e| match &e.heard {
                        Some(heard) => format!("{heard} → {}", e.written),
                        None => e.written.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }

        yield Ok(Event::default().event("status")
            .data(json!({"phase": "server_polishing", "transcript": &transcript}).to_string()));
        let llm_start = Instant::now();
        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let runtime_task = tokio::spawn(run_server_runtime_voice_stream(
            http_client.clone(),
            pool.clone(),
            user_id.clone(),
            client_run_id.clone(),
            transcript.clone(),
            prefs.output_language.clone(),
            prefs.selected_model.clone(),
            dictionary.clone(),
            target_app.clone(),
            token_tx,
        ));
        while let Some(raw_token) = token_rx.recv().await {
            yield Ok(Event::default().event("token")
                .data(json!({"token": raw_token}).to_string()));
        }

        let outcome = match runtime_task.await {
            Ok(result) => result,
            Err(e) => Err(format!("server polish task failed: {e}")),
        };
        let polish_ms = llm_start.elapsed().as_millis() as i64;
        let (typed, model_used, server_trace) = match outcome {
            Ok((result, model, trace_meta)) => (result.polished, model, Some(trace_meta)),
            Err(e) => {
                // Not polished, so the transcript is what gets typed — the
                // user's words are never dropped because the server failed.
                warn!("[voice] server polish failed; typing the transcript: {e}");
                (transcript.clone(), "polish_failed".to_string(), None)
            }
        };
        dictation_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "llm.raw_output",
            component: "backend",
            function: "routes::voice::run_server_runtime_voice_stream",
            input: Some(&transcript),
            output: Some(&typed),
            duration_ms: Some(polish_ms),
            reason: Some("model output typed as returned"),
            risk: Some("model_output"),
            metadata: json!({
                "model": model_used.as_str(),
                "dictionary": dictionary.len(),
                "server_runtime": server_trace.as_ref().map(|m| json!({
                    "roundtrip_ms": m.roundtrip_ms,
                    "server_total_ms": m.server_total_ms,
                    "server_prompt_ms": m.server_prompt_ms,
                    "server_model_ms": m.server_model_ms,
                    "first_token_ms": m.first_token_ms,
                    "token_count": m.token_count,
                })),
            }),
        });
        let total_ms = total_start.elapsed().as_millis() as i64;
        info!("[timing] polish={}ms total={}ms stt={}ms", polish_ms, total_ms, transcribe_ms);
        let trace_json = serde_json::to_string(&dictation_trace).ok();
        yield Ok(finish_dictation(&pool, dictation, typed, &model_used, polish_ms, total_ms, trace_json).await);
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// What a dictation needs to be saved to History once its text is final.
struct FinishedDictation {
    user_id: String,
    voice_run_id: String,
    client_run_id: Option<String>,
    transcript: String,
    target_app: Option<String>,
    audio_id: Option<String>,
    output_language: String,
    audio_secs: f64,
    confidence: f64,
    transcribe_ms: i64,
}

/// Save the dictation to History, close its voice run, and return the `done`
/// event carrying `typed` — the exact text the desktop types.
async fn finish_dictation(
    pool: &crate::store::DbPool,
    run: FinishedDictation,
    typed: String,
    model_used: &str,
    polish_ms: i64,
    total_ms: i64,
    trace_json: Option<String>,
) -> Event {
    let recording_id = Uuid::new_v4().to_string();
    let word_count = typed.split_whitespace().count() as i64;
    let inserted = {
        let pool = pool.clone();
        let id = recording_id.clone();
        let user_id = run.user_id.clone();
        let transcript = run.transcript.clone();
        let typed = typed.clone();
        let target_app = run.target_app.clone();
        let audio_id = run.audio_id.clone();
        let client_run_id = run.client_run_id.clone();
        let model_used = model_used.to_string();
        let (audio_secs, confidence, transcribe_ms) =
            (run.audio_secs, run.confidence, run.transcribe_ms);
        tokio::task::spawn_blocking(move || {
            let rec = InsertRecording {
                id: &id,
                user_id: &user_id,
                transcript: &transcript,
                polished: &typed,
                word_count,
                recording_seconds: if audio_secs > 0.0 {
                    audio_secs
                } else {
                    estimated_secs(word_count)
                },
                model_used: &model_used,
                confidence: Some(confidence),
                transcribe_ms: Some(transcribe_ms),
                embed_ms: Some(0),
                polish_ms: Some(polish_ms),
                target_app: target_app.as_deref(),
                source: "voice",
                audio_id: audio_id.as_deref(),
                enriched_transcript: Some(&transcript),
                raw_transcript: Some(&transcript),
                local_corrected_transcript: None,
                polished_output: Some(&typed),
                trace_json: trace_json.as_deref(),
            };
            crate::observability::after_recording_insert(
                &pool,
                &user_id,
                &rec,
                crate::observability::observability_extras(client_run_id.as_deref()),
            );
            insert_recording(&pool, rec).is_some()
        })
        .await
        .unwrap_or(false)
    };
    if inserted {
        let _ = crate::store::voice_runs::mark_voice_run_completed(
            pool,
            &run.voice_run_id,
            &recording_id,
            None,
        );
    } else {
        warn!("[voice] failed to insert the dictation into History");
        let _ =
            crate::store::voice_runs::mark_voice_run_completed_unlinked(pool, &run.voice_run_id);
    }

    Event::default().event("done").data(
        json!({
            "recording_id": recording_id,
            "transcript": run.transcript,
            "audio_id": run.audio_id,
            "source": "voice",
            "target_app": run.target_app,
            "output_language": run.output_language,
            "enriched_transcript": run.transcript,
            "polished": typed,
            "model_used": model_used,
            "confidence": run.confidence,
            "latency_ms": {
                "transcribe": run.transcribe_ms,
                "embed": 0,
                "retrieve": 0,
                "polish": polish_ms,
                "total": total_ms,
            },
            "examples_used": 0,
        })
        .to_string(),
    )
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
