//! Cloud auth bridging routes:
//!
//!   PUT /v1/cloud/token   — store a cloud bearer token + license tier locally
//!   DELETE /v1/cloud/token — clear token (logout)
//!   GET /v1/cloud/status  — return current tier + whether token is stored

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, store::users};

// ── PUT /v1/cloud/token ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StoreTokenBody {
    pub token: String,
    pub license_tier: String,
    pub email: Option<String>,
}

pub async fn store_token(
    State(state): State<AppState>,
    Json(body): Json<StoreTokenBody>,
) -> StatusCode {
    users::update_cloud_auth(
        &state.pool,
        &state.default_user_id,
        &body.token,
        &body.license_tier,
        body.email.as_deref(),
    );
    StatusCode::NO_CONTENT
}

// ── DELETE /v1/cloud/token ────────────────────────────────────────────────────

pub async fn clear_token(State(state): State<AppState>) -> StatusCode {
    users::clear_cloud_token(&state.pool, &state.default_user_id);
    StatusCode::NO_CONTENT
}

// ── GET /v1/cloud/status ──────────────────────────────────────────────────────

pub async fn status(State(state): State<AppState>) -> Json<Value> {
    let user = users::get_user(&state.pool, &state.default_user_id);
    let tier = user
        .as_ref()
        .map(|u| u.license_tier.as_str())
        .unwrap_or("free");
    let has_token = user.as_ref().and_then(|u| u.cloud_token.as_ref()).is_some();

    Json(json!({
        "connected":     has_token,
        "license_tier":  tier,
        "email":         user.and_then(|u| if has_token { Some(u.email) } else { None }),
    }))
}
