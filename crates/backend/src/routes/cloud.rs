//! Cloud auth bridging routes:
//!
//!   PUT /v1/cloud/token   — store a cloud bearer token + license tier locally
//!   DELETE /v1/cloud/token — clear token (logout)
//!   GET /v1/cloud/status  — return current tier + whether token is stored
//!   GET /v1/enterprise/status — enterprise connection details

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
    pub server_url: Option<String>,
    pub org_name: Option<String>,
}

pub async fn store_token(
    State(state): State<AppState>,
    Json(body): Json<StoreTokenBody>,
) -> StatusCode {
    if body.server_url.is_some() || body.org_name.is_some() {
        users::update_enterprise_auth(
            &state.pool,
            &state.default_user_id,
            &body.token,
            &body.license_tier,
            body.email.as_deref(),
            body.server_url.as_deref(),
            body.org_name.as_deref(),
        );
    } else {
        users::update_cloud_auth(
            &state.pool,
            &state.default_user_id,
            &body.token,
            &body.license_tier,
            body.email.as_deref(),
        );
    }
    StatusCode::NO_CONTENT
}

// ── DELETE /v1/cloud/token ────────────────────────────────────────────────────

pub async fn clear_token(State(state): State<AppState>) -> StatusCode {
    users::clear_cloud_token(&state.pool, &state.default_user_id);
    crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
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
        "email":         user.as_ref().and_then(|u| if has_token { Some(u.email.clone()) } else { None }),
    }))
}

// ── GET /v1/enterprise/status ─────────────────────────────────────────────────

pub async fn enterprise_status(State(state): State<AppState>) -> Json<Value> {
    let user = users::get_user(&state.pool, &state.default_user_id);
    let connected = users::has_enterprise_auth(&state.pool, &state.default_user_id);

    Json(json!({
        "connected":   connected,
        "email":       user.as_ref().map(|u| u.email.clone()),
        "server_url":  user.as_ref().and_then(|u| u.enterprise_server_url.clone()),
        "org_name":    user.as_ref().and_then(|u| u.enterprise_org_name.clone()),
        "license_tier": user.as_ref().map(|u| u.license_tier.clone()).unwrap_or_else(|| "free".into()),
    }))
}
