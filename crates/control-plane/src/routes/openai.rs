//! OpenAI account connection routes:
//!   POST   /v1/openai/connect     — start PKCE flow (admin only)
//!   POST   /v1/openai/complete    — exchange code for tokens (admin only)
//!   GET    /v1/openai/status      — check connection status
//!   DELETE /v1/openai/disconnect  — remove connected account (admin only)

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, codex_client};

// ── Request types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CompleteBody {
    pub code: String,
    pub code_verifier: String,
    pub plan_type: Option<String>,
    pub label: Option<String>,
}

// ── POST /v1/openai/connect ─────────────────────────────────────────────────

pub async fn connect(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Must be org admin
    let (_org_id, role) = resolve_org_and_role(&state, user.account_id).await?;
    require_admin(&role)?;

    let session = codex_client::create_pkce_session();

    Ok(Json(json!({
        "auth_url":      session.auth_url,
        "code_verifier": session.code_verifier,
        "state":         session.state,
    })))
}

// ── POST /v1/openai/complete ────────────────────────────────────────────────

pub async fn complete(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CompleteBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (org_id, role) = resolve_org_and_role(&state, user.account_id).await?;
    require_admin(&role)?;

    // Exchange the authorization code for tokens
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

    // Determine plan_type: prefer the body override, otherwise leave NULL
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

// ── GET /v1/openai/status ───────────────────────────────────────────────────

pub async fn status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let org_id = resolve_org(&state, user.account_id).await?;

    let row: Option<(
        Option<String>,        // openai_plan_type
        Option<String>,        // openai_label
        Option<DateTime<Utc>>, // openai_connected_at
    )> = sqlx::query_as(
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

// ── DELETE /v1/openai/disconnect ────────────────────────────────────────────

pub async fn disconnect(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (org_id, role) = resolve_org_and_role(&state, user.account_id).await?;
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

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Resolve the caller's org_id AND role from org_members, or return 403.
async fn resolve_org_and_role(
    state: &AppState,
    account_id: Uuid,
) -> Result<(Uuid, String), (StatusCode, Json<Value>)> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT org_id, role FROM org_members WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    row.ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "you must belong to an org"})),
        )
    })
}

/// Resolve the caller's org_id, discarding the role.
async fn resolve_org(
    state: &AppState,
    account_id: Uuid,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let (org_id, _role) = resolve_org_and_role(state, account_id).await?;
    Ok(org_id)
}

/// Require COMPANY_ADMIN role (or the lowercase "admin" used internally).
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
