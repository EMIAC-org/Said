use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;

pub async fn scan(State(state): State<AppState>) -> Json<Value> {
    match crate::observability::meeting_scanner::scan_and_enqueue(
        &state.pool,
        &state.default_user_id,
    ) {
        Ok(enqueued) => {
            crate::observability::uploader::maybe_upload_after_enqueue(
                &state.pool,
                &state.default_user_id,
                &state.http_client,
            );
            Json(json!({ "ok": true, "enqueued": enqueued }))
        }
        Err(error) => Json(json!({ "ok": false, "error": error })),
    }
}
