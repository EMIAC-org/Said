//! POST /v1/voice/polish
//!
//! Receives a multipart form with:
//!   audio        — WAV bytes  (required)
//!   target_app   — bundle-id of the focused app  (optional)
//!   pre_transcript — transcript already obtained via Deepgram WS streaming  (optional, P5)
//!
//! Pipeline: auth → load prefs → STT (skipped if pre_transcript present) → embed‖prompt →
//!           RAG → LLM stream → SSE.
//!
//! P2: Deepgram embedding is spawned concurrently with prompt-skeleton building
//!     the moment the transcript is available.
//!
//! P5: If the caller (Tauri) already streamed audio to Deepgram via WebSocket and
//!     has a transcript ready, it is supplied as `pre_transcript`.  The STT step is
//!     skipped, saving ~1.2–2 s on every recording.

use axum::{
    Json,
    extract::{Multipart, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;
use said_core::deepgram::{BiasPackage, TranscriptMeta};

// ── Audio file helpers ────────────────────────────────────────────────────────

/// Directory where WAV recordings are saved locally (1-day retention).
fn audio_dir() -> std::path::PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
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
        gateway, gemini_direct, groq, openai_codex,
        prompt::{
            VocabEntry, build_system_prompt_with_vocab_entries, build_user_message,
            build_voice_repair_system_prompt, build_voice_repair_user_message,
            resolved_vocab_terms_to_entries, vocab_terms_to_entries,
        },
        script,
        stream_safety::{
            STREAM_RESET_SENTINEL, StreamProvider, StreamSafetyFilter, scrub_polished_output,
        },
        vocab_resolver,
    },
    store::{
        history::{InsertRecording, insert_recording},
        openai_oauth, stt_replacements,
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
    // ── Extract multipart fields ───────────────────────────────────────────────
    let mut wav_data: Vec<u8> = Vec::new();
    let mut target_app: Option<String> = None;
    let mut pre_transcript: Option<String> = None; // P5: from Deepgram WS
    let mut pre_transcript_meta: Option<TranscriptMeta> = None;
    let mut repair_mode: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("audio") => {
                wav_data = field.bytes().await.unwrap_or_default().to_vec();
            }
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
        },
    )
    .await
}

pub async fn polish_transcript(
    State(state): State<AppState>,
    Json(req): Json<TranscriptPolishRequest>,
) -> impl IntoResponse {
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
        },
    )
    .await
}

pub async fn repair_transcript(
    State(state): State<AppState>,
    Json(req): Json<VoiceRepairRequest>,
) -> impl IntoResponse {
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
        let llm_provider = prefs.llm_provider.clone();
        let llm_provider_for_task = llm_provider.clone();
        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let sys_p = system_prompt.clone();
        let usr_m = user_message.clone();
        let client_c = http_client.clone();
        let model = said_core::resolve_model(&prefs.selected_model).to_string();
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
            (groq::GROQ_MODEL_DEFAULT.to_string(), None)
        } else {
            (model.clone(), None)
        };

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
        if enforce_roman_hinglish && script::contains_devanagari(&llm_result.polished) {
            llm_result.polished = script::enforce_roman_hinglish(&llm_result.polished);
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
            let model2 = model.clone();
            let p_ms = llm_result.polish_ms as i64;
            tokio::spawn(async move {
                insert_recording(&pool2, InsertRecording {
                    id: &id2, user_id: &uid2,
                    transcript: &t2, polished: &p2,
                    word_count, recording_seconds: (total_ms as f64 / 1000.0),
                    model_used: &model2,
                    confidence: None,
                    transcribe_ms: None,
                    embed_ms: None,
                    polish_ms: Some(p_ms),
                    target_app: ta2.as_deref(),
                    source: "voice_repair",
                    audio_id: aid2.as_deref(),
                });
            });
        }

        yield Ok(Event::default().event("done").data(
            json!({
                "recording_id": recording_id,
                "transcript": transcript,
                "polished": llm_result.polished,
                "model_used": model,
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

async fn polish_with_input(state: AppState, input: VoicePolishInput) -> Response {
    let VoicePolishInput {
        wav_data,
        target_app,
        pre_transcript,
        pre_transcript_meta,
        repair_mode,
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
        tokio::task::spawn_blocking(move || vocabulary::top_terms(&pool_c, &uid_c, 100))
    };
    let (prefs_opt, (word_corrections, stt_replacement_rules), vocab_full) = tokio::join!(
        crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id),
        crate::get_lexicon_cached(&state.lexicon_cache, &pool, &user_id),
        async { vocab_task.await.unwrap_or_default() },
    );
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

        let stt_bias_package = tokio::task::spawn_blocking({
            let pool = pool.clone();
            let user_id = user_id.clone();
            let language = prefs.language.clone();
            let output_language = prefs.output_language.clone();
            move || stt_bias::build_bias_package(&pool, &user_id, &language, &output_language)
        })
        .await
        .unwrap_or_else(|_| BiasPackage::default());

        // ── STEP 1: STT ───────────────────────────────────────────────────────────
        let audio_seconds = wav_duration_seconds(&wav_data);
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
        };

        let (stt_transcript, _enriched_transcript, alias_result) = if stt_replacement_rules.is_empty() {
            let text = stt_transcript_raw.clone();
            let enriched = enriched_raw.clone();
            (
                text.clone(),
                enriched,
                stt_replacements::ApplyResult {
                    text,
                    matches: vec![],
                },
            )
        } else {
            let alias_result =
                stt_replacements::apply_with_matches(&stt_transcript_raw, &stt_replacement_rules);
            let plain_rewritten = alias_result.text.clone();
            let enriched_rewritten = stt_replacements::apply(&enriched_raw, &stt_replacement_rules);
            if plain_rewritten != stt_transcript_raw {
                info!("[voice] lexicon replacement: {:?} → {:?}", stt_transcript_raw, plain_rewritten);
            }
            (plain_rewritten, enriched_rewritten, alias_result)
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

        let model = said_core::resolve_model(&prefs.selected_model).to_string();

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
            let chosen = tokio::task::spawn_blocking(move || {
                vocab_embeddings::select_for_prompt(
                    &pool_v, &uid_v, &lang_v, emb_v.as_deref(), Some(&txt_v),
                )
            }).await.unwrap_or_default();
            if chosen.is_empty() {
                // Empty is a meaningful result when a transcript was passed:
                // there was no lexical evidence that any vocab term applied.
                // Do not fall back to the full slate; that reintroduces
                // over-replacement from unrelated learned terms.
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
                let mut entries = resolved_vocab_terms_to_entries(resolved.resolved_terms);
                entries.extend(vocab_terms_to_entries(resolved.candidate_terms));
                (resolved.transcript, entries)
            }
        };
        let user_message = build_user_message(&resolved_transcript, &prefs.output_language);

        let system_prompt = if repair_mode.as_deref() == Some("preserve_recall") {
            format!(
                "{}\n\nREPAIR OVERRIDE:\n- The user explicitly asked to reprocess this recording because the previous output likely missed words or drifted in language.\n- Be extra conservative about deleting words.\n- Prefer keeping uncertain transcript content over compressing it.\n- Preserve numbers, names, acronyms, dates, and mixed Hindi-English spans.",
                build_system_prompt_with_vocab_entries(
                    &prefs, &rag_examples, &word_corrections, &vocab_entries,
                )
            )
        } else {
            build_system_prompt_with_vocab_entries(
                &prefs, &rag_examples, &word_corrections, &vocab_entries,
            )
        };

        // ── STEP 5: LLM stream ────────────────────────────────────────────────────
        let llm_provider = prefs.llm_provider.clone();
        let llm_provider_for_task = llm_provider.clone();
        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let sys_p       = system_prompt.clone();
        let usr_m       = user_message.clone();
        let client_c    = http_client.clone();

        let (model_for_llm, openai_token_opt) = if llm_provider == "openai_codex" {
            let pool_tok = pool.clone();
            let uid_tok  = user_id.clone();
            let tok = tokio::task::spawn_blocking(move || openai_oauth::get_token(&pool_tok, &uid_tok))
                .await
                .unwrap_or(None);
            let m = openai_codex::MODEL_MINI.to_string();
            (m, tok.map(|t| t.access_token))
        } else if llm_provider == "gemini_direct" {
            (gemini_direct::GEMINI_DIRECT_MODEL.to_string(), None)
        } else if llm_provider == "groq" {
            (groq::GROQ_MODEL_DEFAULT.to_string(), None)
        } else {
            (model.clone(), None)
        };

        let gk          = gateway_key.clone();
        let gk_gemini   = gemini_key.clone();
        let gk_groq     = groq_key.clone();

        let llm_start = Instant::now();
        info!("[timing] LLM start — provider={llm_provider:?} model={model_for_llm:?}");

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
                let token = if enforce_roman_hinglish && token != STREAM_RESET_SENTINEL && script::contains_devanagari(&token) {
                    if !saw_script_rewrite {
                        saw_script_rewrite = true;
                        yield Ok(Event::default().event("token")
                            .data(json!({"token": STREAM_RESET_SENTINEL}).to_string()));
                    }
                    script::enforce_roman_hinglish(&token)
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
            let romanized = script::enforce_roman_hinglish(&llm_result.polished);
            warn!(
                "[voice] LLM emitted Devanagari in Hinglish mode — romanized {} → {} chars",
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

        let llm_ms   = llm_start.elapsed().as_millis() as i64;
        let total_ms = total_start.elapsed().as_millis() as i64;
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
            let model2  = model.clone();
            let conf    = stt_confidence;
            let t_ms    = transcribe_ms;
            let e_ms    = embed_ms;
            let p_ms    = llm_result.polish_ms as i64;
            let aid2    = saved_audio_id.clone();
            let inserted = tokio::task::spawn_blocking(move || {
                insert_recording(&pool2, InsertRecording {
                    id: &id2, user_id: &uid2,
                    transcript: &t2, polished: &p2,
                    word_count, recording_seconds: (total_ms as f64 / 1000.0),
                    model_used: &model2,
                    confidence:    Some(conf),
                    transcribe_ms: Some(t_ms),
                    embed_ms:      Some(e_ms),
                    polish_ms:     Some(p_ms),
                    target_app:    ta2.as_deref(),
                    source:        "voice",
                    audio_id:      aid2.as_deref(),
                }).is_some()
            }).await.unwrap_or(false);
            if !inserted {
                warn!("[voice] failed to insert recording history row");
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
                "model_used":   model,
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
fn strip_confidence_markers(s: &str) -> String {
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
    use super::strip_confidence_markers;

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
}
