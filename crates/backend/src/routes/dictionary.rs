//! `/v1/dictionary` — the user's word list, for the Dictionary page.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState,
    store::dictionary::{self, SOURCE_ADDED},
};

pub async fn list(State(state): State<AppState>) -> Response {
    let entries = dictionary::list(&state.pool, &state.default_user_id);
    Json(json!({ "entries": entries })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AddBody {
    pub written: String,
    #[serde(default)]
    pub heard: Option<String>,
}

pub async fn add(State(state): State<AppState>, Json(body): Json<AddBody>) -> Response {
    if body.written.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match dictionary::add(
        &state.pool,
        &state.default_user_id,
        &body.written,
        body.heard.as_deref(),
        SOURCE_ADDED,
    ) {
        Some(entry) => (StatusCode::CREATED, Json(entry)).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> StatusCode {
    if dictionary::delete(&state.pool, &state.default_user_id, id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

pub async fn delete_all(State(state): State<AppState>) -> StatusCode {
    dictionary::delete_all(&state.pool, &state.default_user_id);
    StatusCode::NO_CONTENT
}
