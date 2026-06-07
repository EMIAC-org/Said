use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;

pub async fn handler(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "airnote-mobile-gateway",
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "stt_configured": !state.deepgram_api_key.trim().is_empty(),
        "llm_configured": !state.llm_api_key.trim().is_empty(),
    }))
}
