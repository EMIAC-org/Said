//! Desktop client registration + heartbeat + admin listing.

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

#[derive(Deserialize)]
pub struct ClientBody {
    pub device_id: String,
    pub platform: String,
    pub app_version: String,
    pub hostname: Option<String>,
    pub company_bucket_version: Option<i32>,
    pub company_vocab_synced_at: Option<DateTime<Utc>>,
    pub personal_vocab_count: Option<i32>,
    pub personal_alias_count: Option<i32>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ClientRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub device_id: String,
    pub platform: String,
    pub app_version: String,
    pub hostname: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub email: Option<String>,
    pub lark_name: Option<String>,
    pub lark_avatar_url: Option<String>,
    pub auth_source: String,
    pub lark_connected: bool,
    pub company_bucket_version: i32,
    pub company_vocab_synced_at: Option<DateTime<Utc>>,
    pub personal_vocab_count: i32,
    pub personal_alias_count: i32,
}

#[derive(Serialize)]
pub struct ClientUsage {
    pub polish_count: i64,
    pub word_count: i64,
}

fn require_viewer(role: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if role.eq_ignore_ascii_case("admin")
        || role.eq_ignore_ascii_case("COMPANY_ADMIN")
        || role.eq_ignore_ascii_case("MANAGER")
        || role.eq_ignore_ascii_case("member")
        || role.eq_ignore_ascii_case("MEMBER")
    {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "insufficient permissions"})),
        ))
    }
}

/// POST /v1/clients/register — upsert desktop install on connect.
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<ClientBody>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;
    let device_id = body.device_id.trim();
    if device_id.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "device_id required"})),
        ));
    }

    sqlx::query(
        "INSERT INTO desktop_clients
            (org_id, account_id, device_id, platform, app_version, hostname,
             company_bucket_version, company_vocab_synced_at, personal_vocab_count, personal_alias_count)
         VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, 0), $8, COALESCE($9, 0), COALESCE($10, 0))
         ON CONFLICT (org_id, device_id) DO UPDATE
           SET account_id  = EXCLUDED.account_id,
               platform    = EXCLUDED.platform,
               app_version = EXCLUDED.app_version,
               hostname    = EXCLUDED.hostname,
               company_bucket_version = GREATEST(desktop_clients.company_bucket_version, EXCLUDED.company_bucket_version),
               company_vocab_synced_at = COALESCE(EXCLUDED.company_vocab_synced_at, desktop_clients.company_vocab_synced_at),
               personal_vocab_count = EXCLUDED.personal_vocab_count,
               personal_alias_count = EXCLUDED.personal_alias_count,
               last_seen_at = now()",
    )
    .bind(org_id)
    .bind(user.account_id)
    .bind(device_id)
    .bind(body.platform.trim())
    .bind(body.app_version.trim())
    .bind(body.hostname.as_deref())
    .bind(body.company_bucket_version)
    .bind(body.company_vocab_synced_at)
    .bind(body.personal_vocab_count)
    .bind(body.personal_alias_count)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /v1/clients/heartbeat — refresh last_seen_at.
pub async fn heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<ClientBody>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;
    let device_id = body.device_id.trim();
    if device_id.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "device_id required"})),
        ));
    }

    let updated = sqlx::query(
        "UPDATE desktop_clients
            SET last_seen_at = now(),
                app_version  = COALESCE(NULLIF($4, ''), app_version),
                platform     = COALESCE(NULLIF($5, ''), platform),
                company_bucket_version = GREATEST(company_bucket_version, COALESCE($6, company_bucket_version)),
                company_vocab_synced_at = COALESCE($7, company_vocab_synced_at),
                personal_vocab_count = COALESCE($8, personal_vocab_count),
                personal_alias_count = COALESCE($9, personal_alias_count)
          WHERE org_id = $1 AND device_id = $2 AND account_id = $3",
    )
    .bind(org_id)
    .bind(device_id)
    .bind(user.account_id)
    .bind(body.app_version.trim())
    .bind(body.platform.trim())
    .bind(body.company_bucket_version)
    .bind(body.company_vocab_synced_at)
    .bind(body.personal_vocab_count)
    .bind(body.personal_alias_count)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    if updated.rows_affected() == 0 {
        return register(State(state), headers, user, Json(body)).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /v1/orgs/:org_id/clients — list org desktop installs.
pub async fn list_org_clients(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id).await?;
    require_viewer(&role)?;

    let clients: Vec<ClientRow> = sqlx::query_as::<_, ClientRow>(
        "SELECT dc.id, dc.account_id, dc.device_id, dc.platform, dc.app_version,
                dc.hostname, dc.first_seen_at, dc.last_seen_at,
                a.email, om.lark_name, om.lark_avatar_url,
                CASE
                  WHEN om.lark_user_id IS NOT NULL THEN 'lark'
                  WHEN om.auth_source IS NOT NULL THEN om.auth_source
                  ELSE 'email'
                END AS auth_source,
                (om.lark_user_id IS NOT NULL) AS lark_connected,
                dc.company_bucket_version, dc.company_vocab_synced_at,
                dc.personal_vocab_count, dc.personal_alias_count
           FROM desktop_clients dc
           JOIN accounts a ON a.id = dc.account_id
           LEFT JOIN org_members om ON om.account_id = dc.account_id AND om.org_id = dc.org_id
          WHERE dc.org_id = $1
          ORDER BY dc.last_seen_at DESC",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({ "clients": clients })))
}

/// GET /v1/orgs/:org_id/clients/:account_id/usage — 7-day usage for one user.
pub async fn client_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, account_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id).await?;
    require_viewer(&role)?;

    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SUM(polish_count), 0), COALESCE(SUM(word_count), 0)
           FROM org_usage_daily
          WHERE org_id = $1 AND account_id = $2
            AND event_date >= (CURRENT_DATE - INTERVAL '7 days')",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let (polish_count, word_count) = row.unwrap_or((0, 0));

    Ok(Json(json!({
        "usage": ClientUsage { polish_count, word_count },
    })))
}

/// GET /v1/orgs/:org_id/stats — dashboard aggregates.
pub async fn org_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id).await?;
    require_viewer(&role)?;

    let active_desktops: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM desktop_clients
          WHERE org_id = $1 AND last_seen_at > now() - INTERVAL '15 minutes'",
    )
    .bind(org_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let total_desktops: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM desktop_clients WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&state.db)
            .await
            .map_err(db_err)?;

    let member_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM org_members WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&state.db)
            .await
            .map_err(db_err)?;

    let (polish_7d, words_7d): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(polish_count), 0), COALESCE(SUM(word_count), 0)
           FROM org_usage_daily
          WHERE org_id = $1
            AND event_date >= (CURRENT_DATE - INTERVAL '7 days')",
    )
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?
    .unwrap_or((0, 0));

    Ok(Json(json!({
        "active_desktops": active_desktops,
        "total_desktops": total_desktops,
        "member_count": member_count,
        "usage_7d": {
            "polish_count": polish_7d,
            "word_count": words_7d,
        },
    })))
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
