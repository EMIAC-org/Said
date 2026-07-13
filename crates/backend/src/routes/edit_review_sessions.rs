use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use crate::{AppState, store::edit_review_sessions};

pub async fn next(
    State(state): State<AppState>,
) -> Json<Option<edit_review_sessions::EditReviewSession>> {
    Json(edit_review_sessions::next_pending(
        &state.pool,
        &state.default_user_id,
    ))
}

pub async fn skip(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    if edit_review_sessions::resolve(&state.pool, &state.default_user_id, &id, 2) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}
