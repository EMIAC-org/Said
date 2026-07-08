use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;

pub async fn handler(State(state): State<AppState>) -> Json<Value> {
    let _prefs = crate::get_prefs_cached(
        &state.prefs_cache,
        &state.pool,
        state.default_user_id.as_str(),
    )
    .await;
    let speech_model_ready = said_core::paths::active_dictation_model_path().is_file();
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "speech_model": said_core::stt::telemetry_speech_model(),
        "speech_model_ready": speech_model_ready,
    }))
}

/// Ultra-lightweight ping for the frontend heartbeat.
/// Returns watchdog health level so the UI can react before a full freeze.
pub async fn ping(State(state): State<AppState>) -> Json<Value> {
    let wd = &state.watchdog;
    let level = wd.health_level();
    let level_name = match level {
        0 => "green",
        1 => "yellow",
        2 => "orange",
        _ => "red",
    };
    Json(json!({
        "ok": level < 3,
        "level": level_name,
        "strikes": wd.strikes(),
        "shedding": wd.is_shedding(),
    }))
}
