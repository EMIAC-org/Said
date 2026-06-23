//! POST /v1/confirm-term     — user confirms an ambiguous term for vocabulary
//! POST /v1/block-correction — user blocks a wrong correction rule

use axum::{Json, extract::State, http::StatusCode};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    AppState,
    store::{prefs::get_prefs, stt_replacements, tier2_edit_policy, users, vocab_fts, vocabulary},
};

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
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

// ── POST /v1/confirm-term ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConfirmBody {
    pub term: String,
    pub original: String,
    pub action: String,
    pub recording_id: Option<String>,
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
        let server_confirm = ConfirmBatchBody {
            items: vec![ConfirmBatchItem {
                original: body.original.clone(),
                corrected: body.term.clone(),
            }],
            recording_id: body.recording_id.clone(),
        };
        if let Some(server_response) = confirm_batch_with_server(&state, &server_confirm).await {
            info!(
                "[confirm] server-owned confirm learned {}/1 term(s) for {:?}",
                server_response.learned_count, body.term,
            );
            return (
                StatusCode::OK,
                Json(ConfirmResponse {
                    confirmed: server_response.learned_count > 0,
                    term: body.term,
                }),
            );
        }

        if crate::legacy_learning::audit_only_legacy_mutations() {
            info!(
                "[confirm] local learn blocked — legacy learning frozen for {:?}",
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
        vocabulary::upsert_for_language_with_context(
            &state.pool,
            user_id,
            &body.term,
            1.0,
            "confirmed",
            &language,
            None,
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
    if crate::legacy_learning::audit_only_legacy_mutations() {
        info!("[confirm] block_correction skipped — legacy learning frozen");
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
}

#[derive(Deserialize)]
pub struct ConfirmBatchItem {
    pub original: String,
    pub corrected: String,
}

#[derive(Serialize)]
pub struct ConfirmBatchResponse {
    pub learned_count: usize,
    pub blocked_count: usize,
    pub learned_terms: Vec<String>,
    pub server_owned: bool,
}

pub(crate) async fn confirm_batch_with_server(
    state: &AppState,
    body: &ConfirmBatchBody,
) -> Option<ConfirmBatchResponse> {
    let user = users::get_user(&state.pool, &state.default_user_id)?;
    let token = user.cloud_token.filter(|t| !t.trim().is_empty())?;
    let base_url = user
        .enterprise_server_url
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
    let url = format!(
        "{}/v1/runtime/learning/confirm-batch",
        base_url.trim_end_matches('/')
    );
    let items = body
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "original": item.original.as_str(),
                "corrected": item.corrected.as_str(),
            })
        })
        .collect::<Vec<_>>();
    let req = serde_json::json!({
        "recording_id": body.recording_id.as_deref(),
        "items": items,
    });
    match state
        .http_client
        .post(url)
        .bearer_auth(token)
        .json(&req)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(value) => {
                let learned_count = value
                    .get("learned_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let learned_terms = value
                    .get("learned_terms")
                    .and_then(|v| v.as_array())
                    .map(|terms| {
                        terms
                            .iter()
                            .filter_map(|term| term.as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let blocked_count = value
                    .get("blocked_count")
                    .and_then(|v| v.as_u64())
                    .map(|count| count as usize)
                    .unwrap_or_else(|| body.items.len().saturating_sub(learned_count));
                Some(ConfirmBatchResponse {
                    blocked_count,
                    learned_count,
                    learned_terms,
                    server_owned: true,
                })
            }
            Err(e) => {
                warn!("[confirm-batch] server confirm parse failed: {e}");
                None
            }
        },
        Ok(resp) => {
            warn!("[confirm-batch] server confirm failed: {}", resp.status());
            None
        }
        Err(e) => {
            warn!("[confirm-batch] server confirm failed: {e}");
            None
        }
    }
}

pub async fn confirm_batch(
    State(state): State<AppState>,
    Json(body): Json<ConfirmBatchBody>,
) -> (StatusCode, Json<ConfirmBatchResponse>) {
    if let Some(server_response) = confirm_batch_with_server(&state, &body).await {
        info!(
            "[confirm-batch] server-owned confirm learned {}/{} terms",
            server_response.learned_count,
            body.items.len()
        );
        return (StatusCode::OK, Json(server_response));
    }

    let user_id = state.default_user_id.as_str();
    let prefs = get_prefs(&state.pool, user_id);
    let learning_enabled = prefs.as_ref().map(|p| p.learning_enabled).unwrap_or(true);
    if crate::legacy_learning::audit_only_legacy_mutations() {
        info!("[confirm-batch] local learn blocked — legacy learning frozen");
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

    for item in &body.items {
        let corrected = item.corrected.trim();
        let original = item.original.trim();
        if corrected.is_empty() {
            continue;
        }

        // Promote or refresh vocabulary. Alias learning must still run when
        // the term already exists; review cards are commonly used to teach a
        // new distortion of a known term.
        let inserted_or_updated = vocabulary::upsert_for_language_with_context(
            &state.pool,
            user_id,
            corrected,
            1.0,
            "confirmed",
            &language,
            None,
        );

        vocab_fts::upsert(&state.pool, user_id, corrected, None);

        let alias_safe = if original.is_empty() {
            false
        } else {
            crate::llm::alias_safety::judge_alias_source(
                &state.http_client,
                &state.pool,
                user_id,
                &groq_key,
                original,
                corrected,
                None,
            )
            .await
            .allows_learning()
        };

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
        } else if !original.is_empty() {
            info!(
                "[confirm-batch] alias safety blocked {:?} -> {:?}",
                original, corrected
            );
        }

        let term_type = vocabulary::classify_term_type(corrected).to_string();
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
        }

        info!(
            "[confirm-batch] learned {:?} from {:?}",
            corrected, original
        );
    }

    if learned_count > 0 {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
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
