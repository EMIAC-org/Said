//! POST /v1/pre-embed
//!
//! Speculative embedding — fired by Tauri the moment the user stops speaking
//! (CloseStream sent to Deepgram), while the 500ms drain window is still open.
//!
//! Returns 202 immediately; embedding runs fire-and-forget in the background.
//! When the full /v1/voice/polish request arrives ~500ms later, the embedding
//! is already in the SQLite cache → 0ms embed wait instead of 250–300ms.

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use tracing::{debug, info};

use crate::{AppState, embedder::gemini};

#[derive(Deserialize)]
pub struct PreEmbedBody {
    pub text: String,
}

pub async fn handler(State(state): State<AppState>, Json(body): Json<PreEmbedBody>) -> StatusCode {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    let learning_enabled = crate::get_prefs_cached(
        &state.prefs_cache,
        &state.pool,
        state.default_user_id.as_str(),
    )
    .await
    .map(|p| p.learning_enabled)
    .unwrap_or(true);
    if !learning_enabled || crate::legacy_learning::audit_only_legacy_mutations() {
        debug!("[pre-embed] skipped — user learning disabled");
        return StatusCode::ACCEPTED;
    }

    debug!(
        "[pre-embed] received {} chars — spawning background embed",
        text.len()
    );

    let pool = state.pool.clone();
    let http_client = state.http_client.clone();
    let user_id = state.default_user_id.as_str().to_string();
    let prefs_cache = state.prefs_cache.clone();

    // Fire-and-forget — caller gets 202 immediately, embedding stores in SQLite cache.
    tokio::spawn(async move {
        // Use the in-memory prefs cache (30 s TTL) — zero SQLite hits on warm path.
        let Some(prefs) = crate::get_prefs_cached(&prefs_cache, &pool, &user_id).await else {
            return;
        };
        if !prefs.learning_enabled {
            debug!("[pre-embed] skipped — learning disabled");
            return;
        }
        let gemini_key = prefs
            .gemini_api_key
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();

        if gemini_key.is_empty() {
            debug!("[pre-embed] skipped — no Gemini key");
            return;
        }

        let start = std::time::Instant::now();
        match gemini::embed(&http_client, &pool, &text, &gemini_key).await {
            Some(_) => info!(
                "[pre-embed] cached in {}ms ({} chars)",
                start.elapsed().as_millis(),
                text.len()
            ),
            None => debug!("[pre-embed] embed returned None"),
        }
    });

    StatusCode::ACCEPTED
}
