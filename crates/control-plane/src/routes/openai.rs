//! OpenAI account connection routes:
//!   POST   /v1/openai/connect     — start PKCE flow (admin only)
//!   POST   /v1/openai/complete    — exchange code for tokens (admin only)
//!   GET    /v1/openai/status      — check connection status
//!   DELETE /v1/openai/disconnect  — remove connected account (admin only)

use crate::{AppState, auth::AuthUser, codex_client, tenant};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct CompleteBody {
    pub code: String,
    pub code_verifier: String,
    pub plan_type: Option<String>,
    pub label: Option<String>,
}

pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::require_active_org_role(&state, &user, &headers).await?;
    require_admin(&role)?;

    let session = codex_client::create_pkce_session();

    Ok(Json(json!({
        "auth_url":      session.auth_url,
        "code_verifier": session.code_verifier,
        "state":         session.state,
    })))
}

pub async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<CompleteBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (org_id, role) = tenant::require_active_org_role(&state, &user, &headers).await?;
    require_admin(&role)?;

    let tokens = codex_client::exchange_code(&body.code, &body.code_verifier)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("OpenAI token exchange failed: {e}")})),
            )
        })?;

    let now = Utc::now();
    let expires_at = now + chrono::Duration::seconds(tokens.expires_in);
    let plan_type = body.plan_type.as_deref();
    let label = body.label.as_deref();

    sqlx::query(
        "UPDATE orgs
            SET openai_access_token     = $1,
                openai_refresh_token    = $2,
                openai_token_expires_at = $3,
                openai_plan_type        = $4,
                openai_connected_at     = $5,
                openai_label            = $6
          WHERE id = $7",
    )
    .bind(&tokens.access_token)
    .bind(&tokens.refresh_token)
    .bind(expires_at)
    .bind(plan_type)
    .bind(now)
    .bind(label)
    .bind(org_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({
        "connected": true,
        "plan_type": plan_type,
        "label":     label,
    })))
}

pub async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let row: Option<(Option<String>, Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT openai_plan_type, openai_label, openai_connected_at
           FROM orgs
          WHERE id = $1 AND openai_access_token IS NOT NULL",
    )
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    match row {
        Some((plan_type, label, connected_at)) => Ok(Json(json!({
            "connected":    true,
            "plan_type":    plan_type,
            "label":        label,
            "connected_at": connected_at,
        }))),
        None => Ok(Json(json!({
            "connected":    false,
            "plan_type":    null,
            "label":        null,
            "connected_at": null,
        }))),
    }
}

pub async fn disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (org_id, role) = tenant::require_active_org_role(&state, &user, &headers).await?;
    require_admin(&role)?;

    sqlx::query(
        "UPDATE orgs
            SET openai_access_token     = NULL,
                openai_refresh_token    = NULL,
                openai_token_expires_at = NULL,
                openai_plan_type        = NULL,
                openai_connected_at     = NULL,
                openai_label            = NULL
          WHERE id = $1",
    )
    .bind(org_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(StatusCode::NO_CONTENT)
}

fn require_admin(role: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if role.eq_ignore_ascii_case("admin") || role.eq_ignore_ascii_case("COMPANY_ADMIN") {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "only org admins can manage OpenAI connections"})),
        ))
    }
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
