//! POST /v1/text/polish
//!
//! Same polish pipeline as /v1/voice/polish for already-transcribed text.
//! Body: { "text": "...", "target_app"?: "..." }
//! Response: SSE stream identical to voice/polish.

use axum::{
    Json,
    extract::State,
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

use crate::{
    AppState,
    embedder::gemini,
    llm::{
        gateway, gemini_direct, groq, openai_codex,
        prompt::{
            VocabEntry, build_refine_last_transform_prompt,
            build_refine_last_transform_user_message, build_system_prompt_with_vocab_entries,
            build_tray_format_system_prompt, build_tray_format_user_message,
            build_tray_system_prompt, build_tray_user_message, build_user_message,
            resolved_vocab_terms_to_entries,
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
};

#[derive(Deserialize)]
pub struct TextPolishBody {
    pub text: String,
    pub target_app: Option<String>,
    /// When set (by tray "Polish my message"), overrides the user's stored tone_preset
    /// and forces English output — the preset label already encodes the output language.
    pub tone_override: Option<String>,
}

#[derive(Deserialize)]
pub struct TextRefineBody {
    pub source_text: String,
    pub previous_output: String,
    pub tone: Option<String>,
}

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
    warn!("[text] invalidated stored OpenAI OAuth token after auth failure");
    true
}

fn llm_error_payload(raw: &str, message: String) -> serde_json::Value {
    if let Some(details) = crate::llm::decode_llm_error(raw) {
        return json!({
            "message": message,
            "error_code": details.error_code,
            "retryable": details.retryable,
            "diagnostic": details.diagnostic,
        });
    }
    json!({ "message": message })
}

pub async fn polish(
    State(state): State<AppState>,
    Json(body): Json<TextPolishBody>,
) -> impl IntoResponse {
    if body.text.trim().is_empty() {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }

    // ⌥1 "Polish My Message" and ⌥2 "Convert to English" both run on the server
    // via DeepSeek (one hardened prompt, two modes). Anything else stays on the
    // local polish path.
    let server_helper_mode = match body.tone_override.as_deref() {
        Some("message_polish") => Some("polish"),
        Some("professional") => Some("to_english"),
        _ => None,
    };
    if let Some(mode) = server_helper_mode {
        if !crate::store::users::has_enterprise_auth(&state.pool, &state.default_user_id) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": crate::routes::message_polish::MESSAGE_POLISH_SIGNIN_ERROR,
                })),
            )
                .into_response();
        }
        return polish_message_polish_server(state, body, mode).await;
    }

    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();
    let transcript = body.text.clone();
    let target_app = body.target_app.clone();
    let tone_override = body.tone_override.clone();

    // Load prefs + lexicon from cache and grab shared HTTP client before stream.
    let vocab_task = {
        let pool_c = pool.clone();
        let uid_c = user_id.clone();
        tokio::task::spawn_blocking(move || vocabulary::top_terms(&pool_c, &uid_c, 500))
    };
    let prefs_opt = crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id).await;
    let Some(prefs_for_guard) = prefs_opt.as_ref() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let missing = crate::routes::key_guard::missing_text_api_keys(&pool, &user_id, prefs_for_guard);
    if !missing.is_empty() {
        return crate::routes::key_guard::missing_api_keys_response(missing);
    }
    let (word_corrections_cached, _stt_replacement_rules) =
        crate::get_lexicon_cached(&state.lexicon_cache, &pool, &user_id).await;
    let vocab_full = vocab_task.await.unwrap_or_default();
    let http_client = state.http_client.clone();

    let stream = async_stream::stream! {
        let total_start = Instant::now();

        let prefs = match prefs_opt {
            Some(p) => p,
            None => {
                yield Ok::<Event, Infallible>(Event::default().event("error")
                    .data(json!({"message": "preferences not found"}).to_string()));
                return;
            }
        };

        info!("[text] polishing {} chars with model={}", transcript.len(), prefs.selected_model);

        yield Ok(Event::default().event("status")
            .data(json!({"phase": "polishing", "transcript": transcript}).to_string()));

        // 1. Embed transcript + retrieve RAG examples
        let gemini_key = prefs.gemini_api_key.clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        // http_client is the shared client from AppState (loaded before stream)
        let e_start = Instant::now();
        let embedding   = gemini::embed(&http_client, &pool, &transcript, &gemini_key).await;
        let embed_ms    = e_start.elapsed().as_millis() as i64;

        let rag_examples = match &embedding {
            Some(emb) => {
                let hits = retrieve_similar(&pool, &user_id, emb, 5, 0.65);
                debug!("[text] RAG: {} examples retrieved", hits.len());
                hits
            }
            None => {
                debug!("[text] RAG skipped (no embedding)");
                vec![]
            }
        };
        let examples_used = rag_examples.len();

        // 2. Word corrections — formatter and normal polish both get them;
        //    other tray tones (professional, casual, etc.) don't need them.
        let is_formatter = tone_override.as_deref() == Some("format");
        let word_corrections = if tone_override.is_none() || is_formatter {
            word_corrections_cached
        } else {
            vec![]
        };
        if !word_corrections.is_empty() {
            info!("[text] {} word correction(s) loaded", word_corrections.len());
        }

        // Vocab resolution — formatter and normal polish get context-aware
        // vocab; other tray tones get raw transcript with no vocab.
        let (resolved_transcript, vocab_entries): (String, Vec<VocabEntry>) = if tone_override.is_none() || is_formatter {
            let alias_t0 = Instant::now();
            let alias_result = stt_replacements::ApplyResult {
                text: transcript.clone(),
                matches: vec![],
                traces: vec![],
            };
            let selected_terms = vocab_embeddings::select_for_prompt(
                &pool,
                &user_id,
                &prefs.output_language,
                embedding.as_deref(),
                Some(&alias_result.text),
            );
            let resolved = vocab_resolver::resolve_for_prompt(
                &alias_result.text,
                &selected_terms,
                &vocab_full,
                &alias_result,
            );
            let resolve_ms = alias_t0.elapsed().as_millis() as i64;
            info!(
                "[text] vocab resolver={}ms alias_matches={} context_matches={} resolved={} candidates={}",
                resolve_ms,
                resolved.alias_match_count,
                resolved.context_match_count,
                resolved.resolved_terms.len(),
                resolved.candidate_terms.len(),
            );
            let entries = resolved_vocab_terms_to_entries(resolved.resolved_terms);
            (resolved.transcript, entries)
        } else {
            (transcript.clone(), vec![])
        };
        let relevant_corrections = crate::store::corrections::filter_relevant(
            &word_corrections, &resolved_transcript, 2, 10,
        );
        let system_prompt = if is_formatter {
            build_tray_format_system_prompt(&vocab_entries, &relevant_corrections)
        } else if let Some(ref tone) = tone_override {
            build_tray_system_prompt(tone)
        } else {
            build_system_prompt_with_vocab_entries(
                &prefs, &rag_examples, &relevant_corrections, &vocab_entries,
            )
        };
        let user_message = if is_formatter {
            build_tray_format_user_message(&resolved_transcript)
        } else if let Some(ref tone) = tone_override {
            build_tray_user_message(&resolved_transcript, tone)
        } else {
            build_user_message(&resolved_transcript, &prefs.output_language)
        };

        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let gateway_key = prefs.gateway_api_key.clone()
            .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
            .or_else(|| { let k = said_core::api_key(); if k.is_empty() { None } else { Some(k.to_string()) } })
            .unwrap_or_default();
        let gemini_key_text = prefs.gemini_api_key.clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        let groq_key_text = prefs.groq_api_key.clone()
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();

        // Resolve selected-text transforms through the quality lane. Option+N
        // must never drop to the lightweight 8B path: use connected Codex
        // mini, otherwise Groq 70B.
        let sys_p       = system_prompt.clone();
        let usr_m       = user_message.clone();
        let client_c    = http_client.clone();

        let codex_tok = openai_oauth::get_token(&pool, &user_id);
        let (llm_provider, model_for_llm, openai_token_opt) = if let Some(tok) = codex_tok {
            (
                "openai_codex".to_string(),
                openai_codex::MODEL_MINI.to_string(),
                Some(tok.access_token),
            )
        } else {
            ("groq".to_string(), groq::GROQ_MODEL_70B.to_string(), None)
        };
        let llm_provider_for_task = llm_provider.clone();

        let groq_key_for_recovery = groq_key_text.clone();

        info!("[text] LLM provider={llm_provider:?} model={model_for_llm:?}");
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
                    &client_c, &gemini_key_text, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else if llm_provider_for_task == "groq" {
                groq::stream_polish(
                    &client_c, &groq_key_text, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else {
                gateway::stream_polish(&client_c, &gateway_key, &model_for_llm, &sys_p, &usr_m, token_tx).await
            }
        });

        let enforce_roman_hinglish = tone_override.is_none() && prefs.output_language == "hinglish";
        let mut stream_filter =
            StreamSafetyFilter::new(StreamProvider::from_llm_provider(&llm_provider), &resolved_transcript);

        let mut saw_script_rewrite = false;
        while let Some(raw_token) = token_rx.recv().await {
            let filtered = stream_filter.push_token(raw_token);
            if filtered.unsafe_detected {
                warn!("[text] stream safety disabled live typing for provider={llm_provider}");
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
            Ok(Ok(r))  => r,
            Ok(Err(e)) => {
                let message = if invalidate_openai_session_on_auth_error(&pool, &user_id, &llm_provider, &e) {
                    "OpenAI not connected — go to Settings to connect your account".to_string()
                } else {
                    crate::llm::llm_error_message(&e)
                };
                warn!("[text] LLM error: {e}");
                yield Ok(Event::default().event("error")
                    .data(llm_error_payload(&e, message).to_string()));
                return;
            }
            Err(_) => {
                yield Ok(Event::default().event("error")
                    .data(json!({"message": "internal error"}).to_string()));
                return;
            }
        };

        if enforce_roman_hinglish && script::contains_devanagari(&llm_result.polished) {
            let romanized = match crate::llm::devanagari_recovery::recover(
                &http_client, &groq_key_for_recovery, &llm_result.polished,
            ).await {
                Ok(recovered) => {
                    info!("[text] Devanagari LLM recovery succeeded — {} → {} chars", llm_result.polished.len(), recovered.len());
                    recovered
                }
                Err(e) => {
                    warn!("[text] Devanagari LLM recovery failed ({e}) — mechanical fallback");
                    script::enforce_roman_hinglish(&llm_result.polished)
                }
            };
            warn!(
                "[text] LLM emitted Devanagari in Hinglish mode — recovered {} → {} chars",
                llm_result.polished.len(),
                romanized.len(),
            );
            llm_result.polished = romanized;
        }

        let scrubbed = scrub_polished_output(
            &llm_result.polished,
            &resolved_transcript,
            stream_filter.saw_unsafe_content(),
        );
        if scrubbed != llm_result.polished {
            warn!(
                "[text] scrubbed prompt/transcript leakage from final output {} → {} chars",
                llm_result.polished.len(),
                scrubbed.len(),
            );
            llm_result.polished = scrubbed;
        }

        // format_recover disabled — will re-enable with targeted replacement
        // let recovered = crate::llm::format_recover::recover(&llm_result.polished);
        // if recovered != llm_result.polished {
        //     llm_result.polished = recovered;
        // }

        let total_ms     = total_start.elapsed().as_millis() as i64;
        let recording_id = Uuid::new_v4().to_string();
        let word_count   = llm_result.polished.split_whitespace().count() as i64;

        // Persist recording
        {
            let pool2  = pool.clone();
            let id2    = recording_id.clone();
            let uid2   = user_id.clone();
            let t2     = resolved_transcript.clone();
            let p2     = llm_result.polished.clone();
            let ta2    = target_app.clone();
            let model2 = actual_model_used.clone();
            let e_ms   = embed_ms;
            let p_ms   = llm_result.polish_ms as i64;
            tokio::spawn(async move {
                insert_recording(&pool2, InsertRecording {
                    id: &id2, user_id: &uid2,
                    transcript: &t2, polished: &p2,
                    word_count, recording_seconds: word_count as f64 * 60.0 / 130.0,
                    model_used: &model2,
                    confidence:    None,
                    transcribe_ms: None,
                    embed_ms:      Some(e_ms),
                    polish_ms:     Some(p_ms),
                    target_app:    ta2.as_deref(),
                    source:        "text",
                    audio_id:      None,
                    enriched_transcript: None,
                    raw_transcript: Some(&t2),
                    local_corrected_transcript: Some(&t2),
                    polished_output: Some(&p2),
                    trace_json: None,
                });
            });
        }

        debug!("[text] done — {total_ms}ms total, {word_count} words, {examples_used} RAG examples");

        yield Ok(Event::default().event("done").data(
            json!({
                "recording_id": recording_id,
                "transcript":   resolved_transcript,
                "polished":     llm_result.polished,
                "model_used":   actual_model_used,
                "confidence":   null,
                "audio_id":     null,
                "source":       "text",
                "target_app":   target_app,
                "output_language": if is_formatter {
                    &prefs.output_language
                } else if tone_override.is_some() {
                    "english"
                } else {
                    &prefs.output_language
                },
                "latency_ms": {
                    "transcribe": 0,
                    "embed":      embed_ms,
                    "retrieve":   0,
                    "polish":     llm_result.polish_ms,
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

async fn polish_message_polish_server(
    state: AppState,
    body: TextPolishBody,
    mode: &str,
) -> Response {
    let mode = mode.to_string();
    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();
    let transcript = body.text.clone();
    let target_app = body.target_app.clone();
    let http_client = state.http_client.clone();

    let stream = async_stream::stream! {
        let total_start = Instant::now();

        yield Ok::<Event, Infallible>(Event::default().event("status")
            .data(json!({"phase": "message_polishing", "transcript": transcript}).to_string()));

        match crate::routes::message_polish::run_server_message_polish(
            &http_client,
            &pool,
            &user_id,
            &transcript,
            None,
            &mode,
        ).await {
            Ok((llm_result, model_used)) => {
                let total_ms = total_start.elapsed().as_millis() as i64;
                let recording_id = Uuid::new_v4().to_string();
                let word_count = llm_result.polished.split_whitespace().count() as i64;

                let pool2 = pool.clone();
                let id2 = recording_id.clone();
                let uid2 = user_id.clone();
                let t2 = transcript.clone();
                let p2 = llm_result.polished.clone();
                let ta2 = target_app.clone();
                let model2 = model_used.clone();
                let p_ms = llm_result.polish_ms as i64;
                tokio::task::spawn_blocking(move || {
                    insert_recording(&pool2, InsertRecording {
                        id: &id2,
                        user_id: &uid2,
                        transcript: &t2,
                        polished: &p2,
                        word_count,
                        recording_seconds: word_count as f64 * 60.0 / 130.0,
                        model_used: &model2,
                        confidence: None,
                        transcribe_ms: Some(0),
                        embed_ms: Some(0),
                        polish_ms: Some(p_ms),
                        target_app: ta2.as_deref(),
                        source: "text",
                        audio_id: None,
                        enriched_transcript: None,
                        raw_transcript: None,
                        local_corrected_transcript: None,
                        polished_output: Some(&p2),
                        trace_json: None,
                    });
                });

                yield Ok(Event::default().event("done").data(
                    json!({
                        "recording_id": recording_id,
                        "transcript": transcript,
                        "polished": llm_result.polished,
                        "model_used": model_used,
                        "source": "text",
                        "target_app": target_app,
                        "output_language": "english",
                        "latency_ms": {
                            "transcribe": 0,
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
                warn!("[text] server message polish failed: {e}");
                yield Ok(Event::default().event("error")
                    .data(json!({"message": e}).to_string()));
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

pub async fn refine_last(
    State(state): State<AppState>,
    Json(body): Json<TextRefineBody>,
) -> impl IntoResponse {
    if body.source_text.trim().is_empty() || body.previous_output.trim().is_empty() {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }

    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();
    let prefs_opt = crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id).await;
    let http_client = state.http_client.clone();
    let source_text = body.source_text.clone();
    let previous_output = body.previous_output.clone();
    let tone_override = body.tone.clone();

    let stream = async_stream::stream! {
        let total_start = Instant::now();
        let prefs = match prefs_opt {
            Some(p) => p,
            None => {
                yield Ok::<Event, Infallible>(Event::default().event("error")
                    .data(json!({"message": "preferences not found"}).to_string()));
                return;
            }
        };

        let tone = tone_override
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| prefs.tone_preset.clone());
        let system_prompt = build_refine_last_transform_prompt(&tone);
        let user_message = build_refine_last_transform_user_message(&source_text, &previous_output);

        yield Ok(Event::default().event("status")
            .data(json!({"phase": "polishing", "transcript": source_text}).to_string()));

        let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
        let gateway_key = prefs.gateway_api_key.clone()
            .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
            .or_else(|| { let k = said_core::api_key(); if k.is_empty() { None } else { Some(k.to_string()) } })
            .unwrap_or_default();
        let gemini_key_text = prefs.gemini_api_key.clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        let groq_key_text = prefs.groq_api_key.clone()
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();

        let sys_p = system_prompt.clone();
        let usr_m = user_message.clone();
        let client_c = http_client.clone();
        let codex_tok = openai_oauth::get_token(&pool, &user_id);
        let (llm_provider, model_for_llm, openai_token_opt) = if let Some(tok) = codex_tok {
            (
                "openai_codex".to_string(),
                openai_codex::MODEL_MINI.to_string(),
                Some(tok.access_token),
            )
        } else {
            ("groq".to_string(), groq::GROQ_MODEL_70B.to_string(), None)
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
                    &client_c, &gemini_key_text, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else if llm_provider_for_task == "groq" {
                groq::stream_polish(
                    &client_c, &groq_key_text, &model_for_llm, &sys_p, &usr_m, token_tx,
                ).await
            } else {
                gateway::stream_polish(&client_c, &gateway_key, &model_for_llm, &sys_p, &usr_m, token_tx).await
            }
        });

        while let Some(token) = token_rx.recv().await {
            yield Ok(Event::default().event("token")
                .data(json!({"token": token}).to_string()));
        }

        let llm_result = match llm_task.await {
            Ok(Ok(r))  => r,
            Ok(Err(e)) => {
                let message = if invalidate_openai_session_on_auth_error(&pool, &user_id, &llm_provider, &e) {
                    "OpenAI not connected — go to Settings to connect your account".to_string()
                } else {
                    crate::llm::llm_error_message(&e)
                };
                warn!("[text-refine] LLM error: {e}");
                yield Ok(Event::default().event("error")
                    .data(llm_error_payload(&e, message).to_string()));
                return;
            }
            Err(_) => {
                yield Ok(Event::default().event("error")
                    .data(json!({"message": "internal error"}).to_string()));
                return;
            }
        };

        let total_ms = total_start.elapsed().as_millis() as i64;
        let recording_id = Uuid::new_v4().to_string();
        let word_count = llm_result.polished.split_whitespace().count() as i64;
        {
            let pool2  = pool.clone();
            let id2    = recording_id.clone();
            let uid2   = user_id.clone();
            let t2     = source_text.clone();
            let p2     = llm_result.polished.clone();
            let model2 = actual_model_used.clone();
            let p_ms   = llm_result.polish_ms as i64;
            tokio::spawn(async move {
                insert_recording(&pool2, InsertRecording {
                    id: &id2, user_id: &uid2,
                    transcript: &t2, polished: &p2,
                    word_count, recording_seconds: word_count as f64 * 60.0 / 130.0,
                    model_used: &model2,
                    confidence:    None,
                    transcribe_ms: None,
                    embed_ms:      None,
                    polish_ms:     Some(p_ms),
                    target_app:    None,
                    source:        "text_refine",
                    audio_id:      None,
                    enriched_transcript: None,
                    raw_transcript: Some(&t2),
                    local_corrected_transcript: Some(&t2),
                    polished_output: Some(&p2),
                    trace_json: None,
                });
            });
        }

        yield Ok(Event::default().event("done").data(
            json!({
                "recording_id": recording_id,
                "transcript":   source_text,
                "polished":     llm_result.polished,
                "model_used":   actual_model_used,
                "confidence":   null,
                "audio_id":     null,
                "source":       "text_refine",
                "target_app":   null,
                "output_language": null,
                "latency_ms": {
                    "transcribe": 0,
                    "embed":      0,
                    "retrieve":   0,
                    "polish":     llm_result.polish_ms,
                    "total":      total_ms,
                },
                "examples_used": 0,
            }).to_string()
        ));
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use crate::llm::{
        prompt::{resolved_vocab_terms_to_entries, vocab_terms_to_entries},
        vocab_resolver,
    };
    use crate::store::vocab_embeddings::upsert_embedding;
    use crate::store::{DbPool, now_ms, stt_replacements, vocab_embeddings};
    use r2d2_sqlite::SqliteConnectionManager;
    use rusqlite::params;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        pool.get()
            .unwrap()
            .execute_batch(
                "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE vocabulary (
                 user_id                 TEXT NOT NULL REFERENCES local_user(id),
                 term                    TEXT NOT NULL,
                 weight                  REAL NOT NULL DEFAULT 1.0,
                 use_count               INTEGER NOT NULL DEFAULT 1,
                 last_used               INTEGER NOT NULL,
                 source                  TEXT NOT NULL DEFAULT 'auto',
                 language                TEXT,
                 example_context         TEXT,
                 term_type               TEXT,
                 meaning                 TEXT,
                 meaning_updated_at      INTEGER,
                 examples_since_meaning  INTEGER NOT NULL DEFAULT 0,
                 UNIQUE(user_id, term)
             );
             CREATE TABLE vocab_embeddings (
                 user_id    TEXT NOT NULL REFERENCES local_user(id),
                 term       TEXT NOT NULL,
                 embedding  BLOB NOT NULL,
                 updated_at INTEGER NOT NULL,
                 UNIQUE(user_id, term)
             );
             CREATE TABLE vocab_embedding_examples (
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id       TEXT NOT NULL REFERENCES local_user(id),
                 term          TEXT NOT NULL,
                 embedding     BLOB NOT NULL,
                 example_text  TEXT NOT NULL,
                 recorded_at   INTEGER NOT NULL
             );
             CREATE VIRTUAL TABLE vocab_fts USING fts5(
                 user_id UNINDEXED, term, example_context,
                 tokenize = 'unicode61 remove_diacritics 2'
             );",
            )
            .unwrap();
        pool
    }

    fn seed_vocab(
        pool: &DbPool,
        term: &str,
        weight: f64,
        context: &str,
        meaning: Option<&str>,
        embedding: &[f32],
    ) {
        pool.get().unwrap().execute(
            "INSERT INTO vocabulary
               (user_id, term, weight, use_count, last_used, source, language, example_context, term_type, meaning)
             VALUES ('u1', ?1, ?2, 1, ?3, 'auto', 'english', ?4, 'proper_noun', ?5)",
            params![term, weight, now_ms(), context, meaning],
        ).unwrap();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocab_fts (user_id, term, example_context)
             VALUES ('u1', ?1, ?2)",
                params![term, context],
            )
            .unwrap();
        upsert_embedding(pool, "u1", term, embedding);
    }

    #[test]
    fn text_prompt_vocab_stays_empty_for_unrelated_top_weight_term() {
        let pool = mem_pool();
        seed_vocab(
            &pool,
            "tembeess",
            5.0,
            "tembeess Friday team meeting",
            Some("Internal project term for a team meeting context."),
            &[1.0, 0.0, 0.0, 0.0],
        );

        let selected = vocab_embeddings::select_for_prompt(
            &pool,
            "u1",
            "english",
            Some(&[0.99, 0.0, 0.0, 0.0]),
            Some("what time is it"),
        );
        let alias_result = stt_replacements::ApplyResult {
            text: "what time is it".into(),
            matches: vec![],
            traces: vec![],
        };
        let resolved = vocab_resolver::resolve_for_prompt(
            &alias_result.text,
            &selected,
            &selected,
            &alias_result,
        );
        let mut chosen = resolved_vocab_terms_to_entries(resolved.resolved_terms);
        chosen.extend(vocab_terms_to_entries(resolved.candidate_terms));
        assert!(
            chosen.is_empty(),
            "text polish should not inject unrelated top-weight vocab"
        );
    }
}
