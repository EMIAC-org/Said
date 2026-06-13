//! Org routes:
//!   GET  /v1/orgs                  — list all org memberships
//!   GET  /v1/orgs/me               — get active org (legacy)
//!   POST /v1/orgs                  — create an org (caller becomes admin)
//!   POST /v1/orgs/:org_id/activate — set active workspace
//!   GET  /v1/orgs/:org_id/members  — list org members

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

// ── Request / response types ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateOrgBody {
    pub name: String,
    /// URL-safe slug (lowercase, hyphens). Must be unique.
    pub slug: String,
    /// Roles permitted to create meetings. Defaults to ["COMPANY_ADMIN", "MANAGER"].
    pub meeting_creator_roles: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct OrgInfo {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct MemberInfo {
    pub id: Uuid,
    pub account_id: Uuid,
    pub email: String,
    pub role: String,
    pub lark_name: Option<String>,
    pub lark_avatar_url: Option<String>,
    pub lark_department: Option<String>,
    pub auth_source: String,
    pub lark_connected: bool,
    pub joined_at: DateTime<Utc>,
}

// ── GET /v1/orgs ────────────────────────────────────────────────────────────

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let orgs = tenant::list_memberships(&state, user.account_id).await?;
    let active_org_id: Option<Uuid> =
        sqlx::query_scalar("SELECT active_org_id FROM accounts WHERE id = $1")
            .bind(user.account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?
            .flatten();

    Ok(Json(json!({
        "orgs": orgs,
        "active_org_id": active_org_id,
        "personal_mode": active_org_id.is_none(),
    })))
}

// ── GET /v1/orgs/me ─────────────────────────────────────────────────────────

pub async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant = tenant::resolve_tenant(&state, &user, &headers).await?;
    let active_org_id: Option<Uuid> =
        sqlx::query_scalar("SELECT active_org_id FROM accounts WHERE id = $1")
            .bind(user.account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?
            .flatten();

    let lookup_org = if let Some(org_id) = tenant.active_org_id.or(active_org_id) {
        Some(org_id)
    } else {
        sqlx::query_scalar(
            "SELECT org_id FROM org_members WHERE account_id = $1 ORDER BY joined_at ASC LIMIT 1",
        )
        .bind(user.account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    };

    let Some(org_id) = lookup_org else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "you are not a member of any org"})),
        ));
    };

    let row: Option<(
        Uuid,
        String,
        String,
        String,
        DateTime<Utc>,
        Value,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT o.id, o.name, o.slug, om.role, o.created_at, o.meeting_creator_roles,
                om.lark_name, om.lark_avatar_url
           FROM org_members om
           JOIN orgs o ON o.id = om.org_id
          WHERE om.account_id = $1 AND om.org_id = $2",
    )
    .bind(user.account_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some((id, name, slug, role, created_at, meeting_creator_roles, lark_name, lark_avatar_url)) =
        row
    else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "you are not a member of any org"})),
        ));
    };

    Ok(Json(json!({
        "org": {
            "id":                    id,
            "name":                  name,
            "slug":                  slug,
            "role":                  role,
            "created_at":            created_at,
            "meeting_creator_roles": meeting_creator_roles,
            "lark_name":             lark_name,
            "lark_avatar_url":       lark_avatar_url,
            "is_active":             active_org_id == Some(id),
        },
        "active_org_id": active_org_id,
    })))
}

// ── POST /v1/orgs ───────────────────────────────────────────────────────────

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateOrgBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let name = body.name.trim().to_string();
    let slug = body.slug.trim().to_lowercase();

    if name.is_empty() || slug.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "name and slug are required"})),
        ));
    }

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orgs WHERE slug = $1)")
        .bind(&slug)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;

    if exists {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "slug already taken"})),
        ));
    }

    let meeting_creator_roles: Value = body
        .meeting_creator_roles
        .map(|roles| json!(roles))
        .unwrap_or_else(|| json!(["COMPANY_ADMIN", "MANAGER"]));

    let org_id: Uuid = sqlx::query_scalar(
        "INSERT INTO orgs (name, slug, meeting_creator_roles) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&name)
    .bind(&slug)
    .bind(&meeting_creator_roles)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    sqlx::query(
        "INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'COMPANY_ADMIN')",
    )
    .bind(org_id)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    sqlx::query(
        "INSERT INTO org_subscriptions (org_id, tier) VALUES ($1, 'team')
         ON CONFLICT (org_id) DO NOTHING",
    )
    .bind(org_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    let _ = tenant::activate_org(&state, &user, org_id).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "org": {
                "id":                    org_id,
                "name":                  name,
                "slug":                  slug,
                "role":                  "COMPANY_ADMIN",
                "meeting_creator_roles": meeting_creator_roles,
            },
            "active_org_id": org_id,
        })),
    ))
}

// ── POST /v1/orgs/:org_id/activate ──────────────────────────────────────────

pub async fn activate(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let active = tenant::activate_org(&state, &user, org_id).await?;
    Ok(Json(json!({
        "active_org_id": active,
        "personal_mode": false,
    })))
}

// ── POST /v1/orgs/:org_id/deactivate — return to personal mode ───────────────

pub async fn deactivate(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    tenant::clear_active_org(&state, &user).await?;
    Ok(Json(json!({
        "active_org_id": null,
        "personal_mode": true,
    })))
}

// ── GET /v1/orgs/:org_id/members ────────────────────────────────────────────

pub async fn members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    tenant::ensure_org_member(&state, user.account_id, org_id).await?;

    let members: Vec<MemberInfo> = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            bool,
            DateTime<Utc>,
        ),
    >(
        "SELECT om.id,
                om.account_id,
                a.email,
                om.role,
                om.lark_name,
                om.lark_avatar_url,
                om.lark_department,
                CASE
                  WHEN om.lark_user_id IS NOT NULL THEN 'lark'
                  WHEN om.auth_source IS NOT NULL THEN om.auth_source
                  ELSE 'email'
                END AS auth_source,
                (om.lark_user_id IS NOT NULL) AS lark_connected,
                om.joined_at
           FROM org_members om
           JOIN accounts a ON a.id = om.account_id
          WHERE om.org_id = $1
          ORDER BY om.joined_at ASC",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(
        |(
            id,
            account_id,
            email,
            role,
            lark_name,
            lark_avatar_url,
            lark_department,
            auth_source,
            lark_connected,
            joined_at,
        )| MemberInfo {
            id,
            account_id,
            email,
            role,
            lark_name,
            lark_avatar_url,
            lark_department,
            auth_source,
            lark_connected,
            joined_at,
        },
    )
    .collect();

    Ok(Json(json!({ "members": members })))
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
