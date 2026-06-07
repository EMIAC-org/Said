//! Public bootstrap + authenticated runtime config.

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::{AppState, auth::AuthUser, runtime, util::ApiResult};

/// `GET /v1/mobile/bootstrap` — public config the app can read before login.
pub async fn bootstrap(State(state): State<AppState>) -> Json<Value> {
    let streaming = !state.deepgram_api_key.trim().is_empty();
    Json(json!({
        "schema": "airnote.mobile.bootstrap.v1",
        "gateway_region": state.gateway_region,
        "min_supported_ios_version": "17.0",
        "min_supported_app_version": "0.1.0",
        "features": {
            "ios_keyboard": true,
            "ios_action_button": true,
            "streaming_voice": streaming,
            "batch_fallback": true,
            "explicit_learning": true
        },
        "limits": {
            "max_recording_seconds": crate::util::MAX_RECORDING_SECONDS,
            "max_audio_bytes": 15_728_640
        }
    }))
}

/// `GET /v1/runtime/config` — authenticated runtime config + current vocab hash.
pub async fn config(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let current_vocab_hash = runtime::vocab::current_hash(&state.db, user.account_id).await;
    let keys_ready =
        !state.deepgram_api_key.trim().is_empty() && !state.llm_api_key.trim().is_empty();
    let status = if keys_ready {
        "voice_pipeline_ready"
    } else {
        "mock_pipeline"
    };

    Ok(Json(json!({
        "schema": "airnote.runtime.config.v1",
        "runtime": {
            "mode": "server_first_mobile",
            "session_path": "/v1/runtime/sessions",
            "voice_ws_path": "/v1/runtime/voice",
            "batch_path": "/v1/runtime/voice/batch",
            "event_path": "/v1/runtime/events",
            "vocab_snapshot_path": "/v1/mobile/vocab/snapshot",
            "max_recording_seconds": crate::util::MAX_RECORDING_SECONDS,
            "streaming_enabled": true,
            "batch_fallback_enabled": true,
            "raw_audio_retention": "none",
            "raw_text_retention": "none",
            "learning_mode": "insert_first_learn_later",
            "status": status
        },
        "account": { "id": user.account_id, "email": user.email },
        "current_vocab_hash": current_vocab_hash
    })))
}
