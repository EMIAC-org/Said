use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::{AppState, routes::key_guard};

pub async fn handler(State(state): State<AppState>) -> Json<Value> {
    let prefs = crate::get_prefs_cached(
        &state.prefs_cache,
        &state.pool,
        state.default_user_id.as_str(),
    )
    .await;
    let (preferred, effective, stt_ready) = prefs.as_ref().map_or(
        ("deepgram".to_string(), "deepgram".to_string(), false),
        |p| {
            let preferred = said_core::stt::resolve_provider_from_pref(&p.stt_provider);
            let effective = key_guard::effective_stt_provider(p);
            let ready = if said_core::stt::is_whisper_local(&effective) {
                said_core::paths::active_dictation_model_path().is_file()
            } else if said_core::stt::is_swift_local(&effective) {
                said_core::paths::swift_model_weights_path().is_file()
            } else {
                said_core::stt::resolve_deepgram_api_key(p.deepgram_api_key.as_deref()).is_some()
            };
            (preferred, effective, ready)
        },
    );
    Json(json!({
        "ok":           true,
        "version":      env!("CARGO_PKG_VERSION"),
        "stt_provider": effective,
        "stt_provider_preferred": preferred,
        "stt_ready":    stt_ready,
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
        "ok":      level < 3,
        "level":   level_name,
        "strikes": wd.strikes(),
        "shedding": wd.is_shedding(),
    }))
}
