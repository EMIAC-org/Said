//! Local telemetry outbox routes — fast, non-blocking patches from the desktop shell.

use crate::{AppState, telemetry::uploader};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

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
