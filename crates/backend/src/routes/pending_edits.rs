//! Pending-edit review endpoints.
//!
//! POST /v1/pending-edits             — store a detected edit for later approval
//! GET  /v1/pending-edits             — list unresolved pending edits + count
//! GET  /v1/pending-edits/unnotified  — list edits not yet shown to the user
//! POST /v1/pending-edits/:id/resolve — approve (→ learning corpus) or skip
//! POST /v1/pending-edits/:id/dismiss — skip + mark as notified (notification dismissed)

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use reqwest::Client;

use crate::{
    AppState,
    embedder::gemini,
    store::{corrections, history, pending_edits, prefs::get_prefs, vectors},
    stt::background as stt_background,
};

// ── Create ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateBody {
    pub recording_id: Option<String>,
    pub ai_output: String,
    pub user_kept: String,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> (StatusCode, Json<serde_json::Value>) {
    match pending_edits::insert(
        &state.pool,
        &state.default_user_id,
        body.recording_id.as_deref(),
        &body.ai_output,
        &body.user_kept,
    ) {
        Some(id) => {
            info!("[pending-edits] stored {id}");
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id })))
        }
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "insert failed" })),
        ),
    }
}

// ── List ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ListResponse {
    pub edits: Vec<pending_edits::PendingEdit>,
    pub total: i64,
}

pub async fn list(State(state): State<AppState>) -> Json<ListResponse> {
    let edits = pending_edits::list_pending(&state.pool, &state.default_user_id);
    let total = pending_edits::count_pending(&state.pool, &state.default_user_id);
    Json(ListResponse { edits, total })
}

// ── Resolve ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResolveBody {
    pub action: String, // "approve" | "skip"
}

pub async fn resolve(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ResolveBody>,
) -> StatusCode {
    let action_code: i32 = if body.action == "approve" { 1 } else { 2 };

    // When approved, write to the learning corpus the same way the live
    // feedback route does (insert edit_event → fire-and-forget embed).
    if body.action == "approve" {
        if let Some(pe) = pending_edits::get(&state.pool, &id) {
            if let Some(rec_id) = &pe.recording_id {
                if let Some(rec) = history::get_recording(&state.pool, rec_id) {
                    let event_id = vectors::insert_edit_event(
                        &state.pool,
                        &state.default_user_id,
                        Some(rec_id),
                        &rec.transcript,
                        &pe.ai_output,
                        &pe.user_kept,
                        None,
                    );
                    history::apply_edit_feedback(&state.pool, rec_id, &pe.user_kept);

                    // Extract and store word-level corrections
                    let diffs = corrections::extract_diffs(&pe.ai_output, &pe.user_kept);
                    if !diffs.is_empty() {
                        corrections::upsert(&state.pool, &state.default_user_id, &diffs);
                        info!("[pending-edits] stored {} word correction(s)", diffs.len());
                    }

                    let contradicted =
                        crate::store::stt_replacements::note_negative_signals_for_edit(
                            &state.pool,
                            &state.default_user_id,
                            &pe.ai_output,
                            &pe.user_kept,
                        );
                    if contradicted > 0 {
                        info!(
                            "[pending-edits] downgraded {} alias export signal(s)",
                            contradicted
                        );
                        let state2 = state.clone();
                        tokio::spawn(async move {
                            stt_background::run_pending_alias_reviews(state2, 8).await;
                        });
                    }

                    info!("[pending-edits] approved {id} → edit_event {:?}", event_id);

                    // Fire-and-forget: embed transcript → upsert preference_vector
                    if let Some(ref eid) = event_id {
                        let pool2 = state.pool.clone();
                        let user_id2 = state.default_user_id.clone();
                        let event_id2 = eid.clone();
                        let transcript2 = rec.transcript.clone();
                        let Some(prefs) = get_prefs(&state.pool, &state.default_user_id) else {
                            pending_edits::resolve(&state.pool, &id, action_code);
                            return StatusCode::NO_CONTENT;
                        };
                        if !prefs.learning_enabled {
                            info!("[pending-edits] learning disabled — skipped vector embedding");
                            pending_edits::resolve(&state.pool, &id, action_code);
                            return StatusCode::NO_CONTENT;
                        }
                        let gemini_key = prefs
                            .gemini_api_key
                            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
                            .unwrap_or_default();
                        tokio::spawn(async move {
                            if !vectors::should_embed_event(&pool2, &event_id2) {
                                info!("[pending-edits] skipped low-info vector for {event_id2}");
                                return;
                            }
                            let http = Client::new();
                            match gemini::embed(&http, &pool2, &transcript2, &gemini_key).await {
                                None => warn!("[pending-edits] embedding skipped for {event_id2}"),
                                Some(emb) => {
                                    vectors::upsert_vector(&pool2, &user_id2, &event_id2, &emb);
                                    info!("[pending-edits] vector stored for {event_id2}");
                                }
                            }
                        });
                    }
                } else {
                    warn!("[pending-edits] recording {rec_id} not found for approval of {id}");
                }
            }
        }
    }

    if pending_edits::resolve(&state.pool, &id, action_code) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── Dismiss ──────────────────────────────────────────────────────────────────

pub async fn dismiss(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    if pending_edits::dismiss(&state.pool, &id) {
        info!("[pending-edits] dismissed {id}");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── Unnotified (for first-time notifications only) ───────────────────────────

#[derive(Serialize)]
pub struct UnnotifiedResponse {
    pub edits: Vec<pending_edits::PendingEdit>,
}

pub async fn list_unnotified(State(state): State<AppState>) -> Json<UnnotifiedResponse> {
    let edits = pending_edits::list_unnotified(&state.pool, &state.default_user_id);
    Json(UnnotifiedResponse { edits })
}

// ── Mark notified ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MarkNotifiedBody {
    pub ids: Vec<String>,
}

pub async fn mark_notified(
    State(state): State<AppState>,
    Json(body): Json<MarkNotifiedBody>,
) -> StatusCode {
    let id_refs: Vec<&str> = body.ids.iter().map(|s| s.as_str()).collect();
    pending_edits::mark_notified(&state.pool, &id_refs);
    StatusCode::NO_CONTENT
}
