use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

use crate::{AppState, store::voice_runs};

pub async fn latest_failed(State(state): State<AppState>) -> Json<Value> {
    let user_id = state.default_user_id.clone();
    let run = voice_runs::latest_retryable_failed_voice_run(&state.pool, &user_id);
    Json(json!({ "run": run }))
}

#[derive(Debug, Deserialize)]
pub struct MarkFailedBody {
    error_code: Option<String>,
    message: Option<String>,
    retryable: Option<bool>,
    owned_by_airnote: Option<bool>,
    diagnostic: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct MarkPasteBody {
    paste_success: bool,
}

pub async fn mark_failed(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(body): Json<MarkFailedBody>,
) -> StatusCode {
    let error_code = body
        .error_code
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("voice_pipeline_failed");
    let message = body
        .message
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Voice pipeline failed");
    let retryable = body.retryable.unwrap_or(true);
    let owned_by_airnote = body.owned_by_airnote.unwrap_or(true);
    let marked = voice_runs::mark_voice_run_failed(
        &state.pool,
        &run_id,
        error_code,
        message,
        retryable,
        owned_by_airnote,
        body.diagnostic.as_ref(),
    )
    .is_some();
    if marked {
        StatusCode::NO_CONTENT
    } else {
        warn!("[voice-runs] mark_failed ignored missing run_id={run_id}");
        StatusCode::NOT_FOUND
    }
}

pub async fn mark_paste(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(body): Json<MarkPasteBody>,
) -> StatusCode {
    if voice_runs::mark_voice_run_paste_success_by_run(&state.pool, &run_id, body.paste_success)
        .is_some()
    {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}
