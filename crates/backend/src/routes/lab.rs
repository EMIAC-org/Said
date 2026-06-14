//! Learning Lab — instrumented pipeline trace for fast iteration.
//!
//! POST /v1/lab/trace — accepts audio, runs the full voice pipeline with
//! instrumentation, returns every intermediate step as JSON (not SSE).
//!
//! These endpoints are PUBLIC (no shared-secret auth) because they're only
//! used from the dev-only learning-lab frontend on localhost.

use axum::{Json, extract::State, http::StatusCode, response::Html, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Instant;
use tracing::info;

use crate::{
    AppState,
    embedder::gemini,
    llm::{
        prompt::{
            build_user_message, default_voice_prompt_template, render_voice_system_prompt_template,
            resolved_vocab_terms_to_entries, vocab_terms_to_entries,
        },
        script, vocab_resolver,
    },
    store::{stt_replacements, vectors::retrieve_similar, vocab_embeddings, vocabulary},
    stt::bias as stt_bias,
};

#[derive(Debug, Deserialize)]
pub struct NumberFormatRequest {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub cases: Vec<NumberFormatCaseInput>,
}

#[derive(Debug, Deserialize)]
pub struct NumberFormatCaseInput {
    #[serde(default)]
    pub id: Option<String>,
    pub raw: String,
    #[serde(default)]
    pub expected: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NumberFormatResponse {
    pub case_count: usize,
    pub changed_count: usize,
    pub pass_count: usize,
    pub fail_count: usize,
    pub cases: Vec<NumberFormatCaseOutput>,
}

#[derive(Debug, Serialize)]
pub struct NumberFormatCaseOutput {
    pub id: String,
    pub raw: String,
    pub formatted: String,
    pub expected: Option<String>,
    pub note: Option<String>,
    pub changed: bool,
    pub passed: Option<bool>,
}

/// GET /v1/lab/number-format or /v1/lab/formatting
/// Serves the local formatter lab. The page exercises Rust formatter code only.
pub async fn number_format_page() -> impl IntoResponse {
    Html(include_str!("../../../../docs/number-format-lab.html"))
}

/// POST /v1/lab/number-format
/// Runs the deterministic number/unit formatter over raw Hinglish cases.
pub async fn number_format(Json(body): Json<NumberFormatRequest>) -> impl IntoResponse {
    run_format_cases(body, crate::formatting::apply_number_units)
}

/// POST /v1/lab/formatting
/// Runs the full deterministic local formatter over raw Hinglish cases.
pub async fn local_formatting(Json(body): Json<NumberFormatRequest>) -> impl IntoResponse {
    run_format_cases(body, crate::formatting::apply_all)
}

fn run_format_cases(
    body: NumberFormatRequest,
    formatter: fn(&str) -> String,
) -> axum::response::Response {
    let mut cases = body.cases;
    if cases.is_empty() {
        if let Some(text) = body.text {
            cases = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .enumerate()
                .map(|(idx, raw)| NumberFormatCaseInput {
                    id: Some(format!("line-{}", idx + 1)),
                    raw: raw.to_string(),
                    expected: None,
                    note: None,
                })
                .collect();
        }
    }

    if cases.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "provide text or cases"})),
        )
            .into_response();
    }

    let mut changed_count = 0usize;
    let mut pass_count = 0usize;
    let mut fail_count = 0usize;
    let outputs: Vec<NumberFormatCaseOutput> = cases
        .into_iter()
        .enumerate()
        .map(|(idx, case)| {
            let formatted = formatter(&case.raw);
            let changed = formatted != case.raw;
            if changed {
                changed_count += 1;
            }
            let passed = case
                .expected
                .as_ref()
                .map(|expected| formatted == *expected);
            match passed {
                Some(true) => pass_count += 1,
                Some(false) => fail_count += 1,
                None => {}
            }
            NumberFormatCaseOutput {
                id: case.id.unwrap_or_else(|| format!("case-{}", idx + 1)),
                raw: case.raw,
                formatted,
                expected: case.expected,
                note: case.note,
                changed,
                passed,
            }
        })
        .collect();

    Json(NumberFormatResponse {
        case_count: outputs.len(),
        changed_count,
        pass_count,
        fail_count,
        cases: outputs,
    })
    .into_response()
}

#[derive(Serialize)]
pub struct SttReplacementTrace {
    pub from: String,
    pub to: String,
    pub method: String,
}

#[derive(Serialize)]
pub struct VocabSelectedTrace {
    pub term: String,
    pub term_type: Option<String>,
    pub weight: f64,
    pub meaning: Option<String>,
    pub resolution: String,
}

#[derive(Serialize)]
pub struct VocabRejectedTrace {
    pub term: String,
    pub reason: String,
    pub best_phonetic_sim: f64,
}

#[derive(Serialize)]
pub struct PipelineTrace {
    // Stage 1: STT
    pub deepgram_raw: String,
    pub stt_confidence: f64,
    pub stt_ms: i64,

    // Stage 2: STT Replacements
    pub stt_replacements_applied: Vec<SttReplacementTrace>,
    pub stt_near_misses: Vec<SttNearMissTrace>,
    pub transcript_after_replacements: String,

    // Stage 3: Vocabulary Selection
    pub vocab_selected: Vec<VocabSelectedTrace>,
    pub vocab_rejected_sample: Vec<VocabRejectedTrace>,

    // Stage 4: Prompt
    pub system_prompt_preview: String,
    pub system_prompt_length: usize,
    pub rag_examples_used: usize,

    // Stage 5: LLM Polish
    pub polished: String,
    pub llm_model: String,
    pub llm_ms: i64,

    // Stage 6: Post-processing
    pub devanagari_detected: bool,
    pub format_recovered: bool,
    pub content_guard_triggered: bool,

    // Final
    pub final_output: String,
    pub total_ms: i64,
}

#[derive(Serialize)]
pub struct SttNearMissTrace {
    pub rule_from: String,
    pub rule_to: String,
    pub best_token: String,
    pub similarity: f64,
}

/// POST /v1/lab/trace
pub async fn trace(
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    let total_start = Instant::now();

    // Extract audio from multipart
    let mut wav_data: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("audio") {
            match field.bytes().await {
                Ok(b) => wav_data = b.to_vec(),
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": format!("failed to read audio: {e}")})),
                    )
                        .into_response();
                }
            }
        }
    }

    if wav_data.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no audio field in request"})),
        )
            .into_response();
    }

    let user_id = state.default_user_id.as_str().to_string();
    let pool = state.pool.clone();
    let http_client = state.http_client.clone();

    // Load prefs + lexicon + vocab in parallel
    let vocab_task = {
        let pool_c = pool.clone();
        let uid_c = user_id.clone();
        tokio::task::spawn_blocking(move || vocabulary::top_terms(&pool_c, &uid_c, 100))
    };
    let (prefs_opt, (word_corrections, stt_replacement_rules), vocab_full) = tokio::join!(
        crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id),
        crate::get_lexicon_cached(&state.lexicon_cache, &pool, &user_id),
        async { vocab_task.await.unwrap_or_default() },
    );

    let prefs = match prefs_opt {
        Some(p) => p,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "preferences not found"})),
            )
                .into_response();
        }
    };

    let deepgram_key = prefs
        .deepgram_api_key
        .clone()
        .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok())
        .unwrap_or_default();
    let gateway_key = prefs
        .gateway_api_key
        .clone()
        .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
        .or_else(|| {
            let k = said_core::api_key();
            if k.is_empty() {
                None
            } else {
                Some(k.to_string())
            }
        })
        .unwrap_or_default();
    let groq_key = prefs
        .groq_api_key
        .clone()
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
        .unwrap_or_default();
    let gemini_key = prefs
        .gemini_api_key
        .clone()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .unwrap_or_default();

    // Build bias package
    let stt_bias_package = tokio::task::spawn_blocking({
        let pool = pool.clone();
        let user_id = user_id.clone();
        let language = prefs.language.clone();
        let output_language = prefs.output_language.clone();
        move || stt_bias::build_bias_package(&pool, &user_id, &language, &output_language)
    })
    .await
    .unwrap_or_default();

    // ── STAGE 1: STT ────────────────────────────────────────────────────────────
    let stt_start = Instant::now();
    let stt_result = crate::stt::deepgram::transcribe_any_format(
        &http_client,
        &deepgram_key,
        wav_data,
        &stt_bias_package,
    )
    .await;

    let stt_result = match stt_result {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("STT failed: {e}")})),
            )
                .into_response();
        }
    };
    let stt_ms = stt_start.elapsed().as_millis() as i64;
    let deepgram_raw = stt_result.transcript.clone();
    let stt_confidence = stt_result.confidence;
    info!("[lab] STT done in {}ms: {:?}", stt_ms, deepgram_raw);

    // ── STAGE 2: Local corrections ──────────────────────────────────────────────
    let (transcript_after_replacements, replacement_traces, near_misses) =
        if stt_replacement_rules.is_empty() && vocab_full.is_empty() {
            (deepgram_raw.clone(), vec![], vec![])
        } else {
            let pool_t = pool.clone();
            let uid_t = user_id.clone();
            let deepgram_t = deepgram_raw.clone();
            let rules_t = stt_replacement_rules.clone();
            let vocab_t = vocab_full.clone();
            let alias_result = tokio::task::spawn_blocking(move || {
                crate::tier2::correct_with_store(&pool_t, &uid_t, &deepgram_t, &rules_t, &vocab_t)
            })
            .await
            .unwrap_or_else(|_| {
                stt_replacements::apply_exact_safe(&deepgram_raw, &stt_replacement_rules)
            });

            let traces: Vec<SttReplacementTrace> = alias_result
                .matches
                .iter()
                .map(|m| SttReplacementTrace {
                    from: m.transcript_form.clone(),
                    to: m.correct_form.clone(),
                    method: m.kind.as_str().to_string(),
                })
                .collect();

            // Compute near-misses for rules that DIDN'T fire
            let matched_forms: std::collections::HashSet<&str> = alias_result
                .matches
                .iter()
                .map(|m| m.transcript_form.as_str())
                .collect();
            let transcript_tokens: Vec<&str> = deepgram_raw.split_whitespace().collect();
            let mut near_miss_traces = vec![];
            for rule in &stt_replacement_rules {
                if matched_forms.contains(rule.transcript_form.as_str()) {
                    continue;
                }
                let mut best_sim = 0.0_f64;
                let mut best_token = String::new();
                for tok in &transcript_tokens {
                    let tok_core = tok
                        .trim_matches(|c: char| !c.is_alphanumeric())
                        .to_ascii_lowercase();
                    if tok_core.is_empty() {
                        continue;
                    }
                    let sim = crate::llm::phonetics::similarity(&tok_core, &rule.transcript_form);
                    if sim > best_sim {
                        best_sim = sim;
                        best_token = tok.to_string();
                    }
                }
                if best_sim > 0.3 {
                    near_miss_traces.push(SttNearMissTrace {
                        rule_from: rule.transcript_form.clone(),
                        rule_to: rule.correct_form.clone(),
                        best_token,
                        similarity: best_sim,
                    });
                }
            }

            (alias_result.text, traces, near_miss_traces)
        };

    // ── STAGE 3: Embedding + Vocab Selection ────────────────────────────────────
    let embedding = gemini::cached(&pool, &transcript_after_replacements).await;
    let rag_examples = match &embedding {
        Some(emb) => {
            let emb_clone = emb.clone();
            let pool_rag = pool.clone();
            let uid_rag = user_id.clone();
            tokio::task::spawn_blocking(move || {
                retrieve_similar(&pool_rag, &uid_rag, &emb_clone, 5, 0.65)
            })
            .await
            .unwrap_or_default()
        }
        None => vec![],
    };

    let alias_result_for_resolver = if stt_replacement_rules.is_empty() {
        stt_replacements::ApplyResult {
            text: deepgram_raw.clone(),
            matches: vec![],
            traces: vec![],
        }
    } else {
        stt_replacements::apply_with_matches(&deepgram_raw, &stt_replacement_rules)
    };

    let (resolved_transcript, vocab_entries, vocab_selected_traces, vocab_rejected_traces) = {
        let pool_v = pool.clone();
        let uid_v = user_id.clone();
        let lang_v = prefs.output_language.clone();
        let emb_v = embedding.clone();
        let txt_v = alias_result_for_resolver.text.clone();
        let chosen = tokio::task::spawn_blocking(move || {
            vocab_embeddings::select_for_prompt(
                &pool_v,
                &uid_v,
                &lang_v,
                emb_v.as_deref(),
                Some(&txt_v),
            )
        })
        .await
        .unwrap_or_default();

        // Build rejected traces
        let chosen_terms: std::collections::HashSet<String> =
            chosen.iter().map(|t| t.term.to_ascii_lowercase()).collect();
        let transcript_lower = alias_result_for_resolver.text.to_ascii_lowercase();
        let transcript_tokens: Vec<String> = transcript_lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let mut rejected_traces = vec![];
        for vt in &vocab_full {
            if chosen_terms.contains(&vt.term.to_ascii_lowercase()) {
                continue;
            }
            let has_meaning = vt
                .meaning
                .as_deref()
                .map(|m| !m.trim().is_empty())
                .unwrap_or(false);
            let term_lower = vt.term.to_ascii_lowercase();
            let is_substring = transcript_lower.contains(&term_lower);
            let best_phon = transcript_tokens
                .iter()
                .map(|tok| crate::llm::phonetics::similarity(tok, &vt.term))
                .fold(0.0_f64, f64::max);
            let reason = if !has_meaning {
                "no_meaning"
            } else if is_substring {
                "substring_ok(BM25_miss?)"
            } else if best_phon >= 0.70 {
                "phonetic_ok(BM25_miss?)"
            } else {
                "phonetic_too_low"
            };
            rejected_traces.push(VocabRejectedTrace {
                term: vt.term.clone(),
                reason: reason.to_string(),
                best_phonetic_sim: best_phon,
            });
        }

        if chosen.is_empty() {
            (
                alias_result_for_resolver.text.clone(),
                vec![],
                vec![],
                rejected_traces,
            )
        } else {
            let resolved = vocab_resolver::resolve_for_prompt(
                &alias_result_for_resolver.text,
                &chosen,
                &vocab_full,
                &alias_result_for_resolver,
            );
            let mut selected_traces: Vec<VocabSelectedTrace> = resolved
                .resolved_terms
                .iter()
                .map(|t| VocabSelectedTrace {
                    term: t.term.clone(),
                    term_type: t.term_type.clone(),
                    weight: t.weight,
                    meaning: t.meaning.clone(),
                    resolution: "resolved".to_string(),
                })
                .collect();
            selected_traces.extend(resolved.candidate_terms.iter().map(|t| VocabSelectedTrace {
                term: t.term.clone(),
                term_type: t.term_type.clone(),
                weight: t.weight,
                meaning: t.meaning.clone(),
                resolution: "candidate".to_string(),
            }));

            let mut entries = resolved_vocab_terms_to_entries(resolved.resolved_terms);
            entries.extend(vocab_terms_to_entries(resolved.candidate_terms));
            (
                resolved.transcript,
                entries,
                selected_traces,
                rejected_traces,
            )
        }
    };

    // ── STAGE 4: Build prompt ───────────────────────────────────────────────────
    let user_message = build_user_message(&resolved_transcript, &prefs.output_language);
    let default_prompt_body = default_voice_prompt_template();
    let prompt_body = {
        let pool_p = pool.clone();
        let uid_p = user_id.clone();
        let body_p = default_prompt_body.clone();
        tokio::task::spawn_blocking(move || {
            crate::store::prompt_templates::active_body_or_default(
                &pool_p,
                &uid_p,
                crate::store::prompt_templates::DefaultPrompt {
                    kind: crate::llm::prompt::VOICE_PROMPT_KIND,
                    title: crate::llm::prompt::VOICE_PROMPT_TITLE,
                    base_version: crate::llm::prompt::VOICE_PROMPT_BASE_VERSION,
                    body: &body_p,
                },
            )
        })
        .await
        .unwrap_or(default_prompt_body)
    };
    let system_prompt = render_voice_system_prompt_template(
        &prompt_body,
        &prefs,
        &rag_examples,
        &word_corrections,
        &vocab_entries,
    );

    let system_prompt_length = system_prompt.len();
    let system_prompt_preview = if system_prompt.len() > 1500 {
        format!("{}…", &system_prompt[..1500])
    } else {
        system_prompt.clone()
    };

    // ── STAGE 5: LLM Polish ────────────────────────────────────────────────────
    let llm_start = Instant::now();
    let model = said_core::resolve_model(&prefs.selected_model).to_string();
    let llm_provider = prefs.llm_provider.clone();

    let (model_for_llm, openai_token_opt) = if llm_provider == "openai_codex" {
        let pool_tok = pool.clone();
        let uid_tok = user_id.clone();
        let tok = tokio::task::spawn_blocking(move || {
            crate::store::openai_oauth::get_token(&pool_tok, &uid_tok)
        })
        .await
        .unwrap_or(None);
        (
            crate::llm::openai_codex::MODEL_MINI.to_string(),
            tok.map(|t| t.access_token),
        )
    } else if llm_provider == "gemini_direct" {
        (
            crate::llm::gemini_direct::GEMINI_DIRECT_MODEL.to_string(),
            None,
        )
    } else if llm_provider == "groq" {
        (
            if prefs.selected_model == "smart" {
                crate::llm::groq::GROQ_MODEL_SMART
            } else {
                crate::llm::groq::GROQ_MODEL_FAST
            }
            .to_string(),
            None,
        )
    } else {
        (model.clone(), None)
    };

    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel::<String>(64);
    let sys_p = system_prompt.clone();
    let usr_m = user_message.clone();
    let client_c = http_client.clone();
    let llm_provider_for_task = llm_provider.clone();
    let gk = gateway_key.clone();
    let gk_gemini = gemini_key.clone();
    let gk_groq = groq_key.clone();
    let model_for_llm_c = model_for_llm.clone();

    let llm_task = tokio::spawn(async move {
        if llm_provider_for_task == "openai_codex" {
            let access_token = openai_token_opt.as_deref().unwrap_or("");
            if access_token.is_empty() {
                return Err("OpenAI not connected".to_string());
            }
            crate::llm::openai_codex::stream_polish(
                &client_c,
                access_token,
                &model_for_llm_c,
                &sys_p,
                &usr_m,
                token_tx,
            )
            .await
        } else if llm_provider_for_task == "gemini_direct" {
            crate::llm::gemini_direct::stream_polish(
                &client_c,
                &gk_gemini,
                &model_for_llm_c,
                &sys_p,
                &usr_m,
                token_tx,
            )
            .await
        } else if llm_provider_for_task == "groq" {
            crate::llm::groq::stream_polish(
                &client_c,
                &gk_groq,
                &model_for_llm_c,
                &sys_p,
                &usr_m,
                token_tx,
            )
            .await
        } else {
            crate::llm::gateway::stream_polish(
                &client_c,
                &gk,
                &model_for_llm_c,
                &sys_p,
                &usr_m,
                token_tx,
            )
            .await
        }
    });

    // Drain token channel (we don't stream in lab mode, just collect)
    let mut tokens = Vec::new();
    while let Some(tok) = token_rx.recv().await {
        tokens.push(tok);
    }

    let llm_result = match llm_task.await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("LLM failed: {e}")})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("LLM task panicked: {e}")})),
            )
                .into_response();
        }
    };
    let llm_ms = llm_start.elapsed().as_millis() as i64;
    let polished_raw = llm_result.polished.clone();

    // ── STAGE 6: Post-processing ────────────────────────────────────────────────
    let enforce_roman_hinglish = prefs.output_language == "hinglish";
    let mut final_output = polished_raw.clone();

    let devanagari_detected = enforce_roman_hinglish && script::contains_devanagari(&final_output);
    if devanagari_detected {
        final_output =
            match crate::llm::devanagari_recovery::recover(&http_client, &groq_key, &final_output)
                .await
            {
                Ok(recovered) => recovered,
                Err(_) => script::enforce_roman_hinglish(&final_output),
            };
    }

    let transcript_wc = resolved_transcript.split_whitespace().count();
    let polished_wc = final_output.split_whitespace().count();
    let content_guard_triggered = transcript_wc > 4 && polished_wc < transcript_wc / 2;
    if content_guard_triggered {
        let mut cleaned = crate::routes::voice::strip_confidence_markers(&resolved_transcript);
        if enforce_roman_hinglish && script::contains_devanagari(&cleaned) {
            cleaned = script::enforce_roman_hinglish(&cleaned);
        }
        final_output = cleaned;
    }

    let recovered = crate::llm::format_recover::recover(&final_output);
    let format_recovered = recovered != final_output;
    if format_recovered {
        final_output = recovered;
    }

    let total_ms = total_start.elapsed().as_millis() as i64;

    info!(
        "[lab] trace complete in {}ms — stt={}ms llm={}ms",
        total_ms, stt_ms, llm_ms
    );

    let trace = PipelineTrace {
        deepgram_raw,
        stt_confidence,
        stt_ms,
        stt_replacements_applied: replacement_traces,
        stt_near_misses: near_misses,
        transcript_after_replacements,
        vocab_selected: vocab_selected_traces,
        vocab_rejected_sample: vocab_rejected_traces,
        system_prompt_preview,
        system_prompt_length,
        rag_examples_used: rag_examples.len(),
        polished: polished_raw,
        llm_model: model_for_llm,
        llm_ms,
        devanagari_detected,
        format_recovered,
        content_guard_triggered,
        final_output,
        total_ms,
    };

    Json(trace).into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Chaos endpoints — simulate failure modes for watchdog testing.
// These are PUBLIC (lab-only, no auth) and only useful during development.
// ═══════════════════════════════════════════════════════════════════════════════

/// POST /v1/lab/chaos/tokio-starve
/// Spawns N blocking sleeps directly on tokio worker threads, starving the
/// runtime for `duration_s` seconds.
pub async fn chaos_tokio_starve(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let threads = body.get("threads").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
    let duration_s = body
        .get("duration_s")
        .and_then(|v| v.as_u64())
        .unwrap_or(15);
    let dur = std::time::Duration::from_secs(duration_s);

    info!("[chaos] tokio-starve: blocking {threads} workers for {duration_s}s");

    for _ in 0..threads {
        let d = dur;
        tokio::spawn(async move {
            std::thread::sleep(d);
        });
    }

    Json(json!({
        "chaos": "tokio-starve",
        "threads": threads,
        "duration_s": duration_s,
    }))
    .into_response()
}

/// POST /v1/lab/chaos/pool-exhaust
/// Grabs all SQLite connections and holds them for `duration_s` seconds.
pub async fn chaos_pool_exhaust(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let duration_s = body
        .get("duration_s")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);
    let dur = std::time::Duration::from_secs(duration_s);

    info!("[chaos] pool-exhaust: holding all connections for {duration_s}s");

    for _ in 0..5 {
        let pool = state.pool.clone();
        let d = dur;
        tokio::task::spawn_blocking(move || {
            if let Ok(_conn) = pool.get() {
                std::thread::sleep(d);
            }
        });
    }

    Json(json!({
        "chaos": "pool-exhaust",
        "connections_held": 5,
        "duration_s": duration_s,
    }))
    .into_response()
}

/// POST /v1/lab/chaos/sse-stall
/// Returns an SSE stream that sends one event then hangs forever, simulating
/// a stuck LLM stream.
pub async fn chaos_sse_stall() -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::stream::Stream;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    info!("[chaos] sse-stall: starting stalled SSE stream");

    struct StallStream {
        sent_first: bool,
    }
    impl Stream for StallStream {
        type Item = Result<Event, std::convert::Infallible>;
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if !self.sent_first {
                self.sent_first = true;
                return Poll::Ready(Some(Ok(
                    Event::default().data("[chaos] stream will now stall indefinitely")
                )));
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    Sse::new(StallStream { sent_first: false })
        .keep_alive(KeepAlive::default())
        .into_response()
}
