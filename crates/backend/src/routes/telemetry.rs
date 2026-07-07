//! Local telemetry outbox routes — fast, non-blocking patches from the desktop shell.

use crate::{AppState, telemetry::uploader};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct DictationTracePatch {
    pub dictation_trace_json: Value,
}

pub async fn patch_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(patch): Json<crate::store::telemetry::RunSummaryPatch>,
) -> StatusCode {
    let user_id = state.default_user_id.clone();
    let pool = state.pool.clone();
    let finalize = patch.finalize;
    let http = state.http_client.clone();

    tokio::spawn(async move {
        if let Err(e) = crate::store::telemetry::patch_run(&pool, &user_id, &run_id, &patch) {
            tracing::warn!("[telemetry] patch_run {run_id}: {e}");
            return;
        }
        if finalize {
            uploader::maybe_upload_after_run(&pool, &user_id, &http);
        }
    });

    StatusCode::NO_CONTENT
}

pub async fn flush(State(state): State<AppState>) -> StatusCode {
    let pool = state.pool.clone();
    let user_id = state.default_user_id.as_str().to_string();
    let http = state.http_client.clone();
    let version = env!("CARGO_PKG_VERSION").to_string();
    let device_id = said_core::paths::device_id();

    tokio::spawn(async move {
        uploader::upload_pending(&pool, &user_id, &http, &version, &device_id).await;
    });

    StatusCode::ACCEPTED
}

pub async fn patch_dictation_trace(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(patch): Json<DictationTracePatch>,
) -> StatusCode {
    let pool = state.pool.clone();
    let user_id = state.default_user_id.clone();
    let http = state.http_client.clone();
    let trace = patch.dictation_trace_json;

    tokio::spawn(async move {
        if let Err(e) = crate::store::history::merge_recording_trace(&pool, &recording_id, &trace) {
            tracing::warn!(
                "[observability] local trace merge failed recording_id={recording_id}: {e}"
            );
        }
        if crate::observability::should_enqueue(&pool, &user_id) {
            let payload = crate::observability::DictationPatchPayload {
                recording_id: recording_id.clone(),
                final_text: None,
                edit_feedback_json: None,
                dictation_trace_json: Some(trace),
            };
            if let Err(e) = crate::observability::enqueue_dictation_patch(&pool, &user_id, payload)
            {
                tracing::warn!("[observability] trace patch enqueue failed: {e}");
            }
            crate::observability::uploader::maybe_upload_after_enqueue(&pool, &user_id, &http);
        }
    });

    StatusCode::ACCEPTED
}
