//! Bug report routes.
//!
//! Desktop creates a short-lived signed token, then opens `/report-bug?token=...`.
//! The public form can submit without an admin session, but only with that token.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

const REPORT_PURPOSE: &str = "bug_report";
const MAX_SCREENSHOT_DATA_URL_BYTES: usize = 6 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct BugReportClaims {
    sub: String,
    email: String,
    org_id: Option<String>,
    exp: usize,
    purpose: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateReportSessionBody {
    pub app_version: Option<String>,
    pub platform: Option<String>,
    pub device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PublicBugReportBody {
    pub token: String,
    pub title: String,
    pub description: String,
    pub severity: Option<String>,
    pub app_version: Option<String>,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub screenshot_url: Option<String>,
    pub screenshot_data_url: Option<String>,
    pub screenshot_name: Option<String>,
    pub screenshot_mime: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBugStatusBody {
    pub status: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct BugReportRow {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub reporter_email: Option<String>,
    pub reporter_name: Option<String>,
    pub title: String,
    pub description: String,
    pub severity: String,
    pub status: String,
    pub app_version: Option<String>,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub screenshot_url: Option<String>,
    pub screenshot_data_url: Option<String>,
    pub screenshot_name: Option<String>,
    pub screenshot_mime: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<CreateReportSessionBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let expires_at = Utc::now() + Duration::days(7);
    let claims = BugReportClaims {
        sub: user.account_id.to_string(),
        email: user.email,
        org_id: Some(org_id.to_string()),
        exp: expires_at.timestamp() as usize,
        purpose: REPORT_PURPOSE.to_string(),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
    )
    .map_err(|_| internal("failed to sign report token"))?;

    let mut path = format!("/report-bug?token={}", urlencoding::encode(&token));
    append_optional_query(&mut path, "app_version", body.app_version.as_deref());
    append_optional_query(&mut path, "platform", body.platform.as_deref());
    append_optional_query(&mut path, "device_id", body.device_id.as_deref());

    Ok(Json(json!({
        "token": token,
        "path": path,
        "url": path,
        "expires_at": expires_at,
    })))
}

pub async fn submit_public(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PublicBugReportBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let claims = decode_report_token(&state, &body.token)?;
    let account_id =
        Uuid::parse_str(&claims.sub).map_err(|_| unauthorized("invalid report token"))?;
    let org_id = claims
        .org_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok());

    let title = clean_required(&body.title, 140, "title")?;
    let description = clean_required(&body.description, 10_000, "description")?;
    let severity = normalize_severity(body.severity.as_deref());
    let screenshot_data_url = normalize_optional(body.screenshot_data_url, 0);
    if let Some(data_url) = &screenshot_data_url {
        if data_url.len() > MAX_SCREENSHOT_DATA_URL_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error": "screenshot too large"})),
            ));
        }
        if !data_url.starts_with("data:image/") {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": "screenshot must be an image"})),
            ));
        }
    }

    let reporter_name: Option<String> = if let Some(org_id) = org_id {
        sqlx::query_scalar(
            "SELECT lark_name
               FROM org_members
              WHERE account_id = $1 AND org_id = $2",
        )
        .bind(account_id)
        .bind(org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    } else {
        None
    };

    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(500).collect::<String>());

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO bug_reports (
            org_id, account_id, reporter_email, reporter_name, title, description,
            severity, app_version, platform, device_id, screenshot_url,
            screenshot_data_url, screenshot_name, screenshot_mime, user_agent
         )
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
         RETURNING id",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(claims.email)
    .bind(reporter_name)
    .bind(title)
    .bind(description)
    .bind(severity)
    .bind(normalize_optional(body.app_version, 80))
    .bind(normalize_optional(body.platform, 80))
    .bind(normalize_optional(body.device_id, 160))
    .bind(normalize_optional(body.screenshot_url, 2_000))
    .bind(screenshot_data_url)
    .bind(normalize_optional(body.screenshot_name, 240))
    .bind(normalize_optional(body.screenshot_mime, 120))
    .bind(user_agent)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({ "ok": true, "id": id })))
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let rows: Vec<BugReportRow> = sqlx::query_as(
        "SELECT id, org_id, account_id, reporter_email, reporter_name, title, description,
                severity, status, app_version, platform, device_id, screenshot_url,
                screenshot_data_url, screenshot_name, screenshot_mime, created_at, updated_at
           FROM bug_reports
          WHERE org_id = $1
          ORDER BY created_at DESC
          LIMIT 200",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({ "bugs": rows })))
}

pub async fn update_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBugStatusBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;
    let status = normalize_status(&body.status);
    let updated: Option<BugReportRow> = sqlx::query_as(
        "UPDATE bug_reports
            SET status = $2, updated_at = now()
          WHERE id = $1
            AND org_id = $3
          RETURNING id, org_id, account_id, reporter_email, reporter_name, title, description,
                    severity, status, app_version, platform, device_id, screenshot_url,
                    screenshot_data_url, screenshot_name, screenshot_mime, created_at, updated_at",
    )
    .bind(id)
    .bind(status)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some(row) = updated else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "bug report not found"})),
        ));
    };

    Ok(Json(json!({ "bug": row })))
}

fn decode_report_token(
    state: &AppState,
    token: &str,
) -> Result<BugReportClaims, (StatusCode, Json<Value>)> {
    let mut validation = Validation::default();
    validation.validate_exp = true;
    let token_data = decode::<BugReportClaims>(
        token,
        &DecodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
        &validation,
    )
    .map_err(|_| unauthorized("invalid or expired report token"))?;

    if token_data.claims.purpose != REPORT_PURPOSE {
        return Err(unauthorized("invalid report token"));
    }
    Ok(token_data.claims)
}

fn append_optional_query(path: &mut String, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };
    path.push('&');
    path.push_str(key);
    path.push('=');
    path.push_str(&urlencoding::encode(value));
}

fn clean_required(
    value: &str,
    max_chars: usize,
    field: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": format!("{field} is required")})),
        ));
    }
    Ok(cleaned.chars().take(max_chars).collect())
}

fn normalize_optional(value: Option<String>, max_chars: usize) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else if max_chars > 0 {
            Some(trimmed.chars().take(max_chars).collect())
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_severity(value: Option<&str>) -> &'static str {
    match value
        .unwrap_or("normal")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "low" => "low",
        "high" => "high",
        "blocking" => "blocking",
        _ => "normal",
    }
}

fn normalize_status(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "open" => "open",
        "triaged" => "triaged",
        "fixed" => "fixed",
        "closed" => "closed",
        _ => "open",
    }
}

fn unauthorized(message: &'static str) -> (StatusCode, Json<Value>) {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message })))
}

fn internal(message: &'static str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message })),
    )
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
