//! POST /v1/confirm-term     — user confirms an ambiguous term for vocabulary
//! POST /v1/block-correction — user blocks a wrong correction rule

use axum::{Json, extract::State, http::StatusCode};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{
    AppState,
    store::{prefs::get_prefs, stt_replacements, tier2_edit_policy, vocab_fts, vocabulary},
};

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

    if body.action == "learn" {
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
    pub learned_terms: Vec<String>,
}

pub async fn confirm_batch(
    State(state): State<AppState>,
    Json(body): Json<ConfirmBatchBody>,
) -> (StatusCode, Json<ConfirmBatchResponse>) {
    let user_id = state.default_user_id.as_str();
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

    let mut learned_count = 0_usize;
    let mut learned_terms = Vec::new();

    for item in &body.items {
        let corrected = item.corrected.trim();
        let original = item.original.trim();
        if corrected.is_empty() {
            continue;
        }

        // Promote to vocabulary
        if vocabulary::upsert_for_language_with_context(
            &state.pool,
            user_id,
            corrected,
            1.0,
            "confirmed",
            &language,
            None,
        ) {
            learned_count += 1;
            learned_terms.push(corrected.to_string());

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

            info!(
                "[confirm-batch] learned {:?} from {:?}",
                corrected, original
            );
        }
    }

    if learned_count > 0 {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
        crate::routes::classify::schedule_retrain_public(state.clone());
    }

    info!(
        "[confirm-batch] learned {learned_count}/{} terms",
        body.items.len(),
    );

    (
        StatusCode::OK,
        Json(ConfirmBatchResponse {
            learned_count,
            learned_terms,
        }),
    )
}
