//! POST /v1/confirm-term     — user confirms an ambiguous term for vocabulary
//! POST /v1/block-correction — user blocks a wrong correction rule

use axum::{Json, extract::State, http::StatusCode};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    AppState,
    embedder::gemini,
    llm::{alias_safety, meaning, promotion_gate},
    store::{
        corrections, edit_review_sessions, openai_oauth, prefs::get_prefs, stt_replacements,
        tier2_edit_policy, users, vocab_embeddings, vocab_fts, vocabulary,
    },
};

const VOCAB_CONTEXT_MAX_CHARS: usize = 500;

fn clean_vocab_context(context: Option<&str>) -> Option<String> {
    let raw = context?.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.chars().count() > VOCAB_CONTEXT_MAX_CHARS {
        Some(
            raw.chars()
                .take(VOCAB_CONTEXT_MAX_CHARS)
                .collect::<String>()
                + "…",
        )
    } else {
        Some(raw.to_string())
    }
}

fn unsafe_confirmed_correction_source(original: &str, corrected: &str) -> bool {
    corrected.trim().is_empty()
        || original.split_whitespace().count() > 4
        || corrected.split_whitespace().count() > 4
        || ((alias_safety::is_common_alias_source(original)
            || promotion_gate::is_common_word(original))
            && matches!(
                vocabulary::classify_term_type(corrected),
                "brand" | "acronym" | "proper_noun" | "code_identifier"
            ))
}

fn groq_key_for_learning(prefs: Option<&crate::store::prefs::Preferences>) -> String {
    prefs
        .and_then(|p| p.groq_api_key.clone())
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
        .or_else(|| {
            // Only pass true Groq keys to the Groq endpoint. Generic gateway or
            // Cerebras keys belong to the server-runtime fallback below.
            std::env::var("GATEWAY_API_KEY")
                .ok()
                .filter(|key| key.trim_start().starts_with("gsk_"))
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct RuntimeMeaningResponse {
    meaning: String,
}

async fn generate_meaning_via_runtime(
    state: &AppState,
    user_id: &str,
    term: &str,
    context: &str,
) -> Option<String> {
    let Some(user) = users::get_user(&state.pool, user_id) else {
        return None;
    };
    let Some(token) = user.cloud_token.filter(|value| !value.trim().is_empty()) else {
        info!("[vocab-meaning] skipped runtime meaning for {term:?} — no cloud token");
        return None;
    };
    let base_url = user
        .enterprise_server_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
    let url = format!(
        "{}/v1/runtime/learning/meaning",
        base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "term": term,
        "context": context,
        "selected_model": said_core::polish::model::DEFAULT_POLISH_MODEL_KEY,
    });
    match state
        .http_client
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<RuntimeMeaningResponse>().await
        {
            Ok(parsed) if !parsed.meaning.trim().is_empty() => Some(parsed.meaning),
            Ok(_) => {
                warn!("[vocab-meaning] runtime returned empty meaning for {term:?}");
                None
            }
            Err(err) => {
                warn!("[vocab-meaning] runtime meaning parse failed for {term:?}: {err}");
                None
            }
        },
        Ok(resp) => {
            let status = resp.status();
            let preview = resp.text().await.unwrap_or_default();
            warn!(
                "[vocab-meaning] runtime meaning failed for {term:?}: {status} {}",
                said_core::text::truncate_utf8(&preview, 180)
            );
            None
        }
        Err(err) => {
            warn!("[vocab-meaning] runtime meaning request failed for {term:?}: {err}");
            None
        }
    }
}

fn schedule_vocab_artifacts(
    state: AppState,
    user_id: String,
    term: String,
    context: Option<String>,
) {
    let Some(context) = context else {
        info!("[vocab-meaning] skipped {term:?} — no example context");
        return;
    };
    tokio::spawn(async move {
        let _guard = crate::bg_task_guard();
        if state.watchdog.is_shedding() {
            info!("[vocab-meaning] skipped {term:?} — watchdog shedding load");
            return;
        }

        vocab_fts::upsert(&state.pool, &user_id, &term, Some(&context));

        let prefs = get_prefs(&state.pool, &user_id);
        let gemini_key = prefs
            .as_ref()
            .and_then(|p| p.gemini_api_key.clone())
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        let embed_text = format!("{term}. {context}");
        if !gemini_key.trim().is_empty() {
            if let Some(embedding) =
                gemini::embed(&state.http_client, &state.pool, &embed_text, &gemini_key).await
            {
                vocab_embeddings::record_example_and_recentre(
                    &state.pool,
                    &user_id,
                    &term,
                    &embedding,
                    &embed_text,
                );
            }
        }

        let example_count = vocabulary::bump_examples_since_meaning(&state.pool, &user_id, &term);
        if !vocabulary::meaning_needs_refresh(&state.pool, &user_id, &term) {
            info!(
                "[vocab-meaning] deferred refresh for {term:?} — examples_since_meaning={example_count}"
            );
            return;
        }

        let groq_key = groq_key_for_learning(prefs.as_ref());
        let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
        let codex_token = openai_oauth::get_token(&state.pool, &user_id);
        let codex_access_token = codex_token
            .as_ref()
            .map(|token| token.access_token.as_str());
        let current = vocabulary::get_meaning(&state.pool, &user_id, &term);
        let examples = {
            let stored = vocab_embeddings::support_example_texts(&state.pool, &user_id, &term, 4);
            if stored.is_empty() {
                vec![context.clone()]
            } else {
                stored
            }
        };

        let generated_local = if let Some(current_meaning) = current.as_deref() {
            meaning::refine(
                &state.http_client,
                &groq_key,
                &openai_key,
                codex_access_token,
                &term,
                current_meaning,
                &examples,
            )
            .await
        } else {
            meaning::generate_initial(
                &state.http_client,
                &groq_key,
                &openai_key,
                codex_access_token,
                &term,
                examples.first().map(String::as_str).unwrap_or(&context),
            )
            .await
        };
        let generated = if generated_local.is_some() {
            generated_local
        } else {
            generate_meaning_via_runtime(&state, &user_id, &term, &context).await
        };

        if let Some(new_meaning) = generated.filter(|value| !value.trim().is_empty()) {
            if vocabulary::update_meaning(&state.pool, &user_id, &term, &new_meaning) {
                crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
                refresh_local_profile_summary(&state, "vocab_meaning");
            }
        }
    });
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn protected_term_type(term_type: &str) -> bool {
    matches!(
        term_type,
        "brand" | "acronym" | "proper_noun" | "code_identifier"
    )
}

fn protected_phrase(term: &str) -> bool {
    let tokens: Vec<&str> = term.split_whitespace().collect();
    tokens.len() > 1
        && tokens
            .iter()
            .any(|token| protected_term_type(vocabulary::classify_term_type(token.trim())))
}

fn source_contains_different_protected_term(
    pool: &crate::store::DbPool,
    user_id: &str,
    source: &str,
    corrected: &str,
) -> bool {
    let corrected_norm = tier2_edit_policy::normalize_token(corrected);
    source
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|part| !part.trim().is_empty())
        .any(|part| {
            vocabulary::find_by_term_ci(pool, user_id, part.trim()).is_some_and(|existing| {
                tier2_edit_policy::normalize_token(&existing.term) != corrected_norm
                    && existing
                        .term_type
                        .as_deref()
                        .is_some_and(protected_term_type)
            })
        })
}

fn deterministic_confirm_alias_allowed(
    pool: &crate::store::DbPool,
    user_id: &str,
    original: &str,
    corrected: &str,
    inferred_term_type: &str,
) -> bool {
    let original = original.trim();
    let corrected = corrected.trim();
    if original.is_empty()
        || corrected.is_empty()
        || tier2_edit_policy::normalize_token(original)
            == tier2_edit_policy::normalize_token(corrected)
    {
        return false;
    }
    if alias_safety::is_common_alias_source(original)
        || promotion_gate::is_common_word(original)
        || crate::tier2::is_in_dictionary(&alias_safety::normalize_source(original))
    {
        return false;
    }
    if source_contains_different_protected_term(pool, user_id, original, corrected) {
        return false;
    }

    let existing_target = vocabulary::find_by_term_ci(pool, user_id, corrected);
    let target_type = existing_target
        .as_ref()
        .and_then(|term| term.term_type.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(inferred_term_type);

    protected_term_type(target_type)
        || (target_type == "phrase" && protected_phrase(corrected))
        || existing_target.is_some_and(|term| {
            term.term_type.as_deref().is_some_and(|kind| {
                protected_term_type(kind) || (kind == "phrase" && protected_phrase(&term.term))
            })
        })
}

fn post_runtime_memory_dirty(state: AppState) {
    tokio::spawn(async move {
        let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
            return;
        };
        let Some(token) = user.cloud_token.filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let base_url = user
            .enterprise_server_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
        let url = format!("{}/v1/runtime/memory/dirty", base_url.trim_end_matches('/'));
        let _ = state
            .http_client
            .post(url)
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
    });
}

fn post_runtime_client_event(
    state: AppState,
    event_type: &'static str,
    recording_id: Option<String>,
    classification: &'static str,
    input_hash: String,
    corrected_hash: Option<String>,
    payload: serde_json::Value,
) {
    tokio::spawn(async move {
        let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
            return;
        };
        let Some(token) = user.cloud_token.filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let base_url = user
            .enterprise_server_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
        let url = format!(
            "{}/v1/runtime/client-events",
            base_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "event_type": event_type,
            "recording_id": recording_id,
            "classification": classification,
            "input_hash": input_hash,
            "corrected_hash": corrected_hash,
            "payload": payload,
        });
        match state
            .http_client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!("[confirm] runtime client event uploaded");
            }
            Ok(resp) => {
                warn!(
                    "[confirm] runtime client event upload failed: {}",
                    resp.status()
                );
            }
            Err(e) => {
                warn!("[confirm] runtime client event upload failed: {e}");
            }
        }
    });
}

fn refresh_local_profile_summary(state: &AppState, source: &str) {
    match crate::store::profile_summary::rebuild(&state.pool, state.default_user_id.as_str()) {
        Some(summary) => info!(
            "[profile-summary] refreshed source={source} version={} chars={} counts={}",
            summary.version,
            summary.profile_markdown.chars().count(),
            summary.source_counts_json,
        ),
        None => warn!("[profile-summary] refresh failed source={source}"),
    }
}

// ── POST /v1/confirm-term ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConfirmBody {
    pub term: String,
    pub original: String,
    pub action: String,
    pub recording_id: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Serialize)]
pub struct ConfirmResponse {
    pub confirmed: bool,
    pub term: String,
}

pub async fn confirm_term(
    State(state): State<AppState>,
    Json(body): Json<ConfirmBody>,
) -> (StatusCode, Json<ConfirmResponse>) {
    let user_id = state.default_user_id.as_str();
    let learning_enabled = get_prefs(&state.pool, user_id)
        .map(|p| p.learning_enabled)
        .unwrap_or(true);

    if body.action == "learn" {
        if !learning_enabled {
            info!("[confirm] local learn blocked — user learning disabled");
            return (
                StatusCode::OK,
                Json(ConfirmResponse {
                    confirmed: false,
                    term: body.term,
                }),
            );
        }
        if crate::legacy_learning::audit_only_legacy_mutations() {
            info!(
                "[confirm] local learn blocked — learning disabled for {:?}",
                body.term
            );
            return (
                StatusCode::OK,
                Json(ConfirmResponse {
                    confirmed: false,
                    term: body.term,
                }),
            );
        }

        // ── Resolve output language from preferences ─────────────────────────
        let prefs = get_prefs(&state.pool, user_id);
        let language = prefs
            .as_ref()
            .map(|p| p.output_language.clone())
            .unwrap_or_else(|| "hinglish".into());
        let groq_key = prefs
            .as_ref()
            .and_then(|p| p.groq_api_key.clone())
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
            .unwrap_or_default();

        // ── Promote to vocabulary ────────────────────────────────────────────
        let example_context = clean_vocab_context(body.context.as_deref());
        vocabulary::upsert_for_language_with_context(
            &state.pool,
            user_id,
            &body.term,
            1.0,
            "confirmed",
            &language,
            example_context.as_deref(),
        );

        let alias_safe = if body.original.trim().is_empty() {
            false
        } else {
            crate::llm::alias_safety::judge_alias_source(
                &state.http_client,
                &state.pool,
                user_id,
                &groq_key,
                &body.original,
                &body.term,
                None,
            )
            .await
            .allows_learning()
        };

        if alias_safe {
            // ── Record edit-policy rule ──────────────────────────────────────
            tier2_edit_policy::record_explicit_edit(
                &state.pool,
                user_id,
                &body.original,
                &body.term,
                "replace",
                &[],
                &[],
                body.recording_id.as_deref(),
            );

            // ── Create STT alias ────────────────────────────────────────────
            stt_replacements::upsert_aliases_for_language(
                &state.pool,
                user_id,
                &body.original,
                &body.original,
                &body.term,
                1.0,
                &language,
            );
            stt_replacements::mark_confirmed_aliases_for_language(
                &state.pool,
                user_id,
                &body.original,
                &body.original,
                &body.term,
                &language,
            );

            // ── Proactive distortion seeding ────────────────────────────────
            let proactive = stt_replacements::generate_proactive_distortions(
                &state.pool,
                user_id,
                &body.term,
                &body.original,
                &language,
            );
            if proactive > 0 {
                info!(
                    "[confirm] seeded {proactive} proactive distortion(s) for {:?}",
                    body.term
                );
            }
        } else if !body.original.trim().is_empty() {
            info!(
                "[confirm] skipped alias learning {:?} → {:?} — alias safety blocked source",
                body.original, body.term,
            );
        }

        // ── Auto-approve aliases (user explicitly confirmed) ────────────────
        let approved = stt_replacements::approve_aliases_for_term(&state.pool, user_id, &body.term);
        if approved > 0 {
            info!(
                "[confirm] auto-approved {approved} alias(es) for {:?}",
                body.term
            );
        }

        // ── Auto-activate edit-policy rules ─────────────────────────────────
        let activated = tier2_edit_policy::activate_all_for_term(&state.pool, user_id, &body.term);
        if activated > 0 {
            info!(
                "[confirm] auto-activated {activated} edit-policy rule(s) for {:?}",
                body.term
            );
        }

        // ── Invalidate lexicon cache ─────────────────────────────────────────
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
        refresh_local_profile_summary(&state, "confirm_term");
        schedule_vocab_artifacts(
            state.clone(),
            user_id.to_string(),
            body.term.clone(),
            example_context,
        );

        // ── Trigger retrain ─────────────────────────────────────────────────
        crate::routes::classify::schedule_retrain_public(state.clone());

        info!(
            "[confirm] user confirmed {:?} — promoted to vocabulary",
            body.term,
        );

        let term_type = vocabulary::classify_term_type(&body.term).to_string();
        let accepted_aliases = if alias_safe {
            serde_json::json!([{
                "transcript_form": body.original,
                "correct_form": body.term,
                "edit_type": "replace",
                "term_type": term_type,
                "source": "local_confirm_modal",
            }])
        } else {
            serde_json::json!([])
        };
        post_runtime_client_event(
            state.clone(),
            "classify_edit_result",
            body.recording_id.clone(),
            "STT_ERROR",
            hash_text(&body.original),
            Some(hash_text(&body.term)),
            serde_json::json!({
                "learned": true,
                "notify": true,
                "source": "confirm_term",
                "promoted_count": 1,
                "promoted_term_count": 1,
                "capture_method": "user_confirmed_modal",
                "memory": {
                    "accepted_terms": [{
                        "term": body.term,
                        "term_type": term_type,
                        "weight": 1.0,
                        "source": "local_confirm_modal",
                    }],
                    "accepted_aliases": accepted_aliases,
                },
            }),
        );
        post_runtime_memory_dirty(state.clone());

        (
            StatusCode::OK,
            Json(ConfirmResponse {
                confirmed: true,
                term: body.term,
            }),
        )
    } else {
        // action == "skip"
        info!(
            "[confirm] user skipped {:?} — marked as style preference",
            body.term,
        );

        (
            StatusCode::OK,
            Json(ConfirmResponse {
                confirmed: false,
                term: body.term,
            }),
        )
    }
}

// ── POST /v1/block-correction ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct BlockBody {
    pub variant: String,
    pub wrong_replacement: String,
}

#[derive(Serialize)]
pub struct BlockResponse {
    pub blocked: bool,
}

pub async fn block_correction(
    State(state): State<AppState>,
    Json(body): Json<BlockBody>,
) -> (StatusCode, Json<BlockResponse>) {
    let user_id = state.default_user_id.as_str();
    let learning_enabled = get_prefs(&state.pool, user_id)
        .map(|p| p.learning_enabled)
        .unwrap_or(true);
    if !learning_enabled || crate::legacy_learning::audit_only_legacy_mutations() {
        info!("[confirm] block_correction skipped — user learning disabled");
        return (StatusCode::OK, Json(BlockResponse { blocked: false }));
    }
    let variant_norm = tier2_edit_policy::normalize_token(&body.variant);
    let replacement_norm = tier2_edit_policy::normalize_token(&body.wrong_replacement);

    // ── Block the edit-policy rule ────────────────────────────────────────────
    // Directly set negative_count to BLOCK_NEGATIVES so status becomes "blocked".
    if !variant_norm.is_empty() && !replacement_norm.is_empty() {
        if let Ok(conn) = state.pool.get() {
            let _ = conn.execute(
                "UPDATE tier2_edit_policy_rules
                    SET negative_count = ?4,
                        status = 'blocked',
                        last_seen = ?5
                  WHERE user_id = ?1
                    AND variant_norm = ?2
                    AND correct_form_norm = ?3
                    AND edit_type = 'replace'",
                params![
                    user_id,
                    variant_norm,
                    replacement_norm,
                    tier2_edit_policy::BLOCK_NEGATIVES,
                    crate::store::now_ms(),
                ],
            );
        }
    }

    // ── Delete matching STT replacement aliases ──────────────────────────────
    // Remove aliases where from_text matches variant and to_text matches the
    // wrong replacement, so the STT layer stops rewriting this pair.
    if let Ok(conn) = state.pool.get() {
        let deleted = conn
            .execute(
                "DELETE FROM stt_replacements
                  WHERE user_id = ?1
                    AND lower(transcript_form) = lower(?2)
                    AND lower(correct_form) = lower(?3)",
                params![user_id, body.variant.trim(), body.wrong_replacement.trim()],
            )
            .unwrap_or(0);
        if deleted > 0 {
            info!(
                "[block] deleted {deleted} STT alias(es) for {:?} -> {:?}",
                body.variant, body.wrong_replacement,
            );
        }
    }

    // ── Invalidate lexicon cache ─────────────────────────────────────────────
    crate::invalidate_lexicon_cache(&state.lexicon_cache).await;

    info!(
        "[block] user blocked correction {:?} -> {:?}",
        body.variant, body.wrong_replacement,
    );

    post_runtime_client_event(
        state.clone(),
        "correction_blocked",
        None,
        "NEGATIVE_CORRECTION",
        hash_text(&body.variant),
        Some(hash_text(&body.wrong_replacement)),
        serde_json::json!({
            "learned": false,
            "source": "block_correction",
            "negative_count": 1,
            "blocked": true,
        }),
    );

    (StatusCode::OK, Json(BlockResponse { blocked: true }))
}

// ── POST /v1/confirm-batch ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConfirmBatchBody {
    pub items: Vec<ConfirmBatchItem>,
    pub recording_id: Option<String>,
    #[serde(default)]
    pub review_session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ConfirmBatchItem {
    pub original: String,
    pub corrected: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Serialize)]
pub struct ConfirmBatchResponse {
    pub learned_count: usize,
    pub blocked_count: usize,
    pub learned_terms: Vec<String>,
    pub server_owned: bool,
}

pub async fn confirm_batch(
    State(state): State<AppState>,
    Json(body): Json<ConfirmBatchBody>,
) -> (StatusCode, Json<ConfirmBatchResponse>) {
    let user_id = state.default_user_id.as_str();
    let prefs = get_prefs(&state.pool, user_id);
    let learning_enabled = prefs.as_ref().map(|p| p.learning_enabled).unwrap_or(true);
    if !learning_enabled {
        info!("[confirm-batch] local learn blocked — user learning disabled");
        return (
            StatusCode::OK,
            Json(ConfirmBatchResponse {
                blocked_count: body.items.len(),
                learned_count: 0,
                learned_terms: vec![],
                server_owned: false,
            }),
        );
    }
    if crate::legacy_learning::audit_only_legacy_mutations() {
        info!("[confirm-batch] local learn blocked — learning disabled");
        return (
            StatusCode::OK,
            Json(ConfirmBatchResponse {
                blocked_count: body.items.len(),
                learned_count: 0,
                learned_terms: vec![],
                server_owned: false,
            }),
        );
    }
    let language = prefs
        .as_ref()
        .map(|p| p.output_language.clone())
        .unwrap_or_else(|| "hinglish".into());
    let groq_key = prefs
        .as_ref()
        .and_then(|p| p.groq_api_key.clone())
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
        .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
        .unwrap_or_default();

    let mut learned_count = 0_usize;
    let mut learned_terms = Vec::new();
    let mut server_memory_terms: Vec<serde_json::Value> = Vec::new();
    let mut server_memory_aliases: Vec<serde_json::Value> = Vec::new();
    let mut observability_aliases: Vec<crate::observability::AliasLearnItem> = Vec::new();

    for item in &body.items {
        let corrected = item.corrected.trim();
        let original = item.original.trim();
        if corrected.is_empty() {
            continue;
        }
        if matches!(
            item.tag.as_deref(),
            Some("polish_error" | "format_preference")
        ) {
            if original.is_empty()
                || original == corrected
                || unsafe_confirmed_correction_source(original, corrected)
            {
                info!(
                    "[confirm-batch] blocked unsafe writing correction {:?} -> {:?}",
                    original, corrected
                );
                continue;
            }
            corrections::upsert(
                &state.pool,
                user_id,
                &[(original.to_ascii_lowercase(), corrected.to_string())],
            );
            learned_count += 1;
            if !learned_terms.iter().any(|term| term == corrected) {
                learned_terms.push(corrected.to_string());
            }
            info!(
                "[confirm-batch] learned reviewed {} correction {:?} -> {:?}",
                item.tag.as_deref().unwrap_or("writing"),
                original,
                corrected,
            );
            continue;
        }

        if !original.is_empty()
            && tier2_edit_policy::normalize_token(original)
                == tier2_edit_policy::normalize_token(corrected)
        {
            info!(
                "[confirm-batch] skipped no-op candidate {:?} -> {:?}",
                original, corrected
            );
            continue;
        }

        let term_type = vocabulary::classify_term_type(corrected).to_string();
        let alias_safe = if original.is_empty() {
            false
        } else if deterministic_confirm_alias_allowed(
            &state.pool,
            user_id,
            original,
            corrected,
            &term_type,
        ) {
            info!(
                "[confirm-batch] alias safety accepted by deterministic HITL gate {:?} -> {:?}",
                original, corrected
            );
            true
        } else {
            let safety = alias_safety::judge_alias_source(
                &state.http_client,
                &state.pool,
                user_id,
                &groq_key,
                original,
                corrected,
                None,
            )
            .await;
            info!(
                "[confirm-batch] alias safety judge verdict={} provider={} model={} conf={:.2} {:?} -> {:?}: {}",
                safety.verdict.as_str(),
                safety.provider,
                safety.model,
                safety.confidence,
                original,
                corrected,
                safety.reason
            );
            safety.allows_learning()
        };

        if !original.is_empty() && !alias_safe {
            info!(
                "[confirm-batch] alias safety blocked {:?} -> {:?} — not storing vocab-only surrogate",
                original, corrected
            );
            continue;
        }

        // Promote or refresh vocabulary. Alias learning must still run when
        // the term already exists; review cards are commonly used to teach a
        // new distortion of a known term.
        let example_context = clean_vocab_context(item.context.as_deref());
        let inserted_or_updated = vocabulary::upsert_for_language_with_context(
            &state.pool,
            user_id,
            corrected,
            1.0,
            "confirmed",
            &language,
            example_context.as_deref(),
        );

        vocab_fts::upsert(&state.pool, user_id, corrected, example_context.as_deref());

        if alias_safe {
            // Record edit-policy rule
            tier2_edit_policy::record_explicit_edit(
                &state.pool,
                user_id,
                original,
                corrected,
                "replace",
                &[],
                &[],
                body.recording_id.as_deref(),
            );

            // STT alias
            stt_replacements::upsert_aliases_for_language(
                &state.pool,
                user_id,
                original,
                original,
                corrected,
                1.0,
                &language,
            );
            stt_replacements::mark_confirmed_aliases_for_language(
                &state.pool,
                user_id,
                original,
                original,
                corrected,
                &language,
            );

            // Proactive distortions
            stt_replacements::generate_proactive_distortions(
                &state.pool,
                user_id,
                corrected,
                original,
                &language,
            );

            // Auto-approve (user explicitly confirmed via batch)
            stt_replacements::approve_aliases_for_term(&state.pool, user_id, corrected);

            observability_aliases.push(crate::observability::AliasLearnItem {
                heard: original.to_string(),
                correct: corrected.to_string(),
                source: "confirm_batch".into(),
                safety: None,
                recording_id: body.recording_id.clone(),
            });
        }

        server_memory_terms.push(serde_json::json!({
            "term": corrected,
            "term_type": term_type,
            "weight": 1.0,
            "source": "local_confirm_batch",
        }));
        if alias_safe {
            server_memory_aliases.push(serde_json::json!({
                "transcript_form": original,
                "correct_form": corrected,
                "edit_type": "replace",
                "term_type": term_type,
                "source": "local_confirm_batch",
            }));
        }

        if inserted_or_updated || alias_safe {
            learned_count += 1;
            if !learned_terms.iter().any(|term| term == corrected) {
                learned_terms.push(corrected.to_string());
            }
            schedule_vocab_artifacts(
                state.clone(),
                user_id.to_string(),
                corrected.to_string(),
                example_context,
            );
        }

        info!(
            "[confirm-batch] learned {:?} from {:?}",
            corrected, original
        );
    }

    if learned_count > 0 {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
        refresh_local_profile_summary(&state, "confirm_batch");
        crate::routes::classify::schedule_retrain_public(state.clone());
        post_runtime_client_event(
            state.clone(),
            "classify_edit_result",
            body.recording_id.clone(),
            "STT_ERROR",
            hash_text(
                &body
                    .items
                    .iter()
                    .map(|item| item.original.as_str())
                    .collect::<Vec<_>>()
                    .join("|"),
            ),
            Some(hash_text(&learned_terms.join("|"))),
            serde_json::json!({
                "learned": true,
                "notify": true,
                "source": "confirm_batch",
                "promoted_count": learned_count,
                "promoted_term_count": learned_terms.len(),
                "capture_method": "user_confirmed_modal",
                "memory": {
                    "accepted_terms": server_memory_terms,
                    "accepted_aliases": server_memory_aliases,
                },
            }),
        );
        post_runtime_memory_dirty(state.clone());
    }

    info!(
        "[confirm-batch] learned {learned_count}/{} terms (local fallback)",
        body.items.len(),
    );

    if !observability_aliases.is_empty() {
        let pool = state.pool.clone();
        let user_id_owned = user_id.to_string();
        let http = state.http_client.clone();
        let batch = crate::observability::AliasBatchPayload {
            items: observability_aliases,
        };
        tokio::spawn(async move {
            if let Err(e) = crate::observability::enqueue_alias_batch(&pool, &user_id_owned, batch)
            {
                tracing::warn!("[observability] confirm-batch alias enqueue failed: {e}");
            }
            crate::observability::uploader::maybe_upload_after_enqueue(
                &pool,
                &user_id_owned,
                &http,
            );
        });
    }

    if let Some(session_id) = body.review_session_id.as_deref() {
        if !edit_review_sessions::resolve(&state.pool, user_id, session_id, 1) {
            warn!("[confirm-batch] review session {session_id} was not pending");
        }
    }

    (
        StatusCode::OK,
        Json(ConfirmBatchResponse {
            blocked_count: body.items.len().saturating_sub(learned_count),
            learned_count,
            learned_terms,
            server_owned: false,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::unsafe_confirmed_correction_source;

    #[test]
    fn reviewed_writing_correction_gate_keeps_rules_small_and_non_aliasing() {
        assert!(!unsafe_confirmed_correction_source("colour", "color"));
        assert!(!unsafe_confirmed_correction_source("8am", "8:00 AM"));
        assert!(unsafe_confirmed_correction_source("please", "AirNote"));
        assert!(unsafe_confirmed_correction_source(
            "a very long source phrase here",
            "short"
        ));
    }
}
