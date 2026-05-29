//! POST /v1/voice/polish
//!
//! Receives a multipart form with:
//!   audio        — WAV bytes  (required)
//!   target_app   — bundle-id of the focused app  (optional)
//!   pre_transcript — transcript already obtained via Deepgram WS streaming  (optional, P5)
//!
//! Pipeline: auth → load prefs → STT → evidence collection → dynamic prompt →
//!           LLM stream → post-LLM passes → SSE.

// ─── STT PROVIDER OVERRIDE ─────────────────────────────────────────────────
//
// Change this ONE constant to switch the STT engine for the entire app.
// The DB preference (prefs.stt_provider) is ignored when this is set.
//
// Valid values:
//   "" (empty)        — use the DB preference (user-configurable in Settings)
//   "deepgram"        — Deepgram nova-3 streaming (fast, but inconsistent on Hinglish)
//   "groq_whisper"    — Whisper large-v3-turbo via Groq API (more accurate, batch only)
//   "whisper_local"   — Local whisper-rs (offline, requires feature flag)
//
// AI agents: to switch STT provider, change ONLY this line. Nothing else.
const STT_PROVIDER_OVERRIDE: &str = "deepgram";

use axum::{
    Json,
    extract::{Multipart, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use said_core::deepgram::{BiasPackage, TranscriptMeta};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

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
            build_message_polish_system_prompt, build_message_polish_user_message,
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

async fn run_message_polish_pass(
    http_client: reqwest::Client,
    pool: crate::store::DbPool,
    user_id: String,
    groq_key: String,
    text: &str,
) -> Result<(crate::llm::PolishResult, String), String> {
    let system_prompt = build_message_polish_system_prompt();
    let user_message = build_message_polish_user_message(text);

    let codex_token = {
        let pool_tok = pool.clone();
        let uid_tok = user_id.clone();
        tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
            .await
            .unwrap_or(None)
    };

    if let Some(tok) = codex_token {
        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let drain = tokio::spawn(async move { while token_rx.recv().await.is_some() {} });
        let result = openai_codex::stream_polish(
            &http_client,
            &tok.access_token,
            openai_codex::MODEL_MINI,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await;
        let _ = drain.await;
        match result {
            Ok(r) => return Ok((r, openai_codex::MODEL_MINI.to_string())),
            Err(e) => {
                let auth_failed =
                    invalidate_openai_session_on_auth_error(&pool, &user_id, "openai_codex", &e);
                if !auth_failed {
                    warn!(
                        "[voice] message polish Codex pass failed; falling back to Groq 70B: {e}"
                    );
                }
            }
        }
    }

    if groq_key.trim().is_empty() {
        return Err("Groq key missing for message polish fallback".to_string());
    }

    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
    let drain = tokio::spawn(async move { while token_rx.recv().await.is_some() {} });
    let result = groq::stream_polish(
        &http_client,
        &groq_key,
        groq::GROQ_MODEL_70B,
        &system_prompt,
        &user_message,
        token_tx,
    )
    .await;
    let _ = drain.await;
    result.map(|r| (r, groq::GROQ_MODEL_70B.to_string()))
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
    let missing =
        crate::routes::key_guard::missing_voice_api_keys(&pool, &user_id, prefs_for_guard);
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

        let deepgram_key = prefs.deepgram_api_key.clone()
            .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok())
            .unwrap_or_default();
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

        // ── STEP 1: STT ───────────────────────────────────────────────────────────
        // Resolve STT provider: code override takes priority over DB preference.
        let stt_provider: &str = if STT_PROVIDER_OVERRIDE.is_empty() {
            &prefs.stt_provider
        } else {
            STT_PROVIDER_OVERRIDE
        };
        info!("[voice] stt_provider={stt_provider:?}");
        let audio_seconds = wav_duration_seconds(&wav_data);
        let use_alt_stt = stt_provider == "whisper_local" || stt_provider == "groq_whisper";
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
                &deepgram_key,
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
                    &deepgram_key,
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
                     Use screen context to pick the right word when two sound alike. \
                     Screen mentions EMIAC = prefer EMIAC over similar sounds. \
                     Screen mentions code/tech = prefer technical terms. \
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

        // ── STEP 5: LLM stream (single pass) ─────────────────────────────────────
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
        let groq_key_for_recovery = groq_key.clone();

        let llm_start = Instant::now();
        info!("[timing] LLM start — provider={llm_provider:?} model={model_for_llm:?}");
        let mut actual_model_used = model_for_llm.clone();

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

        let enforce_roman_hinglish = prefs.output_language == "hinglish";
        let mut stream_filter =
            StreamSafetyFilter::new(StreamProvider::from_llm_provider(&llm_provider), &resolved_transcript);

        // Yield each token as an SSE event. For Hinglish we defensively
        // romanize any Devanagari before it reaches the desktop's live typing
        // path; otherwise a bad model token can already be pasted before the
        // final result is scrubbed.
        let mut saw_script_rewrite = false;
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

        let mut llm_result = match llm_task.await {
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

        if message_polish_mode {
            yield Ok(Event::default().event("status")
                .data(json!({"phase": "message_polishing", "transcript": llm_result.polished}).to_string()));
            let before_message_polish = llm_result.polished.clone();
            match run_message_polish_pass(
                http_client.clone(),
                pool.clone(),
                user_id.clone(),
                groq_key.clone(),
                &before_message_polish,
            ).await {
                Ok((mut message_result, message_model)) => {
                    let scrubbed = scrub_polished_output(
                        &message_result.polished,
                        &before_message_polish,
                        false,
                    );
                    if scrubbed != message_result.polished {
                        warn!(
                            "[voice] scrubbed message-polish leakage {} → {} chars",
                            message_result.polished.len(),
                            scrubbed.len(),
                        );
                        message_result.polished = scrubbed;
                    }
                    if message_result.polished.trim().is_empty() {
                        warn!("[voice] message polish returned empty output — keeping first-pass result");
                    } else {
                        info!(
                            "[voice] message polish mode applied using model={message_model} {} → {} chars",
                            before_message_polish.len(),
                            message_result.polished.len(),
                        );
                        llm_result.polish_ms =
                            llm_result.polish_ms.saturating_add(message_result.polish_ms);
                        llm_result.polished = message_result.polished;
                        actual_model_used = format!("{actual_model_used} + {message_model}");
                    }
                }
                Err(e) => {
                    warn!(
                        "[voice] message polish pass failed — keeping first-pass result: {e}"
                    );
                }
            }
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
