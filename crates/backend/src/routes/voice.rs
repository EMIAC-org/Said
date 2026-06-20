//! POST /v1/voice/polish
//!
//! Receives a multipart form with:
//!   audio        — WAV bytes  (required)
//!   target_app   — bundle-id of the focused app  (optional)
//!   pre_transcript — transcript already obtained via Deepgram WS streaming  (optional, P5)
//!
//! Pipeline: auth → load prefs → STT → evidence collection → dynamic prompt →
//!           LLM stream → post-LLM passes → SSE.

const SERVER_STT_PROBE_ENV: &str = "AIRNOTE_ENABLE_SERVER_STT_PROBE";

use axum::{
    Json,
    extract::{Multipart, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use base64::{Engine as _, engine::general_purpose};
use futures::{SinkExt, StreamExt};
use said_core::deepgram::{BiasPackage, TranscriptMeta};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as WsMessage, client::IntoClientRequest},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

fn server_stt_probe_enabled() -> bool {
    matches!(
        std::env::var(SERVER_STT_PROBE_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
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

/// Delete WAV files older than 24 hours. Called from the cleanup task.
pub fn cleanup_old_audio() {
    let dir = audio_dir();
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
            let _ = std::fs::remove_file(entry.path());
            debug!("[voice] deleted old audio {}", entry.path().display());
        }
    }
}

use crate::{
    AppState,
    embedder::gemini,
    llm::{
        cerebras, gateway, gemini_direct, groq, openai_codex,
        prompt::{
            VOICE_PROMPT_BASE_VERSION, VOICE_PROMPT_KIND, VOICE_PROMPT_TITLE, VocabEntry,
            build_user_message_with_hints, build_voice_repair_system_prompt,
            build_voice_repair_user_message, default_voice_prompt_template,
            render_voice_system_prompt_template, resolved_vocab_terms_to_entries_with_aliases,
        },
        script,
        stream_safety::{
            STREAM_RESET_SENTINEL, StreamProvider, StreamSafetyFilter, scrub_polished_output,
        },
        vocab_resolver,
    },
    store::{
        company_vocab, email_memory,
        history::{InsertRecording, insert_recording},
        openai_oauth, prompt_templates, stt_replacements,
        vectors::retrieve_similar,
        vocab_embeddings, vocabulary,
    },
    stt::{bias as stt_bias, deepgram},
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
}

#[derive(Debug, Serialize)]
struct ServerRuntimeVoiceRequest {
    transcript: String,
    output_language: String,
    selected_model: String,
    screen_context: Option<String>,
    safe_vocab_terms: Vec<String>,
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
    total: i64,
}

#[derive(Debug, Serialize)]
struct ServerRuntimeVoiceWavRequest {
    wav_b64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    output_language: String,
    selected_model: String,
    screen_context: Option<String>,
    safe_vocab_terms: Vec<String>,
    client_run_id: Option<String>,
    recording_id: Option<String>,
    platform: Option<String>,
    app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stt_provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServerRuntimeVoiceWavResponse {
    transcript: String,
    output: String,
    model_used: String,
    latency_ms: ServerRuntimeAudioLatency,
}

#[derive(Debug, Deserialize)]
struct ServerRuntimeAudioLatency {
    stt: i64,
    polish: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptPolishRequest {
    transcript: String,
    target_app: Option<String>,
    #[serde(default)]
    pre_transcript_meta: Option<TranscriptMeta>,
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
    let mut wav_data: Vec<u8> = Vec::new();
    let mut target_app: Option<String> = None;
    let mut pre_transcript: Option<String> = None; // P5: from Deepgram WS
    let mut pre_transcript_meta: Option<TranscriptMeta> = None;
    let mut repair_mode: Option<String> = None;
    let mut screen_context: Option<String> = None;
    let mut message_polish_mode = false;
    let mut client_run_id: Option<String> = None;

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
            _ => {}
        }
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
        },
    )
    .await
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
                yield Ok::<Event, Infallible>(Event::default().event("error")
                    .data(json!({"message": "preferences not found", "audio_id": req.audio_id}).to_string()));
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
        let cerebras_key = prefs.cerebras_api_key.clone()
            .or_else(|| std::env::var("CEREBRAS_API_KEY").ok())
            .unwrap_or_default();
        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let sys_p = system_prompt.clone();
        let usr_m = user_message.clone();
        let client_c = http_client.clone();
        let groq_key_for_recovery = groq_key.clone();
        let llm_provider = prefs.llm_provider.clone();
        let (model_for_llm, openai_token_opt) = if llm_provider == "openai_codex" {
            let pool_tok = pool.clone();
            let uid_tok = user_id.clone();
            let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
                .await
                .unwrap_or(None);
            (openai_codex::MODEL_MINI.to_string(), tok.map(|t| t.access_token))
        } else if llm_provider == "gemini_direct" {
            (gemini_direct::GEMINI_DIRECT_MODEL.to_string(), None)
        } else if llm_provider == "groq" {
            (
                if prefs.selected_model == "smart" {
                    groq::GROQ_MODEL_SMART
                } else {
                    groq::GROQ_MODEL_FAST
                }
                .to_string(),
                None,
            )
        } else if llm_provider == "cerebras" {
            (cerebras::CEREBRAS_MODEL_DEFAULT.to_string(), None)
        } else {
            (said_core::resolve_model(&prefs.selected_model).to_string(), None)
        };
        let llm_provider_for_task = llm_provider.clone();
        let actual_model_used = model_for_llm.clone();

        let llm_task = tokio::spawn(async move {
            if llm_provider_for_task == "openai_codex" {
                let access_token = openai_token_opt.as_deref().unwrap_or("");
                if access_token.is_empty() {
                    return Err("OpenAI not connected — go to Settings to connect your account".to_string());
                }
                openai_codex::stream_polish(
                    &client_c, access_token, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else if llm_provider_for_task == "gemini_direct" {
                gemini_direct::stream_polish(
                    &client_c, &gemini_key, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else if llm_provider_for_task == "groq" {
                groq::stream_polish(
                    &client_c, &groq_key, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else if llm_provider_for_task == "cerebras" {
                cerebras::stream_polish(
                    &client_c, &cerebras_key, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else {
                gateway::stream_polish(&client_c, &gateway_key, &model_for_llm, &sys_p, &usr_m, token_tx).await
            }
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
                yield Ok(Event::default().event("error").data(
                    json!({"message": message, "audio_id": req.audio_id}).to_string()
                ));
                return;
            }
            Err(e) => {
                warn!("[voice-repair] LLM task panicked: {e}");
                yield Ok(Event::default().event("error").data(
                    json!({"message": "internal error", "audio_id": req.audio_id}).to_string()
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

        // Content guard: if the LLM dropped more than half the words,
        // fall back to the cleaned transcript.
        let repair_transcript_wc = transcript.split_whitespace().count();
        let repair_polished_wc   = llm_result.polished.split_whitespace().count();
        if repair_transcript_wc > 4 && repair_polished_wc < repair_transcript_wc / 2 {
            let mut cleaned = strip_confidence_markers(&transcript);
            if enforce_roman_hinglish && script::contains_devanagari(&cleaned) {
                cleaned = script::enforce_roman_hinglish(&cleaned);
            }
            warn!(
                "[voice-repair] polish dropped too much content: transcript={} words → polished={} words — falling back to cleaned transcript",
                repair_transcript_wc, repair_polished_wc,
            );
            llm_result.polished = cleaned;
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
                insert_recording(&pool2, InsertRecording {
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
                });
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

async fn run_server_runtime_voice_probe(
    http_client: &reqwest::Client,
    pool: &crate::store::DbPool,
    user_id: &str,
    client_run_id: Option<&str>,
    transcript: &str,
    output_language: &str,
    selected_model: &str,
    screen_context: Option<&str>,
    vocab_entries: &[VocabEntry],
) -> Result<(crate::llm::PolishResult, String), String> {
    let Some(user) = crate::store::users::get_user(pool, user_id) else {
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

    let safe_vocab_terms = vocab_entries
        .iter()
        .map(|entry| entry.term.trim().to_string())
        .filter(|term| !term.is_empty())
        .take(20)
        .collect::<Vec<_>>();

    let req = ServerRuntimeVoiceRequest {
        transcript: transcript.to_string(),
        output_language: output_language.to_string(),
        selected_model: selected_model.to_string(),
        screen_context: screen_context.map(|s| s.chars().take(500).collect()),
        safe_vocab_terms,
        client_run_id: client_run_id
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| Some(Uuid::new_v4().to_string())),
    };

    let url = format!("{}/v1/runtime/voice/polish", base_url.trim_end_matches('/'));
    let start = Instant::now();
    let resp = crate::cp_client::with_org_context(
        http_client
            .post(&url)
            .bearer_auth(token)
            .json(&req)
            .timeout(std::time::Duration::from_secs(30)),
        Some(&user),
    )
    .send()
    .await
    .map_err(|e| format!("server runtime request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "server runtime returned {status}: {}",
            said_core::text::truncate_utf8(&body, 240)
        ));
    }

    let parsed = resp
        .json::<ServerRuntimeVoiceResponse>()
        .await
        .map_err(|e| format!("server runtime response parse failed: {e}"))?;
    let measured_ms = start.elapsed().as_millis() as u64;
    let server_ms = parsed.latency_ms.total.max(0) as u64;
    let polish_ms = measured_ms.max(server_ms);

    Ok((
        crate::llm::PolishResult {
            polished: parsed.output,
            polish_ms,
        },
        format!("server-runtime:{}", parsed.model_used),
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
    cerebras_key: String,
    system_prompt: String,
    user_message: String,
) -> Result<(crate::llm::PolishResult, String), String> {
    let (model_for_llm, openai_token_opt) = if llm_provider == "openai_codex" {
        let pool_tok = pool.clone();
        let uid_tok = user_id.clone();
        let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
            .await
            .unwrap_or(None);
        (
            openai_codex::MODEL_MINI.to_string(),
            tok.map(|t| t.access_token),
        )
    } else if llm_provider == "gemini_direct" {
        (gemini_direct::GEMINI_DIRECT_MODEL.to_string(), None)
    } else if llm_provider == "groq" {
        (
            if selected_model == "smart" {
                groq::GROQ_MODEL_SMART
            } else {
                groq::GROQ_MODEL_FAST
            }
            .to_string(),
            None,
        )
    } else if llm_provider == "cerebras" {
        (cerebras::CEREBRAS_MODEL_DEFAULT.to_string(), None)
    } else {
        (said_core::resolve_model(&selected_model).to_string(), None)
    };

    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
    let drain = tokio::spawn(async move { while token_rx.recv().await.is_some() {} });

    let result = if llm_provider == "openai_codex" {
        let access_token = openai_token_opt.as_deref().unwrap_or("");
        if access_token.is_empty() {
            return Err(
                "OpenAI not connected — go to Settings to connect your account".to_string(),
            );
        }
        openai_codex::stream_polish(
            &http_client,
            access_token,
            &model_for_llm,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await
    } else if llm_provider == "gemini_direct" {
        gemini_direct::stream_polish(
            &http_client,
            &gemini_key,
            &model_for_llm,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await
    } else if llm_provider == "groq" {
        groq::stream_polish(
            &http_client,
            &groq_key,
            &model_for_llm,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await
    } else if llm_provider == "cerebras" {
        cerebras::stream_polish(
            &http_client,
            &cerebras_key,
            &model_for_llm,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await
    } else {
        gateway::stream_polish(
            &http_client,
            &gateway_key,
            &model_for_llm,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await
    };

    let _ = drain.await;
    result.map(|r| (r, model_for_llm))
}

async fn run_server_runtime_voice_wav_probe(
    http_client: &reqwest::Client,
    pool: &crate::store::DbPool,
    user_id: &str,
    wav_data: &[u8],
    output_language: &str,
    selected_model: &str,
    screen_context: Option<&str>,
    safe_vocab_terms: Vec<String>,
    stt_provider: &str,
    recording_id: Option<&str>,
    mode: &str,
    client_run_id: Option<&str>,
) -> Result<
    (
        String,
        crate::llm::PolishResult,
        String,
        ServerRuntimeAudioLatency,
    ),
    String,
> {
    let Some(user) = crate::store::users::get_user(pool, user_id) else {
        return Err("local user not found".to_string());
    };
    let token = user
        .cloud_token
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "server audio runtime requires AirNote sign-in".to_string())?;
    let base_url = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://airnote.emiactech.com")
        .to_string();

    let req = ServerRuntimeVoiceWavRequest {
        wav_b64: general_purpose::STANDARD.encode(wav_data),
        mode: if mode.trim().is_empty() || mode == "normal_voice" {
            None
        } else {
            Some(mode.to_string())
        },
        output_language: output_language.to_string(),
        selected_model: selected_model.to_string(),
        screen_context: screen_context.map(|s| s.chars().take(500).collect()),
        safe_vocab_terms,
        client_run_id: client_run_id
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| Some(Uuid::new_v4().to_string())),
        recording_id: recording_id.map(str::to_string),
        platform: Some(std::env::consts::OS.to_string()),
        app_version: option_env!("CARGO_PKG_VERSION").map(str::to_string),
        stt_provider: Some(stt_provider.to_string()),
    };

    let url = format!("{}/v1/runtime/voice/wav", base_url.trim_end_matches('/'));
    let start = Instant::now();
    let request_timeout = if mode == "message_polish" {
        std::time::Duration::from_secs(90)
    } else {
        std::time::Duration::from_secs(45)
    };
    let resp = crate::cp_client::with_org_context(
        http_client
            .post(&url)
            .bearer_auth(token)
            .json(&req)
            .timeout(request_timeout),
        Some(&user),
    )
    .send()
    .await
    .map_err(|e| format!("server audio runtime request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "server audio runtime returned {status}: {}",
            said_core::text::truncate_utf8(&body, 240)
        ));
    }

    let parsed = resp
        .json::<ServerRuntimeVoiceWavResponse>()
        .await
        .map_err(|e| format!("server audio runtime response parse failed: {e}"))?;
    let measured_ms = start.elapsed().as_millis() as u64;
    let server_ms = parsed.latency_ms.total.max(0) as u64;
    let polish_ms = measured_ms.max(server_ms);

    Ok((
        parsed.transcript,
        crate::llm::PolishResult {
            polished: parsed.output,
            polish_ms,
        },
        if mode == "message_polish" {
            format!("server-message-polish-audio:{}", parsed.model_used)
        } else {
            format!("server-audio-runtime:{}", parsed.model_used)
        },
        parsed.latency_ms,
    ))
}

async fn run_server_runtime_voice_ws_probe(
    pool: &crate::store::DbPool,
    user_id: &str,
    wav_data: &[u8],
    output_language: &str,
    selected_model: &str,
    screen_context: Option<&str>,
    safe_vocab_terms: Vec<String>,
    stt_provider: &str,
) -> Result<
    (
        String,
        crate::llm::PolishResult,
        String,
        ServerRuntimeAudioLatency,
    ),
    String,
> {
    let Some(user) = crate::store::users::get_user(pool, user_id) else {
        return Err("local user not found".to_string());
    };
    let token = user
        .cloud_token
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "server audio runtime requires AirNote sign-in".to_string())?;
    let base_url = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://airnote.emiactech.com")
        .to_string();

    let wav = extract_pcm16_wav(wav_data)?;
    let ws_url = build_server_runtime_ws_url(&base_url, &token);
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| format!("server audio runtime WS URL failed: {e}"))?;
    request.headers_mut().insert(
        "User-Agent",
        "AirNote local backend server-audio-runtime-probe"
            .parse()
            .map_err(|e| format!("server audio runtime WS header failed: {e}"))?,
    );

    let start = Instant::now();
    let (socket, _) = connect_async(request)
        .await
        .map_err(|e| format!("server audio runtime WS connect failed: {e}"))?;
    let (mut sink, mut stream) = socket.split();
    let trace_id = Uuid::new_v4().to_string();
    let start_msg = json!({
        "type": "voice.start",
        "run_id": trace_id,
        "mode": "normal_voice",
        "selected_model": selected_model,
        "output_language": output_language,
        "stt_provider": stt_provider,
        "source": "local_backend_ws_probe",
        "platform": std::env::consts::OS,
        "app_version": option_env!("CARGO_PKG_VERSION"),
        "screen_context": screen_context.map(|s| s.chars().take(500).collect::<String>()),
        "safe_vocab_terms": safe_vocab_terms,
        "audio": {
            "encoding": "linear16",
            "sample_rate": wav.sample_rate,
            "channels": 1,
        }
    });
    sink.send(WsMessage::Text(start_msg.to_string()))
        .await
        .map_err(|e| format!("server audio runtime WS start failed: {e}"))?;

    let frame_bytes = ((wav.sample_rate as usize * 2) / 10).max(2);
    for chunk in wav.pcm.chunks(frame_bytes) {
        sink.send(WsMessage::Binary(chunk.to_vec()))
            .await
            .map_err(|e| format!("server audio runtime WS audio send failed: {e}"))?;
    }
    sink.send(WsMessage::Text(
        json!({"type": "audio.end", "run_id": trace_id}).to_string(),
    ))
    .await
    .map_err(|e| format!("server audio runtime WS end failed: {e}"))?;

    let mut transcript = String::new();
    let mut output = None;
    let mut model_used = "server-audio-runtime:ws".to_string();
    let mut server_latency: Option<ServerRuntimeAudioLatency> = None;
    let first_transcript_deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(20);
    let overall_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(55);
    let mut saw_transcript = false;
    loop {
        let now = tokio::time::Instant::now();
        if now >= overall_deadline {
            return Err(format!(
                "server audio runtime WS timed out waiting for done trace_id={trace_id}"
            ));
        }
        if !saw_transcript && now >= first_transcript_deadline {
            return Err(format!(
                "server audio runtime WS first_transcript_timeout trace_id={trace_id}"
            ));
        }

        let next_deadline = if saw_transcript {
            overall_deadline
        } else if first_transcript_deadline < overall_deadline {
            first_transcript_deadline
        } else {
            overall_deadline
        };
        let remaining = next_deadline.saturating_duration_since(now);
        let maybe_msg = tokio::time::timeout(remaining, stream.next())
            .await
            .map_err(|_| {
                if saw_transcript {
                    format!(
                        "server audio runtime WS timed out waiting for done trace_id={trace_id}"
                    )
                } else {
                    format!("server audio runtime WS first_transcript_timeout trace_id={trace_id}")
                }
            })?;
        let Some(msg) = maybe_msg else {
            break;
        };
        let msg = msg.map_err(|e| format!("server audio runtime WS read failed: {e}"))?;
        let text = match msg {
            WsMessage::Text(text) => text,
            WsMessage::Close(_) => break,
            _ => continue,
        };
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("server audio runtime WS JSON failed: {e}"))?;
        match value.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "transcript.partial" => {
                saw_transcript = true;
            }
            "transcript.final" => {
                saw_transcript = true;
                if let Some(t) = value.get("text").and_then(|v| v.as_str()) {
                    transcript = t.to_string();
                }
            }
            "runtime.done" => {
                output = value
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                server_latency = value.get("latency_ms").cloned().and_then(|latency| {
                    serde_json::from_value::<ServerRuntimeAudioLatency>(latency).ok()
                });
                if let Some(model) = value
                    .get("model_used")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.get("model").and_then(|v| v.as_str()))
                {
                    model_used = format!("server-audio-runtime:{model}");
                }
                break;
            }
            "runtime.error" => {
                let error_kind = value
                    .get("error_kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_runtime_error");
                let status = value.get("status").and_then(|v| v.as_i64()).unwrap_or(0);
                let message = value
                    .get("message")
                    .or_else(|| value.get("error"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("server runtime websocket error");
                return Err(format!(
                    "server audio runtime WS returned {error_kind} status={status} trace_id={trace_id}: {}",
                    said_core::text::truncate_utf8(&message, 240)
                ));
            }
            _ => {}
        }
    }

    let output = output
        .ok_or_else(|| format!("server audio runtime WS ended without done trace_id={trace_id}"))?;
    let measured_total_ms = start.elapsed().as_millis() as i64;
    let latency = server_latency.unwrap_or(ServerRuntimeAudioLatency {
        stt: 0,
        polish: measured_total_ms,
        total: measured_total_ms,
    });
    Ok((
        transcript,
        crate::llm::PolishResult {
            polished: output,
            polish_ms: latency.polish.max(0) as u64,
        },
        model_used,
        latency,
    ))
}

struct PcmWav {
    pcm: Vec<u8>,
    sample_rate: u32,
}

fn build_server_runtime_ws_url(base_url: &str, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{base}")
    };
    format!("{ws_base}/v1/runtime/voice/ws?token={token}")
}

fn extract_pcm16_wav(wav: &[u8]) -> Result<PcmWav, String> {
    if wav.len() < 44 || wav.get(0..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
        return Err("server audio runtime WS requires a RIFF/WAVE file".to_string());
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
    } = input;

    // Allow empty WAV when the caller supplied a pre_transcript (P5 / WS path).
    if wav_data.is_empty() && pre_transcript.is_none() {
        warn!("[voice] received empty audio and no pre_transcript");
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Save audio to disk (1-day retention) before exposing audio_id in history.
    // This costs only a few ms, and prevents UI play/download buttons from
    // pointing at a WAV file that failed to save.
    let audio_id = Uuid::new_v4().to_string();
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

    let audio_secs = wav_duration_secs(&wav_data);

    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();

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
    let (prefs_opt, (word_corrections, mut stt_replacement_rules), vocab_full) = tokio::join!(
        crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id),
        crate::get_lexicon_cached(&state.lexicon_cache, &pool, &user_id),
        async { vocab_task.await.unwrap_or_default() },
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
    let missing = if message_polish_mode {
        Vec::new()
    } else {
        crate::routes::key_guard::missing_voice_api_keys(&pool, &user_id, prefs_for_guard)
    };
    if !missing.is_empty() {
        return crate::routes::key_guard::missing_api_keys_response(missing);
    }
    // The polish-prompt vocab slice is computed below, AFTER the transcript
    // embedding lands, so we can do relevance retrieval.

    // ── Build SSE stream ───────────────────────────────────────────────────────
    let audio_id_ref = saved_audio_id.clone();
    let stream = async_stream::stream! {
        let total_start = Instant::now();
        let aid = audio_id_ref.as_deref();

        let prefs = match prefs_opt {
            Some(p) => p,
            None => {
                yield Ok::<Event, Infallible>(
                    Event::default().event("error").data(
                        json!({"message": "preferences not found", "audio_id": aid}).to_string()
                    )
                );
                return;
            }
        };

        let deepgram_key = said_core::stt::resolve_deepgram_api_key(prefs.deepgram_api_key.as_deref())
            .unwrap_or_default();
        let stt_provider = crate::routes::key_guard::effective_stt_provider(&prefs);
        let stt_api_key = deepgram_key.as_str();
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
        let cerebras_key = prefs.cerebras_api_key.clone()
            .or_else(|| std::env::var("CEREBRAS_API_KEY").ok())
            .unwrap_or_default();

        let stt_bias_package = tokio::task::spawn_blocking({
            let pool = pool.clone();
            let user_id = user_id.clone();
            let language = prefs.language.clone();
            let output_language = prefs.output_language.clone();
            move || stt_bias::build_bias_package(&pool, &user_id, &language, &output_language)
        })
        .await
        .unwrap_or_else(|_| BiasPackage::default());

        // ── Pipeline-start summary ───────────────────────────────────────────────
        let bg_active = crate::BG_TASK_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        info!(
            "[pipeline] start — learning={} vocab={} stt_rules={} keyterms={} replacements={} bg_tasks={}",
            if prefs.learning_enabled { "ON" } else { "OFF" },
            vocab_full.len(),
            stt_replacement_rules.len(),
            stt_bias_package.keyterms.len(),
            stt_bias_package.replacements.len(),
            bg_active,
        );
        if !stt_bias_package.keyterms.is_empty() {
            info!("[pipeline] keyterms={:?}", stt_bias_package.keyterms);
        }

        if message_polish_mode {
            if wav_data.is_empty() {
                yield Ok(Event::default().event("error").data(
                    json!({"message": "no audio captured for message polish mode", "audio_id": aid}).to_string()
                ));
                return;
            }

            yield Ok(Event::default().event("status")
                .data(json!({"phase": "server_transcribing"}).to_string()));

            let server_recording_id = Uuid::new_v4().to_string();
            let stt_provider_for_server = crate::routes::key_guard::effective_stt_provider(&prefs);
            let server_audio_result = run_server_runtime_voice_wav_probe(
                &http_client,
                &pool,
                &user_id,
                &wav_data,
                "english",
                &prefs.selected_model,
                screen_context.as_deref(),
                Vec::new(),
                &stt_provider_for_server,
                Some(&server_recording_id),
                "message_polish",
                client_run_id.as_deref(),
            )
            .await;

            match server_audio_result {
                Ok((server_transcript, server_result, server_model, server_latency)) => {
                    let total_ms = total_start.elapsed().as_millis() as i64;
                    let word_count = server_result.polished.split_whitespace().count() as i64;
                    let audio_secs = wav_duration_secs(&wav_data);

                    let pool2 = pool.clone();
                    let id2 = server_recording_id.clone();
                    let uid2 = user_id.clone();
                    let t2 = server_transcript.clone();
                    let p2 = server_result.polished.clone();
                    let ta2 = target_app.clone();
                    let model2 = server_model.clone();
                    let aid2 = saved_audio_id.clone();
                    tokio::task::spawn_blocking(move || {
                        insert_recording(&pool2, InsertRecording {
                            id: &id2,
                            user_id: &uid2,
                            transcript: &t2,
                            polished: &p2,
                            word_count,
                            recording_seconds: if audio_secs > 0.0 { audio_secs } else { estimated_secs(word_count) },
                            model_used: &model2,
                            confidence: None,
                            transcribe_ms: Some(server_latency.stt),
                            embed_ms: Some(0),
                            polish_ms: Some(server_latency.polish),
                            target_app: ta2.as_deref(),
                            source: "server_message_polish_audio",
                            audio_id: aid2.as_deref(),
                            enriched_transcript: Some(&t2),
                            raw_transcript: Some(&t2),
                            local_corrected_transcript: None,
                            polished_output: Some(&p2),
                        });
                    });

                    yield Ok(Event::default().event("done").data(
                        json!({
                            "recording_id": server_recording_id,
                            "transcript": server_transcript,
                            "audio_id": saved_audio_id,
                            "source": "server_message_polish_audio",
                            "target_app": target_app,
                            "output_language": "english",
                            "polished": server_result.polished,
                            "model_used": server_model,
                            "confidence": null,
                            "latency_ms": {
                                "transcribe": server_latency.stt,
                                "embed": 0,
                                "retrieve": 0,
                                "polish": server_latency.polish,
                                "total": total_ms,
                            },
                            "examples_used": 0,
                            "server_message_polish_audio": true,
                        }).to_string()
                    ));
                    return;
                }
                Err(e) => {
                    warn!("[voice] server message-polish audio failed: {e}");
                    if !crate::routes::key_guard::missing_message_polish_voice_keys(&prefs).is_empty() {
                        yield Ok(Event::default().event("error").data(
                            json!({"message": e, "audio_id": aid}).to_string()
                        ));
                        return;
                    }
                    warn!("[voice] falling back to local STT + server message polish");
                }
            }

            yield Ok(Event::default().event("status")
                .data(json!({"phase": "transcribing"}).to_string()));

            let stt_result = run_batch_transcript(
                &http_client,
                &stt_provider,
                stt_api_key,
                wav_data.clone(),
                stt_bias_package.clone(),
                "message_polish:batch".to_string(),
            ).await;

            let (stt_transcript_raw, transcribe_ms) = match stt_result {
                Ok(candidate) => {
                    let ms = total_start.elapsed().as_millis() as i64;
                    info!(
                        "[voice] message-polish batch STT={}ms ({} words)",
                        ms,
                        candidate.meta.word_count,
                    );
                    (candidate.transcript, ms)
                }
                Err(e) => {
                    warn!("[voice] message-polish batch STT error: {e}");
                    yield Ok(Event::default().event("error").data(
                        json!({"message": e, "audio_id": aid}).to_string()
                    ));
                    return;
                }
            };

            if stt_transcript_raw.trim().is_empty() {
                yield Ok(Event::default().event("error").data(
                    json!({"message": "no speech detected — try speaking again", "audio_id": aid}).to_string()
                ));
                return;
            }

            yield Ok(Event::default().event("status")
                .data(json!({"phase": "message_polishing", "transcript": stt_transcript_raw}).to_string()));

            match crate::routes::message_polish::run_server_message_polish(
                &http_client,
                &pool,
                &user_id,
                &stt_transcript_raw,
                client_run_id.as_deref(),
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
                    tokio::task::spawn_blocking(move || {
                        insert_recording(&pool2, InsertRecording {
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
                            enriched_transcript: None,
                            raw_transcript: Some(&t2),
                            local_corrected_transcript: None,
                            polished_output: Some(&p2),
                        });
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
                    yield Ok(Event::default().event("error").data(
                        json!({"message": e, "audio_id": aid}).to_string()
                    ));
                }
            }
            return;
        }

        if prefs.server_audio_runtime_enabled
            && server_stt_probe_enabled()
            && !wav_data.is_empty()
            && !message_polish_mode
            && repair_mode.is_none()
        {
            let recording_id = Uuid::new_v4().to_string();
            let server_audio_transport = std::env::var("AIRNOTE_SERVER_AUDIO_RUNTIME_TRANSPORT")
                .unwrap_or_else(|_| "http".to_string())
                .trim()
                .to_ascii_lowercase();
            let live_cached_result = if server_audio_transport == "ws" {
                match client_run_id.as_deref() {
                    Some(run_id) => {
                        crate::take_live_server_runtime_result(&state.live_server_runtime_cache, run_id)
                            .await
                    }
                    None => None,
                }
            } else {
                None
            };
            if live_cached_result.is_none() {
                yield Ok(Event::default().event("status")
                    .data(json!({"phase": "server_transcribing"}).to_string()));
            }

            let server_audio_result = if let Some(cached) = live_cached_result {
                info!(
                    "[voice] using cached live server runtime result run_id={:?} transcript={} chars output={} chars",
                    client_run_id,
                    cached.transcript.len(),
                    cached.output.len(),
                );
                Ok((
                    cached.transcript,
                    crate::llm::PolishResult {
                        polished: cached.output,
                        polish_ms: cached.latency_ms.polish.max(0) as u64,
                    },
                    format!("server-audio-runtime-live:{}", cached.model_used),
                    ServerRuntimeAudioLatency {
                        stt: cached.latency_ms.stt,
                        polish: cached.latency_ms.polish,
                        total: cached.latency_ms.total,
                    },
                ))
            } else {
                let safe_vocab_terms = vocab_full
                    .iter()
                    .map(|term| term.term.trim().to_string())
                    .filter(|term| !term.is_empty())
                    .take(30)
                    .collect::<Vec<_>>();
                let stt_provider = crate::routes::key_guard::effective_stt_provider(&prefs);
                if server_audio_transport == "ws" {
                    run_server_runtime_voice_ws_probe(
                        &pool,
                        &user_id,
                        &wav_data,
                        &prefs.output_language,
                        &prefs.selected_model,
                        screen_context.as_deref(),
                        safe_vocab_terms,
                        &stt_provider,
                    )
                    .await
                } else {
                    run_server_runtime_voice_wav_probe(
                        &http_client,
                        &pool,
                        &user_id,
                        &wav_data,
                        &prefs.output_language,
                        &prefs.selected_model,
                        screen_context.as_deref(),
                        safe_vocab_terms,
                        &stt_provider,
                        Some(&recording_id),
                        "normal_voice",
                        client_run_id.as_deref(),
                    )
                    .await
                }
            };

            match server_audio_result
            {
                Ok((server_transcript, server_result, server_model, server_latency)) => {
                    let total_ms = total_start.elapsed().as_millis() as i64;
                    let server_source = if server_model.starts_with("server-audio-runtime-live:") {
                        "server_audio_runtime_live"
                    } else {
                        "server_audio_runtime"
                    };
                    info!(
                        "[voice] server audio runtime returned transcript={} chars output={} chars model={} total={}ms",
                        server_transcript.len(),
                        server_result.polished.len(),
                        server_model,
                        server_latency.total,
                    );

                    let recording_id_for_store = recording_id.clone();
                    let transcript_for_store = server_transcript.clone();
                    let polished_for_store = server_result.polished.clone();
                    let model_for_store = server_model.clone();
                    let target_app_for_store = target_app.clone();
                    let audio_id_for_store = saved_audio_id.clone();
                    let pool_store = pool.clone();
                    let user_id_store = user_id.clone();
                    let word_count = polished_for_store.split_whitespace().count() as i64;
                    tokio::spawn(async move {
                        insert_recording(&pool_store, InsertRecording {
                            id: &recording_id_for_store,
                            user_id: &user_id_store,
                            transcript: &transcript_for_store,
                            polished: &polished_for_store,
                            word_count,
                            recording_seconds: estimated_secs(word_count),
                            model_used: &model_for_store,
                            confidence: None,
                            transcribe_ms: Some(server_latency.stt),
                            embed_ms: None,
                            polish_ms: Some(server_latency.polish),
                            target_app: target_app_for_store.as_deref(),
                            source: server_source,
                            audio_id: audio_id_for_store.as_deref(),
                            enriched_transcript: Some(&transcript_for_store),
                            raw_transcript: Some(&transcript_for_store),
                            local_corrected_transcript: Some(&transcript_for_store),
                            polished_output: Some(&polished_for_store),
                        });
                    });

                    yield Ok(Event::default().event("done").data(
                        json!({
                            "recording_id": recording_id,
                            "transcript": server_transcript,
                            "polished": server_result.polished,
                            "model_used": server_model,
                            "confidence": null,
                            "audio_id": aid,
                            "source": server_source,
                            "target_app": target_app,
                            "output_language": prefs.output_language,
                            "latency_ms": {
                                "transcribe": server_latency.stt,
                                "embed": 0,
                                "retrieve": 0,
                                "polish": server_result.polish_ms,
                                "total": total_ms,
                            },
                            "examples_used": 0,
                            "server_audio_runtime": true,
                        }).to_string()
                    ));
                    return;
                }
                Err(e) => {
                    warn!("[voice] server audio runtime failed; falling back to local pipeline: {e}");
                    yield Ok(Event::default().event("status")
                        .data(json!({"phase": "server_audio_fallback"}).to_string()));
                }
            }
        }
        if prefs.server_audio_runtime_enabled && !server_stt_probe_enabled() {
            debug!(
                "[voice] server audio runtime probe disabled; using local Deepgram STT + polish path"
            );
        }

        // ── STEP 1: STT ───────────────────────────────────────────────────────────
        info!("[voice] stt_provider={stt_provider:?}");
        let audio_seconds = wav_duration_seconds(&wav_data);
        let use_alt_stt = said_core::stt::use_batch_stt_only(&stt_provider);
        let pre_transcript = if use_alt_stt { None } else { pre_transcript };
        let (stt_transcript_raw, enriched_raw, stt_confidence, transcribe_ms) = if let Some(t) = pre_transcript {
            let plain = strip_confidence_markers(&t);
            let ws_meta = pre_transcript_meta.unwrap_or_else(|| TranscriptMeta {
                enriched_transcript: t.clone(),
                confidence: 0.95,
                mean_word_confidence: 0.95,
                word_count: plain.split_whitespace().count(),
                stt_mode: stt_bias_package.stt_mode.clone(),
                ..TranscriptMeta::default()
            });
            let primary = TranscriptCandidate {
                transcript: plain,
                meta: TranscriptMeta {
                    enriched_transcript: t.clone(),
                    ..ws_meta.clone()
                },
                source: "ws".to_string(),
            };
            let (chosen, rescue_ms) = match maybe_rescue_transcript(
                &http_client,
                &stt_provider,
                stt_api_key,
                wav_data.clone(),
                audio_seconds,
                &stt_bias_package,
                Some(primary),
            )
            .await {
                Ok(v) => v,
                Err(e) => {
                    warn!("[voice] STT error: {e}");
                    yield Ok(Event::default().event("error").data(
                        json!({"message": e, "audio_id": aid}).to_string()
                    ));
                    return;
                }
            };
            let ms = total_start.elapsed().as_millis() as i64;
            info!(
                "[timing] STT={}ms (WS pre-transcript{} {} words)",
                ms,
                if rescue_ms > 0 { " + rescue" } else { "" },
                chosen.meta.word_count,
            );
            (
                chosen.transcript,
                chosen.meta.enriched_transcript.clone(),
                chosen.meta.confidence,
                ms,
            )
        } else {
            yield Ok(Event::default().event("status")
                .data(json!({"phase": "transcribing"}).to_string()));

            #[cfg(feature = "local-stt")]
            let use_whisper = stt_provider == "whisper_local";
            #[cfg(not(feature = "local-stt"))]
            let use_whisper = false;

            if stt_provider == "groq_whisper" {
                match crate::stt::groq_whisper::transcribe(
                    &http_client,
                    &groq_key,
                    wav_data.clone(),
                    &prefs.language,
                ).await {
                    Ok(result) => {
                        let ms = total_start.elapsed().as_millis() as i64;
                        info!(
                            "[timing] STT={}ms (groq_whisper, {} words, conf={:.2})",
                            ms, result.word_count, result.confidence,
                        );
                        (
                            result.transcript,
                            result.enriched_transcript,
                            result.confidence,
                            ms,
                        )
                    }
                    Err(e) => {
                        warn!("[voice] groq whisper STT error: {e}");
                        yield Ok(Event::default().event("error").data(
                            json!({"message": e, "audio_id": aid}).to_string()
                        ));
                        return;
                    }
                }
            } else if use_whisper {
                #[cfg(feature = "local-stt")]
                {
                    let wav_c = wav_data.clone();
                    let lang_c = prefs.language.clone();
                    let whisper_result = tokio::task::spawn_blocking(move || {
                        crate::stt::whisper::transcribe_wav(&wav_c, &lang_c)
                    }).await;
                    match whisper_result {
                        Ok(Ok(result)) => {
                            let ms = total_start.elapsed().as_millis() as i64;
                            info!(
                                "[timing] STT={}ms (whisper_local, {} words)",
                                ms, result.word_count,
                            );
                            (
                                result.transcript,
                                result.enriched_transcript,
                                result.confidence,
                                ms,
                            )
                        }
                        Ok(Err(e)) => {
                            warn!("[voice] whisper STT error: {e}");
                            yield Ok(Event::default().event("error").data(
                                json!({"message": e, "audio_id": aid}).to_string()
                            ));
                            return;
                        }
                        Err(e) => {
                            warn!("[voice] whisper task panicked: {e}");
                            yield Ok(Event::default().event("error").data(
                                json!({"message": format!("{e}"), "audio_id": aid}).to_string()
                            ));
                            return;
                        }
                    }
                }
                #[cfg(not(feature = "local-stt"))]
                unreachable!()
            } else {
                match maybe_rescue_transcript(
                    &http_client,
                    &stt_provider,
                    stt_api_key,
                    wav_data.clone(),
                    audio_seconds,
                    &stt_bias_package,
                    None,
                )
                .await {
                    Ok((chosen, _rescue_ms)) => {
                        let ms = total_start.elapsed().as_millis() as i64;
                        info!(
                            "[timing] STT={}ms ({}, {} words, conf={:.2})",
                            ms,
                            chosen.source,
                            chosen.meta.word_count,
                            chosen.meta.confidence
                        );
                        (
                            chosen.transcript,
                            chosen.meta.enriched_transcript.clone(),
                            chosen.meta.confidence,
                            ms,
                        )
                    }
                    Err(e) => {
                        warn!("[voice] STT error: {e}");
                        yield Ok(Event::default().event("error").data(
                            json!({"message": e, "audio_id": aid}).to_string()
                        ));
                        return;
                    }
                }
            }
        };

        // Pre-LLM: number normalization + tier2 EVIDENCE COLLECTION (read-only).
        // Tier2 does NOT modify the transcript — it only identifies which tokens
        // might be vocabulary terms. The LLM uses these hints + context to decide
        // what to replace (contextual disambiguation).
        let (stt_transcript, enriched_for_hints, alias_result) = {
            let pool_t = pool.clone();
            let uid_t = user_id.clone();
            let numeric_t = crate::number_format::apply(&stt_transcript_raw);
            let original_transcript = numeric_t.clone();
            let rules_t = stt_replacement_rules.clone();
            let vocab_t = vocab_full.clone();
            if numeric_t != stt_transcript_raw {
                info!(
                    "[voice] deterministic number format before LLM: {:?} → {:?}",
                    stt_transcript_raw, numeric_t
                );
            }
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
        let embed_t0 = tokio::time::Instant::now();
        let embedding = gemini::cached(&pool, &stt_transcript).await;
        let embed_ms = embed_t0.elapsed().as_millis() as i64;
        info!("[timing] embed={}ms ({})", embed_ms, if embedding.is_some() { "cache-hit" } else { "cache-miss/nonblocking" });

        // ── STEP 3: RAG retrieval — k-NN over preference_vectors ──────────────────
        let rag_examples = match &embedding {
            Some(emb) => {
                let emb_clone = emb.clone();
                let pool_rag  = pool.clone();
                let uid_rag   = user_id.clone();
                tokio::task::spawn_blocking(move || {
                    retrieve_similar(&pool_rag, &uid_rag, &emb_clone, 5, 0.65)
                }).await.unwrap_or_default()
            }
            None => vec![],
        };
        let rag_ms: u128 = 0; // included in embed_ms above
        let examples_used = rag_examples.len();
        info!("[rag] {} example(s) retrieved", examples_used);

        // ── STEP 4: Relevance-aware vocabulary slice ──────────────────────────────
        // Use the transcript embedding to pick the vocab entries that match
        // what the user actually said. Skip flooding the prompt with all 200
        // vocab rows — pick starred + top-weight + top-relevance (deduped,
        // capped at 25). Falls back to starred + top-weight when no embedding.
        let (resolved_transcript, vocab_entries): (String, Vec<VocabEntry>) = {
            let pool_v   = pool.clone();
            let uid_v    = user_id.clone();
            let lang_v   = prefs.output_language.clone();
            let emb_v    = embedding.clone();
            let txt_v = alias_result.text.clone();
            let mut chosen = tokio::task::spawn_blocking(move || {
                vocab_embeddings::select_for_prompt(
                    &pool_v, &uid_v, &lang_v, emb_v.as_deref(), Some(&txt_v),
                )
            }).await.unwrap_or_default();
            // Company terms are not embedded in the local personal-vector index.
            // Include the highest-priority company entries in the resolver
            // candidate set so fresh enterprise installs get day-one value.
            for term in vocab_full.iter().filter(|t| t.source == "company") {
                if chosen.len() >= 25 {
                    break;
                }
                if !chosen.iter().any(|t| t.term.eq_ignore_ascii_case(&term.term)) {
                    chosen.push(term.clone());
                }
            }
            // Load safe STT aliases for prompt rendering. These are displayed
            // only for terms the resolver admits below; Tier 2 now carries
            // protected-term evidence through polish and mutates only at the end.
            let alias_map: std::collections::HashMap<String, Vec<(String, i64)>> = {
                let mut map: std::collections::HashMap<String, Vec<(String, i64)>> =
                    std::collections::HashMap::new();
                for rule in &stt_replacement_rules {
                    if stt_replacements::is_plausible_alias(&rule.transcript_form, &rule.correct_form) {
                        map.entry(rule.correct_form.to_lowercase())
                            .or_default()
                            .push((rule.transcript_form.clone(), rule.use_count));
                    }
                }
                map
            };

            if chosen.is_empty() {
                info!(
                    "[voice] vocab selector picked 0/{} entries — no transcript evidence",
                    vocab_full.len(),
                );
                (alias_result.text.clone(), vec![])
            } else {
                let resolve_t0 = Instant::now();
                let resolved = vocab_resolver::resolve_for_prompt(
                    &alias_result.text,
                    &chosen,
                    &vocab_full,
                    &alias_result,
                );
                let resolve_ms = resolve_t0.elapsed().as_millis() as i64;
                info!(
                    "[voice] vocab resolver={}ms alias_matches={} context_matches={} resolved={} candidates={}",
                    resolve_ms,
                    resolved.alias_match_count,
                    resolved.context_match_count,
                    resolved.resolved_terms.len(),
                    resolved.candidate_terms.len(),
                );
                let entries = resolved_vocab_terms_to_entries_with_aliases(
                    resolved.resolved_terms,
                    &alias_map,
                );
                (resolved.transcript, entries)
            }
        };
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

        let default_prompt_body = default_voice_prompt_template();
        let prompt_body = {
            let pool_p = pool.clone();
            let uid_p = user_id.clone();
            let body_p = default_prompt_body.clone();
            tokio::task::spawn_blocking(move || {
                prompt_templates::active_body_or_default(
                    &pool_p,
                    &uid_p,
                    prompt_templates::DefaultPrompt {
                        kind: VOICE_PROMPT_KIND,
                        title: VOICE_PROMPT_TITLE,
                        base_version: VOICE_PROMPT_BASE_VERSION,
                        body: &body_p,
                    },
                )
            })
            .await
            .unwrap_or(default_prompt_body)
        };
        let relevant_corrections = crate::store::corrections::filter_relevant(
            &word_corrections, &resolved_transcript, 2, 10,
        );
        let mut base_system_prompt = render_voice_system_prompt_template(
            &prompt_body,
            &prefs,
            &rag_examples,
            &relevant_corrections,
            &vocab_entries,
        );

        // Inject dynamic few-shot correction examples from user's history.
        // These teach the LLM by pattern — far more effective for small models
        // than abstract rules (research: +7-12% F1 improvement).
        {
            let pool_fs = pool.clone();
            let uid_fs = user_id.clone();
            let transcript_fs = stt_transcript.clone();
            let fewshot = tokio::task::spawn_blocking(move || {
                crate::store::history::select_fewshot_examples(
                    &pool_fs, &uid_fs, &transcript_fs, 8,
                )
            })
            .await
            .unwrap_or_default();
            if !fewshot.is_empty() {
                let block = crate::llm::prompt::format_fewshot_block(&fewshot);
                base_system_prompt.push_str(&block);
                info!("[voice] injected {} few-shot correction example(s)", fewshot.len());
            }
        }

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

        if let Some(ref ctx) = screen_context {
            let trimmed: String = ctx.chars().take(500).collect();
            if !trimmed.trim().is_empty() {
                info!(
                    "[voice] screen context: {} chars",
                    trimmed.len()
                );
                base_system_prompt.push_str(&format!(
                    "\n\nSCREEN CONTEXT (text already in the user's app):\n\
                     \"{trimmed}\"\n\n\
                     Use screen context to pick the right word when two sound alike — \
                     if the field already names a product, person, or acronym, prefer that \
                     spelling over phonetically similar STT guesses. \
                     Only use as a tiebreaker — transcript words come first.\n"
                ));
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

        // ── STEP 5: LLM polish ───────────────────────────────────────────────────
        let enforce_roman_hinglish = prefs.output_language == "hinglish";
        let groq_key_for_recovery = groq_key.clone();
        let llm_start = Instant::now();
        let mut saw_script_rewrite = false;
        let (mut llm_result, actual_model_used, stream_filter) = if prefs.server_runtime_enabled {
            yield Ok(Event::default().event("status")
                .data(json!({"phase": "server_polishing", "transcript": &resolved_transcript}).to_string()));
            info!("[timing] LLM start — provider=server_runtime selected_model={:?}", prefs.selected_model);
            match run_server_runtime_voice_probe(
                &http_client,
                &pool,
                &user_id,
                client_run_id.as_deref(),
                &resolved_transcript,
                &prefs.output_language,
                &prefs.selected_model,
                screen_context.as_deref(),
                &vocab_entries,
            )
            .await {
                Ok((result, model)) => {
                    info!(
                        "[voice] server runtime probe returned {} chars using {model}",
                        result.polished.len()
                    );
                    (
                        result,
                        model,
                        StreamSafetyFilter::new(StreamProvider::from_llm_provider("server_runtime"), &resolved_transcript),
                    )
                }
                Err(e) => {
                    warn!("[voice] server runtime probe failed; falling back to local polish: {e}");
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
                        cerebras_key.clone(),
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
                                StreamSafetyFilter::new(
                                    StreamProvider::from_llm_provider(&fallback_provider),
                                    &resolved_transcript,
                                ),
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
                            yield Ok(Event::default().event("error").data(
                                json!({"message": message, "audio_id": aid}).to_string()
                            ));
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
            let (model_for_llm, openai_token_opt) = if llm_provider == "openai_codex" {
                let pool_tok = pool.clone();
                let uid_tok  = user_id.clone();
                let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
                    .await
                    .unwrap_or(None);
                (openai_codex::MODEL_MINI.to_string(), tok.map(|t| t.access_token))
            } else if llm_provider == "gemini_direct" {
                (gemini_direct::GEMINI_DIRECT_MODEL.to_string(), None)
            } else if llm_provider == "groq" {
                (
                    if prefs.selected_model == "smart" {
                        groq::GROQ_MODEL_SMART
                    } else {
                        groq::GROQ_MODEL_FAST
                    }
                    .to_string(),
                    None,
                )
            } else if llm_provider == "cerebras" {
                (cerebras::CEREBRAS_MODEL_DEFAULT.to_string(), None)
            } else {
                (said_core::resolve_model(&prefs.selected_model).to_string(), None)
            };
            let llm_provider_for_task = llm_provider.clone();

            let gk          = gateway_key.clone();
            let gk_gemini   = gemini_key.clone();
            let gk_groq     = groq_key.clone();
            let gk_cerebras = cerebras_key.clone();

            info!("[timing] LLM start — provider={llm_provider:?} model={model_for_llm:?}");
            let actual_model_used = model_for_llm.clone();

            let llm_task = tokio::spawn(async move {
                if llm_provider_for_task == "openai_codex" {
                    let access_token = openai_token_opt.as_deref().unwrap_or("");
                    if access_token.is_empty() {
                        return Err("OpenAI not connected — go to Settings to connect your account".to_string());
                    }
                    openai_codex::stream_polish(
                        &client_c, access_token, &model_for_llm, &sys_p, &usr_m, token_tx,
                    ).await
                } else if llm_provider_for_task == "gemini_direct" {
                    gemini_direct::stream_polish(
                        &client_c, &gk_gemini, &model_for_llm, &sys_p, &usr_m, token_tx,
                    ).await
                } else if llm_provider_for_task == "groq" {
                    groq::stream_polish(
                        &client_c, &gk_groq, &model_for_llm, &sys_p, &usr_m, token_tx,
                    ).await
                } else if llm_provider_for_task == "cerebras" {
                    cerebras::stream_polish(
                        &client_c, &gk_cerebras, &model_for_llm, &sys_p, &usr_m, token_tx,
                    ).await
                } else {
                    gateway::stream_polish(&client_c, &gk, &model_for_llm, &sys_p, &usr_m, token_tx).await
                }
            });

            let mut stream_filter =
                StreamSafetyFilter::new(StreamProvider::from_llm_provider(&llm_provider), &resolved_transcript);

            // Yield each token as an SSE event. For Hinglish we defensively
            // romanize any Devanagari before it reaches the desktop's live typing
            // path; otherwise a bad model token can already be pasted before the
            // final result is scrubbed.
            while let Some(raw_token) = token_rx.recv().await {
                let filtered = stream_filter.push_token(raw_token);
                if filtered.unsafe_detected {
                    warn!("[voice] stream safety disabled live typing for provider={llm_provider}");
                }
                for token in filtered.tokens {
                    let token = if enforce_roman_hinglish && token != STREAM_RESET_SENTINEL {
                        let mut t = token;
                        if script::contains_devanagari(&t) {
                            if !saw_script_rewrite {
                                saw_script_rewrite = true;
                                yield Ok(Event::default().event("token")
                                    .data(json!({"token": STREAM_RESET_SENTINEL}).to_string()));
                            }
                            t = script::enforce_roman_hinglish(&t);
                        }
                        script::strip_non_latin_scripts(&t)
                    } else {
                        token
                    };
                    yield Ok(Event::default().event("token")
                        .data(json!({"token": token}).to_string()));
                }
            }

            let llm_result = match llm_task.await {
                Ok(Ok(r))   => r,
                Ok(Err(e))  => {
                    let message = if invalidate_openai_session_on_auth_error(&pool, &user_id, &llm_provider, &e) {
                        "OpenAI not connected — go to Settings to connect your account".to_string()
                    } else {
                        e.clone()
                    };
                    warn!("[voice] LLM error: {e}");
                    yield Ok(Event::default().event("error").data(
                        json!({"message": message, "audio_id": aid}).to_string()
                    ));
                    return;
                }
                Err(e) => {
                    warn!("[voice] LLM task panicked: {e}");
                    yield Ok(Event::default().event("error").data(
                        json!({"message": "internal error", "audio_id": aid}).to_string()
                    ));
                    return;
                }
            };
            (llm_result, actual_model_used, stream_filter)
        };

        // Defensive scrub: the LLM is told NOT to emit [word?XX%] confidence
        // markers, but occasionally leaks them anyway (sometimes malformed,
        // e.g. "[main60%]" with no '?'). Strip any survivors before this
        // text reaches the user, the paste path, or the DB.
        let scrubbed = strip_confidence_markers(&llm_result.polished);
        if scrubbed != llm_result.polished {
            warn!(
                "[voice] LLM leaked confidence markers — scrubbed {} → {} chars",
                llm_result.polished.len(), scrubbed.len(),
            );
            llm_result.polished = scrubbed;
        }

        let scrubbed = scrub_polished_output(
            &llm_result.polished,
            &resolved_transcript,
            stream_filter.saw_unsafe_content(),
        );
        if scrubbed != llm_result.polished {
            warn!(
                "[voice] scrubbed prompt/transcript leakage from final output {} → {} chars",
                llm_result.polished.len(),
                scrubbed.len(),
            );
            llm_result.polished = scrubbed;
        }

        if enforce_roman_hinglish && script::contains_devanagari(&llm_result.polished) {
            let romanized = match crate::llm::devanagari_recovery::recover(
                &http_client, &groq_key_for_recovery, &llm_result.polished,
            ).await {
                Ok(recovered) => {
                    info!(
                        "[voice] Devanagari LLM recovery succeeded — {} → {} chars",
                        llm_result.polished.len(), recovered.len(),
                    );
                    recovered
                }
                Err(e) => {
                    warn!(
                        "[voice] Devanagari LLM recovery failed ({e}) — falling back to mechanical romanization",
                    );
                    script::enforce_roman_hinglish(&llm_result.polished)
                }
            };
            warn!(
                "[voice] Devanagari detected in output — recovered {} → {} chars",
                llm_result.polished.len(),
                romanized.len(),
            );
            llm_result.polished = romanized;
            if !stream_filter.live_disabled() {
                if !saw_script_rewrite {
                    yield Ok(Event::default().event("token")
                        .data(json!({"token": STREAM_RESET_SENTINEL}).to_string()));
                }
            }
        };

        // Defense: strip any non-Latin script hallucinations (katakana, CJK, etc)
        if enforce_roman_hinglish {
            let stripped = script::strip_non_latin_scripts(&llm_result.polished);
            if stripped != llm_result.polished {
                warn!(
                    "[voice] stripped non-Latin hallucination: {} → {} chars",
                    llm_result.polished.len(),
                    stripped.len(),
                );
                llm_result.polished = stripped;
            }
        }

        let llm_ms   = llm_start.elapsed().as_millis() as i64;
        let total_ms = total_start.elapsed().as_millis() as i64;

        // Content guard: if the LLM dropped more than half the transcript
        // words, fall back to the cleaned transcript (markers stripped).
        // Runs BEFORE format_recover so that email folding (which
        // intentionally collapses many words into one) doesn't trip it.
        let transcript_wc = resolved_transcript.split_whitespace().count();
        let polished_wc   = llm_result.polished.split_whitespace().count();
        if transcript_wc > 4 && polished_wc < transcript_wc / 2 {
            let mut cleaned = strip_confidence_markers(&resolved_transcript);
            if enforce_roman_hinglish && script::contains_devanagari(&cleaned) {
                cleaned = script::enforce_roman_hinglish(&cleaned);
            }
            warn!(
                "[voice] polish dropped too much content: transcript={} words → polished={} words — falling back to cleaned transcript",
                transcript_wc, polished_wc,
            );
            llm_result.polished = cleaned;
        }

        let numeric_final = crate::number_format::apply(&llm_result.polished);
        if numeric_final != llm_result.polished {
            info!(
                "[voice] deterministic number format after LLM: {:?} → {:?}",
                llm_result.polished, numeric_final
            );
            llm_result.polished = numeric_final;
        }

        let email_candidates = email_memory::load_candidates(&pool, &user_id);
        let (email_final, email_recoveries) =
            crate::llm::format_recover::recover_emails_with_candidates(
                &llm_result.polished,
                &email_candidates,
            );
        if email_final != llm_result.polished {
            info!(
                "[voice] deterministic email format after LLM: {} recovery/replacement(s)",
                email_recoveries.len()
            );
            llm_result.polished = email_final;
        }

        // Final exact-alias resolver. This is intentionally narrower than the
        // old global fuzzy Tier2 pass: only approved exact aliases/edit-safe
        // rows can fire here, including cached company bucket aliases.
        let exact_final = stt_replacements::apply_exact_safe(&llm_result.polished, &stt_replacement_rules);
        if exact_final.text != llm_result.polished {
            info!(
                "[voice] final exact alias resolver: {} replacement(s)",
                exact_final.matches.len()
            );
            llm_result.polished = exact_final.text;
        }

        // Post-LLM number/email/exact-alias changes are reconciled by the desktop against
        // the current recording's streamed text. No STREAM_RESET_SENTINEL is
        // needed for deterministic formatter-only changes because the desktop
        // patches only this recording span after `done`.

        let word_count = llm_result.polished.split_whitespace().count() as i64;
        info!("[timing] LLM={}ms (TTFT inside) | total={}ms ← STT={}ms embed={}ms rag={}ms llm={}ms",
            llm_ms, total_ms, transcribe_ms, embed_ms, rag_ms, llm_ms);

        let recording_id = Uuid::new_v4().to_string();

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
            let inserted = tokio::task::spawn_blocking(move || {
                insert_recording(&pool2, InsertRecording {
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
                }).is_some()
            }).await.unwrap_or(false);
            if !inserted {
                warn!("[voice] failed to insert recording history row");
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
                    "retrieve":   rag_ms,
                    "polish":     llm_ms,
                    "total":      total_ms,
                },
                "examples_used": examples_used,
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

#[derive(Debug, Clone)]
struct QualityAssessment {
    score: f64,
    poor: bool,
    mostly_hindi: bool,
    code_switch_hint: bool,
    protected_hits: usize,
    too_short: bool,
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

async fn maybe_rescue_transcript(
    client: &reqwest::Client,
    provider: &str,
    api_key: &str,
    wav_data: Vec<u8>,
    audio_seconds: f64,
    bias: &BiasPackage,
    primary_ws: Option<TranscriptCandidate>,
) -> Result<(TranscriptCandidate, i64), String> {
    if let Some(primary) = primary_ws {
        let primary_quality = assess_candidate(&primary, audio_seconds, bias);
        let Some(rescue_mode) = rescue_mode_for(&primary_quality, &primary.meta.stt_mode) else {
            return Ok((primary, 0));
        };
        if wav_data.is_empty() {
            return Ok((primary, 0));
        }
        let rescue = run_batch_transcript(
            client,
            provider,
            api_key,
            wav_data,
            with_mode(bias, &rescue_mode),
            format!("rescue:{rescue_mode}"),
        )
        .await?;
        let rescue_quality = assess_candidate(&rescue, audio_seconds, bias);
        let chosen = choose_candidate(primary, primary_quality, rescue, rescue_quality);
        return Ok((chosen, 1));
    }

    let primary = run_batch_transcript(
        client,
        provider,
        api_key,
        wav_data.clone(),
        bias.clone(),
        format!("batch:{}", bias.stt_mode),
    )
    .await?;
    let primary_quality = assess_candidate(&primary, audio_seconds, bias);
    let Some(rescue_mode) = rescue_mode_for(&primary_quality, &bias.stt_mode) else {
        return Ok((primary, 0));
    };
    let rescue = run_batch_transcript(
        client,
        provider,
        api_key,
        wav_data,
        with_mode(bias, &rescue_mode),
        format!("rescue:{rescue_mode}"),
    )
    .await?;
    let rescue_quality = assess_candidate(&rescue, audio_seconds, bias);
    let chosen = choose_candidate(primary, primary_quality, rescue, rescue_quality);
    Ok((chosen, 1))
}

fn with_mode(bias: &BiasPackage, stt_mode: &str) -> BiasPackage {
    let mut next = bias.clone();
    next.stt_mode = stt_mode.to_string();
    next
}

async fn run_batch_transcript(
    client: &reqwest::Client,
    _provider: &str,
    api_key: &str,
    wav_data: Vec<u8>,
    bias: BiasPackage,
    source: String,
) -> Result<TranscriptCandidate, String> {
    let result = deepgram::transcribe(client, api_key, wav_data, &bias).await?;
    let meta = result.meta();
    Ok(TranscriptCandidate {
        transcript: result.transcript,
        meta,
        source,
    })
}

fn choose_candidate(
    primary: TranscriptCandidate,
    primary_quality: QualityAssessment,
    rescue: TranscriptCandidate,
    rescue_quality: QualityAssessment,
) -> TranscriptCandidate {
    info!(
        "[voice] transcript quality primary(score={:.2}, poor={}, protected_hits={}, too_short={}) rescue(score={:.2}, poor={}, protected_hits={}, too_short={})",
        primary_quality.score,
        primary_quality.poor,
        primary_quality.protected_hits,
        primary_quality.too_short,
        rescue_quality.score,
        rescue_quality.poor,
        rescue_quality.protected_hits,
        rescue_quality.too_short,
    );
    if rescue_quality.score > primary_quality.score + 0.5 {
        info!("[voice] rescue transcript won over primary");
        rescue
    } else {
        primary
    }
}

fn rescue_mode_for(quality: &QualityAssessment, current_mode: &str) -> Option<String> {
    if !quality.poor {
        return None;
    }
    match current_mode {
        "multi" if quality.mostly_hindi => Some("hi".to_string()),
        "hi" if quality.code_switch_hint => Some("multi".to_string()),
        _ => None,
    }
}

fn assess_candidate(
    candidate: &TranscriptCandidate,
    audio_seconds: f64,
    bias: &BiasPackage,
) -> QualityAssessment {
    let word_count = if candidate.meta.word_count > 0 {
        candidate.meta.word_count
    } else {
        candidate.transcript.split_whitespace().count()
    };
    let mean_confidence = if candidate.meta.mean_word_confidence > 0.0 {
        candidate.meta.mean_word_confidence
    } else if candidate.meta.confidence > 0.0 {
        candidate.meta.confidence
    } else {
        0.75
    };
    let low_conf_ratio = if word_count == 0 {
        1.0
    } else {
        candidate.meta.low_confidence_count as f64 / word_count as f64
    };
    let expected_min_words = if audio_seconds > 3.0 {
        (audio_seconds / 2.0).max(1.0) as usize
    } else {
        0
    };
    let too_short = expected_min_words > 0 && word_count < expected_min_words;
    let protected_hits = count_protected_hits(&candidate.transcript, bias);
    let has_ascii = candidate
        .transcript
        .chars()
        .any(|c| c.is_ascii_alphabetic());
    let devanagari_chars = candidate
        .transcript
        .chars()
        .filter(|c| ('\u{0900}'..='\u{097F}').contains(c))
        .count();
    let alpha_chars = candidate
        .transcript
        .chars()
        .filter(|c| c.is_alphabetic())
        .count()
        .max(1);
    let mostly_hindi = candidate
        .meta
        .languages
        .iter()
        .all(|lang| lang.starts_with("hi"))
        || (devanagari_chars as f64 / alpha_chars as f64) > 0.55;
    let code_switch_hint = candidate
        .meta
        .languages
        .iter()
        .any(|lang| lang.starts_with("en"))
        || (has_ascii && devanagari_chars > 0)
        || protected_hits > 0;
    let score = protected_hits as f64 * 2.0 + mean_confidence * 2.0
        - low_conf_ratio * 2.0
        - if too_short { 2.0 } else { 0.0 }
        + if candidate.meta.languages.len() > 1 {
            0.5
        } else {
            0.0
        };
    let poor = too_short
        || mean_confidence < 0.65
        || (mean_confidence < 0.8 && low_conf_ratio > 0.35)
        || score < 1.0;
    QualityAssessment {
        score,
        poor,
        mostly_hindi,
        code_switch_hint,
        protected_hits,
        too_short,
    }
}

fn count_protected_hits(transcript: &str, bias: &BiasPackage) -> usize {
    let lower = transcript.to_ascii_lowercase();
    let mut hits = 0usize;
    for keyterm in &bias.keyterms {
        if !keyterm.is_empty() && lower.contains(&keyterm.to_ascii_lowercase()) {
            hits += 1;
        }
    }
    for replacement in &bias.replacements {
        if let Some(canonical) = replacement.replace.as_deref() {
            if !canonical.is_empty() && lower.contains(&canonical.to_ascii_lowercase()) {
                hits += 1;
            }
        }
    }
    hits
}

fn wav_duration_seconds(wav_data: &[u8]) -> f64 {
    if wav_data.len() <= 44 {
        return 0.0;
    }
    (wav_data.len().saturating_sub(44)) as f64 / 32_000.0
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
/// LLM can see which words Deepgram was unsure about and use context to fix them.
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
