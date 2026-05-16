//! Org routes:
//!   GET  /v1/orgs/me             — get the current user's org
//!   POST /v1/orgs                — create an org (caller becomes admin)
//!   GET  /v1/orgs/:org_id/members — list org members

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

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
    pub role: String,
    pub lark_name: Option<String>,
    pub lark_avatar_url: Option<String>,
    pub lark_department: Option<String>,
    pub joined_at: DateTime<Utc>,
}

// ── GET /v1/orgs/me ─────────────────────────────────────────────────────────

pub async fn me(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
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
          WHERE om.account_id = $1
          LIMIT 1",
    )
    .bind(user.account_id)
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
        }
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

    let already_member: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE account_id = $1)")
            .bind(user.account_id)
            .fetch_one(&state.db)
            .await
            .map_err(db_err)?;

    if already_member {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "account already belongs to an org"})),
        ));
    }

    // Check slug uniqueness
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

    // Build meeting_creator_roles (default to ["COMPANY_ADMIN", "MANAGER"])
    let meeting_creator_roles: Value = body
        .meeting_creator_roles
        .map(|roles| json!(roles))
        .unwrap_or_else(|| json!(["COMPANY_ADMIN", "MANAGER"]));

    // Insert org
    let org_id: Uuid = sqlx::query_scalar(
        "INSERT INTO orgs (name, slug, meeting_creator_roles) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&name)
    .bind(&slug)
    .bind(&meeting_creator_roles)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    // Add creator as company admin. Meeting creation checks this role by default.
    sqlx::query(
        "INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'COMPANY_ADMIN')",
    )
    .bind(org_id)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "org": {
                "id":                    org_id,
                "name":                  name,
                "slug":                  slug,
                "role":                  "COMPANY_ADMIN",
                "meeting_creator_roles": meeting_creator_roles,
            }
        })),
    ))
}

// ── GET /v1/orgs/:org_id/members ────────────────────────────────────────────

pub async fn members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Verify caller is a member of this org
    let is_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND account_id = $2)",
    )
    .bind(org_id)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    if !is_member {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "you are not a member of this org"})),
        ));
    }

    let members: Vec<(
        Uuid,
        Uuid,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, account_id, role, lark_name, lark_avatar_url, lark_department, joined_at
               FROM org_members
              WHERE org_id = $1
              ORDER BY joined_at ASC",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let members_json: Vec<Value> = members
        .into_iter()
        .map(
            |(id, account_id, role, lark_name, lark_avatar_url, lark_department, joined_at)| {
                json!({
                    "id":              id,
                    "account_id":      account_id,
                    "role":            role,
                    "lark_name":       lark_name,
                    "lark_avatar_url": lark_avatar_url,
                    "lark_department": lark_department,
                    "joined_at":       joined_at,
                })
            },
        )
        .collect();

    Ok(Json(json!({ "members": members_json })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
