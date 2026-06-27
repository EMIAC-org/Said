//! Multi-org tenant context: active workspace resolution and membership checks.

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

pub const ORG_HEADER: &str = "x-airnote-org-id";

#[derive(Clone, Debug)]
pub struct TenantContext {
    pub account_id: Uuid,
    pub active_org_id: Option<Uuid>,
    pub org_role: Option<String>,
    pub personal_mode: bool,
}

#[derive(Clone, Serialize)]
pub struct OrgMembership {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

pub fn multi_org_enabled() -> bool {
    matches!(
        std::env::var("MULTI_ORG_ENABLED")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn allow_platform_credential_fallback() -> bool {
    if !multi_org_enabled() {
        return true;
    }
    matches!(
        std::env::var("ALLOW_PLATFORM_CREDENTIAL_FALLBACK")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn parse_org_header(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get(ORG_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
}

pub async fn resolve_tenant(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
) -> Result<TenantContext, (StatusCode, Json<Value>)> {
    let header_org = parse_org_header(headers);

    // Hot path: desktop dictation sends no org header, and active-org/role only
    // change on explicit activate/clear (which invalidate this entry). Serving
    // the resolved tenant from memory skips ~4 DB round-trips per dictation.
    if header_org.is_none() {
        if let Some(cached) = state.tenant_cache.get(&user.account_id) {
            return Ok(cached);
        }
    }

    let (session_org, account_org) =
        session_and_account_org(state, user.session_token, user.account_id)
            .await
            .map_err(db_err)?;

    let active_org_id = if let Some(org_id) = header_org {
        ensure_org_member(state, user.account_id, org_id).await?;
        Some(org_id)
    } else if let Some(org_id) = session_org {
        ensure_org_member(state, user.account_id, org_id).await?;
        Some(org_id)
    } else if let Some(org_id) = account_org {
        ensure_org_member(state, user.account_id, org_id).await?;
        Some(org_id)
    } else if multi_org_enabled() {
        None
    } else {
        legacy_primary_org_id(state, user.account_id).await?
    };

    let org_role = if let Some(org_id) = active_org_id {
        Some(fetch_org_role(state, user.account_id, org_id).await?)
    } else {
        None
    };

    let ctx = TenantContext {
        account_id: user.account_id,
        active_org_id,
        org_role,
        personal_mode: active_org_id.is_none(),
    };

    // Only the header-less resolution is account-stable; a header override is a
    // per-request scope and must not poison the cached default.
    if header_org.is_none() {
        state.tenant_cache.insert(user.account_id, ctx.clone());
    }

    Ok(ctx)
}

pub async fn require_active_org(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
) -> Result<(TenantContext, Uuid), (StatusCode, Json<Value>)> {
    let tenant = resolve_tenant(state, user, headers).await?;
    let org_id = tenant.active_org_id.ok_or_else(|| {
        json_error(
            StatusCode::FORBIDDEN,
            "active workspace required — set X-AirNote-Org-Id or POST /v1/orgs/:id/activate",
        )
    })?;
    Ok((tenant, org_id))
}

pub async fn require_active_org_role(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
) -> Result<(Uuid, String), (StatusCode, Json<Value>)> {
    let (tenant, org_id) = require_active_org(state, user, headers).await?;
    let role = tenant
        .org_role
        .ok_or_else(|| json_error(StatusCode::FORBIDDEN, "account is not a member of this org"))?;
    Ok((org_id, role))
}

pub async fn require_org_membership(
    state: &AppState,
    account_id: Uuid,
    org_id: Uuid,
) -> Result<String, (StatusCode, Json<Value>)> {
    ensure_org_member(state, account_id, org_id).await?;
    fetch_org_role(state, account_id, org_id).await
}

pub async fn ensure_org_member(
    state: &AppState,
    account_id: Uuid,
    org_id: Uuid,
) -> Result<(), (StatusCode, Json<Value>)> {
    let is_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM org_members WHERE account_id = $1 AND org_id = $2
        )",
    )
    .bind(account_id)
    .bind(org_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    if is_member {
        Ok(())
    } else {
        Err(json_error(
            StatusCode::FORBIDDEN,
            "account is not a member of this org",
        ))
    }
}

pub async fn ensure_path_org_active(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
    path_org_id: Uuid,
) -> Result<(TenantContext, String), (StatusCode, Json<Value>)> {
    let role = require_org_membership(state, user.account_id, path_org_id).await?;
    let tenant = resolve_tenant(state, user, headers).await?;
    if multi_org_enabled()
        && tenant.active_org_id.is_some()
        && tenant.active_org_id != Some(path_org_id)
    {
        let is_admin = role.eq_ignore_ascii_case("admin")
            || role.eq_ignore_ascii_case("company_admin")
            || role.eq_ignore_ascii_case("COMPANY_ADMIN");
        if !is_admin {
            return Err(json_error(
                StatusCode::FORBIDDEN,
                "path org_id does not match active workspace",
            ));
        }
    }
    Ok((tenant, role))
}

pub async fn list_memberships(
    state: &AppState,
    account_id: Uuid,
) -> Result<Vec<OrgMembership>, (StatusCode, Json<Value>)> {
    let active: Option<Uuid> =
        sqlx::query_scalar("SELECT active_org_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?
            .flatten();

    let rows: Vec<(Uuid, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT o.id, o.name, o.slug, om.role, o.created_at
           FROM org_members om
           JOIN orgs o ON o.id = om.org_id
          WHERE om.account_id = $1
          ORDER BY o.name ASC",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|(id, name, slug, role, created_at)| OrgMembership {
            is_active: active == Some(id),
            id,
            name,
            slug,
            role,
            created_at,
        })
        .collect())
}

pub async fn activate_org(
    state: &AppState,
    user: &AuthUser,
    org_id: Uuid,
) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    ensure_org_member(state, user.account_id, org_id).await?;

    sqlx::query("UPDATE accounts SET active_org_id = $1 WHERE id = $2")
        .bind(org_id)
        .bind(user.account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    if let Some(token) = user.session_token {
        sqlx::query("UPDATE sessions SET active_org_id = $1 WHERE token = $2")
            .bind(org_id)
            .bind(token)
            .execute(&state.db)
            .await
            .map_err(db_err)?;
    }

    state.tenant_cache.invalidate(&user.account_id);

    Ok(Some(org_id))
}

pub async fn clear_active_org(
    state: &AppState,
    user: &AuthUser,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query("UPDATE accounts SET active_org_id = NULL WHERE id = $1")
        .bind(user.account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    if let Some(token) = user.session_token {
        sqlx::query("UPDATE sessions SET active_org_id = NULL WHERE token = $1")
            .bind(token)
            .execute(&state.db)
            .await
            .map_err(db_err)?;
    }

    state.tenant_cache.invalidate(&user.account_id);

    Ok(())
}

async fn session_and_account_org(
    state: &AppState,
    session_token: Option<Uuid>,
    account_id: Uuid,
) -> Result<(Option<Uuid>, Option<Uuid>), sqlx::Error> {
    let session_org = if let Some(token) = session_token {
        sqlx::query_scalar("SELECT active_org_id FROM sessions WHERE token = $1")
            .bind(token)
            .fetch_optional(&state.db)
            .await?
            .flatten()
    } else {
        None
    };

    let account_org: Option<Uuid> =
        sqlx::query_scalar("SELECT active_org_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();

    Ok((session_org, account_org))
}

/// Resolve active org for WebSocket sessions (no HTTP headers).
pub async fn resolve_ws_org_id(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    let account_org: Option<Uuid> =
        sqlx::query_scalar("SELECT active_org_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?
            .flatten();

    if let Some(org_id) = account_org {
        ensure_org_member(state, account_id, org_id).await?;
        return Ok(Some(org_id));
    }

    if multi_org_enabled() {
        Ok(None)
    } else {
        legacy_primary_org_id(state, account_id).await
    }
}

async fn legacy_primary_org_id(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    sqlx::query_scalar(
        "SELECT org_id
           FROM org_members
          WHERE account_id = $1
          ORDER BY joined_at ASC
          LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)
}

async fn fetch_org_role(
    state: &AppState,
    account_id: Uuid,
    org_id: Uuid,
) -> Result<String, (StatusCode, Json<Value>)> {
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM org_members WHERE account_id = $1 AND org_id = $2")
            .bind(account_id)
            .bind(org_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    role.ok_or_else(|| json_error(StatusCode::FORBIDDEN, "account is not a member of this org"))
}

pub fn json_error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": message})))
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_org_flag_defaults_false() {
        unsafe {
            std::env::remove_var("MULTI_ORG_ENABLED");
        }
        assert!(!multi_org_enabled());
    }
}
