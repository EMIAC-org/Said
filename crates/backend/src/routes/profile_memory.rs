//! Local bridge for server-owned profile memory.
//!
//! The React UI talks only to the local daemon. This route forwards requests to
//! the signed-in control plane with the stored cloud token and active-org header.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde_json::{Value, json};

use crate::{
    AppState, cp_client,
    store::users::{self, LocalUser},
};

fn control_plane_path(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn enterprise_context(
    state: &AppState,
) -> Result<(LocalUser, String, String), (StatusCode, Json<Value>)> {
    let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
        tracing::warn!("[profile-memory] enterprise context missing local user");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "local user not found"})),
        ));
    };
    let Some(token) = user.cloud_token.clone().filter(|t| !t.trim().is_empty()) else {
        tracing::warn!(
            "[profile-memory] enterprise context missing token email={} server_url_present={}",
            user.email,
            user.enterprise_server_url
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
        );
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "enterprise token missing"})),
        ));
    };
    let Some(base_url) = user
        .enterprise_server_url
        .clone()
        .filter(|s| !s.trim().is_empty())
    else {
        tracing::warn!(
            "[profile-memory] enterprise context missing server url email={} token_present=true",
            user.email,
        );
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "enterprise server URL missing"})),
        ));
    };
    Ok((user, token, base_url))
}

pub async fn memory(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (user, token, base_url) = enterprise_context(&state)?;
    let url = control_plane_path(&base_url, "/v1/runtime/profile/memory");
    tracing::info!(
        "[profile-memory] proxy memory start server={} email={} active_org={}",
        base_url,
        user.email,
        user.active_org_id.as_deref().unwrap_or("personal"),
    );
    let resp =
        cp_client::with_org_context(state.http_client.get(url).bearer_auth(&token), Some(&user))
            .send()
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("profile memory request failed: {e}")})),
                )
            })?;
    proxy_json_response(resp, "memory", None).await
}

pub async fn approve(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    decision(state, &job_id, "approve").await
}

pub async fn dismiss(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    decision(state, &job_id, "dismiss").await
}

async fn decision(
    state: AppState,
    job_id: &str,
    action: &str,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (user, token, base_url) = enterprise_context(&state)?;
    let url = control_plane_path(
        &base_url,
        &format!("/v1/runtime/profile/proposals/{job_id}/{action}"),
    );
    tracing::info!(
        "[profile-memory] proxy proposal decision start action={} job={} server={} email={} active_org={}",
        action,
        job_id,
        base_url,
        user.email,
        user.active_org_id.as_deref().unwrap_or("personal"),
    );
    let resp =
        cp_client::with_org_context(state.http_client.post(url).bearer_auth(&token), Some(&user))
            .send()
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("profile proposal {action} failed: {e}")})),
                )
            })?;
    proxy_json_response(resp, action, Some(job_id)).await
}

async fn proxy_json_response(
    resp: reqwest::Response,
    action: &str,
    job_id: Option<&str>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({"raw": body}));
    tracing::info!(
        "[profile-memory] proxy response action={} job={} status={} body_chars={} pending={} profile_version={}",
        action,
        job_id.unwrap_or("none"),
        status,
        body.chars().count(),
        value
            .get("pending_proposals")
            .and_then(|v| v.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        value
            .get("profile")
            .and_then(|v| v.get("version"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    );
    if status.is_success() {
        Ok(Json(value))
    } else {
        Err((
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(value),
        ))
    }
}
