use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use tracing::info;

use crate::{
    AppState, get_prefs_cached, invalidate_prefs_cache,
    store::{
        corrections,
        prefs::{Preferences, PrefsUpdate},
    },
};

/// GET /v1/corrections — returns the "right" words from the user's correction
/// history as a flat list of strings, ready for use as Deepgram keyterms.
pub async fn get_corrections(State(state): State<AppState>) -> Json<CorrectionsResponse> {
    let user_id = state.default_user_id.clone();
    let all = corrections::load_all(&state.pool, &user_id);
    let keyterms: Vec<String> = all.into_iter().map(|c| c.right).collect();
    Json(CorrectionsResponse { keyterms })
}

#[derive(Serialize)]
pub struct CorrectionsResponse {
    pub keyterms: Vec<String>,
}

pub async fn get_prefs(State(state): State<AppState>) -> Result<Json<Preferences>, StatusCode> {
    let user_id = state.default_user_id.clone();
    // Gap 3: read through cache (SQLite only on miss / TTL expiry)
    let prefs = get_prefs_cached(&state.prefs_cache, &state.pool, &user_id)
        .await
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(prefs))
}

pub async fn patch_prefs(
    State(state): State<AppState>,
    Json(update): Json<PrefsUpdate>,
) -> Result<Json<Preferences>, StatusCode> {
    let provider_key_updated = update.gateway_api_key.is_some()
        || update.deepgram_api_key.is_some()
        || update.gemini_api_key.is_some()
        || update.groq_api_key.is_some()
        || update.cerebras_api_key.is_some();
    let cross_device_updated = update.selected_model.is_some()
        || update.output_language.is_some()
        || update.tone_preset.is_some()
        || update.custom_prompt.is_some()
        || update.auto_paste.is_some()
        || update.edit_capture.is_some()
        || update.learning_enabled.is_some()
        || update.server_runtime_enabled.is_some()
        || update.server_audio_runtime_enabled.is_some()
        || update.stt_provider.is_some();
    info!(
        "[patch_prefs] backend received: llm_provider={:?} selected_model={:?} gateway_key_set={} gemini_key_set={} groq_key_set={}",
        update.llm_provider,
        update.selected_model,
        update
            .gateway_api_key
            .as_ref()
            .map(|v| v.is_some())
            .unwrap_or(false),
        update
            .gemini_api_key
            .as_ref()
            .map(|v| v.is_some())
            .unwrap_or(false),
        update
            .groq_api_key
            .as_ref()
            .map(|v| v.is_some())
            .unwrap_or(false),
    );
    let user_id = state.default_user_id.clone();
    let prefs = crate::store::prefs::update_prefs(&state.pool, &user_id, update)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    // Gap 3: invalidate cache so next request re-reads fresh prefs
    invalidate_prefs_cache(&state.prefs_cache).await;
    info!(
        "[patch_prefs] after update: llm_provider={:?}",
        prefs.llm_provider
    );
    if provider_key_updated {
        let state2 = state.clone();
        tokio::spawn(async move {
            match crate::routes::runtime_credentials::sync_saved_provider_credentials(state2).await
            {
                Ok(summary) if summary.failed > 0 => {
                    tracing::warn!(
                        "[runtime-credentials] post-prefs vault sync partial failure synced={} failed={} results={:?}",
                        summary.synced,
                        summary.failed,
                        summary.results
                    );
                }
                Ok(summary) if summary.synced > 0 || summary.revoked > 0 => {
                    tracing::info!(
                        "[runtime-credentials] post-prefs vault sync ok synced={} revoked={}",
                        summary.synced,
                        summary.revoked
                    );
                }
                Err(err) => {
                    tracing::warn!("[runtime-credentials] post-prefs vault sync failed: {err}");
                }
                _ => {}
            }
        });
    }
    if cross_device_updated {
        let state3 = state.clone();
        let p = prefs.clone();
        tokio::spawn(async move {
            crate::routes::server_settings::push_cross_device_settings_to_server(
                state3,
                p.selected_model,
                p.output_language,
                p.tone_preset,
                p.custom_prompt,
                p.auto_paste,
                p.edit_capture,
                p.learning_enabled,
                p.server_runtime_enabled,
                p.server_audio_runtime_enabled,
            )
            .await;
        });
    }
    Ok(Json(prefs))
}
