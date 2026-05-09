//! POST /v1/edit-feedback
//!
//! Called when Tauri detects the user edited the pasted text.
//! 1. Validates ownership of the recording.
//! 2. No-ops if polished == user_kept (nothing changed).
//! 3. Updates recording.final_text.
//! 4. Inserts an edit_event row (permanent learning corpus).
//! 5. Fire-and-forget: embeds transcript → upserts preference_vector.

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::{
    AppState,
    embedder::gemini,
    store::{corrections, history, prefs::get_prefs, vectors},
    stt::background as stt_background,
};

#[derive(Deserialize)]
pub struct FeedbackBody {
    pub recording_id: String,
    pub user_kept: String,
    pub target_app: Option<String>,
}

pub async fn submit(State(state): State<AppState>, Json(body): Json<FeedbackBody>) -> StatusCode {
    let pool = state.pool.clone();

    // ── Load recording ────────────────────────────────────────────────────────
    let rec = match history::get_recording(&pool, &body.recording_id) {
        Some(r) => r,
        None => return StatusCode::NOT_FOUND,
    };
    if rec.user_id != state.default_user_id.as_str() {
        return StatusCode::FORBIDDEN;
    }

    // ── No-op if the user kept exactly the polished text ─────────────────────
    if body.user_kept.trim() == rec.polished.trim() {
        debug!("[feedback] no change for recording {}", body.recording_id);
        return StatusCode::NO_CONTENT;
    }

    // ── Update recording's final_text ─────────────────────────────────────────
    history::apply_edit_feedback(&pool, &body.recording_id, &body.user_kept);

    // ── Insert edit_event ─────────────────────────────────────────────────────
    let edit_event_id = match vectors::insert_edit_event(
        &pool,
        &rec.user_id,
        Some(&rec.id),
        &rec.transcript,
        &rec.polished,
        &body.user_kept,
        body.target_app.as_deref(),
    ) {
        Some(id) => id,
        None => {
            warn!("[feedback] failed to insert edit_event");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // ── Extract and store word-level corrections ────────────────────────────
    let diffs = corrections::extract_diffs(&rec.polished, &body.user_kept);
    if !diffs.is_empty() {
        corrections::upsert(&pool, &rec.user_id, &diffs);
        info!("[feedback] stored {} word correction(s)", diffs.len());
        // Invalidate so next request picks up the new correction immediately
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    }

    let contradicted = crate::store::stt_replacements::note_negative_signals_for_edit(
        &pool,
        &rec.user_id,
        &rec.polished,
        &body.user_kept,
    );
    if contradicted > 0 {
        info!(
            "[feedback] downgraded {} alias export signal(s)",
            contradicted
        );
        let state2 = state.clone();
        tokio::spawn(async move {
            stt_background::run_pending_alias_reviews(state2, 8).await;
        });
    }

    info!(
        "[feedback] edit_event {} created for recording {}",
        edit_event_id, rec.id
    );

    // ── Fire-and-forget: embed transcript → upsert preference_vector ──────────
    {
        let pool2 = pool.clone();
        let transcript2 = rec.transcript.clone();
        let user_id2 = rec.user_id.clone();
        let event_id2 = edit_event_id.clone();
        let http_client = state.http_client.clone();
        // Resolve Gemini key from prefs, fall back to env var
        let Some(prefs) = get_prefs(&pool, &rec.user_id) else {
            return StatusCode::NO_CONTENT;
        };
        if !prefs.learning_enabled {
            info!("[feedback] learning disabled — skipped vector embedding");
            return StatusCode::NO_CONTENT;
        }
        let gemini_key = prefs
            .gemini_api_key
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();

        tokio::spawn(async move {
            if !vectors::should_embed_event(&pool2, &event_id2) {
                info!("[feedback] skipped low-info vector for event {event_id2}");
                return;
            }
            let t_start = std::time::Instant::now();
            match gemini::embed(&http_client, &pool2, &transcript2, &gemini_key).await {
                None => warn!("[feedback] embedding skipped for event {event_id2}"),
                Some(embedding) => {
                    let embed_ms = t_start.elapsed().as_millis();
                    vectors::upsert_vector(&pool2, &user_id2, &event_id2, &embedding);
                    info!("[feedback] vector stored for event {event_id2} ({embed_ms}ms)");
                }
            }
        });
    }

    StatusCode::NO_CONTENT
}
