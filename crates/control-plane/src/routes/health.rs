//! GET /v1/health

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;

pub async fn handler(State(state): State<AppState>) -> Json<Value> {
    let uptime_s = state.started_at.elapsed().as_secs();
    Json(json!({
        "ok":       true,
        "version":  env!("CARGO_PKG_VERSION"),
        "uptime_s": uptime_s,
    }))
}
