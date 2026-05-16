use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;

pub async fn handler() -> Json<Value> {
    Json(json!({
        "ok":      true,
        "version": env!("CARGO_PKG_VERSION"),
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
