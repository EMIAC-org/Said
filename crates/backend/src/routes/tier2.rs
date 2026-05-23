use axum::{Json, extract::State};

use crate::{AppState, tier2};

pub async fn status(State(state): State<AppState>) -> Json<tier2::Tier2Status> {
    let user_id = state.default_user_id.as_str();
    Json(tier2::status(&state.pool, user_id))
}
