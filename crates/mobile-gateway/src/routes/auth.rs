//! Mobile auth — self-contained email/password with opaque bearer tokens.
//!
//!   POST /v1/auth/mobile-email    — signup-or-login → access + refresh tokens
//!   POST /v1/auth/mobile-refresh  — exchange refresh token for a new access token
//!
//! This service owns its own `accounts`; it never calls the control-plane.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{Json, extract::State, http::StatusCode};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    util::{ApiResult, bad_request, db_err},
};

const ACCESS_TTL_DAYS: i64 = 30;
const REFRESH_TTL_DAYS: i64 = 90;

#[derive(Deserialize)]
pub struct MobileEmailBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub signup: bool,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: Uuid,
    pub refresh_token: Uuid,
    pub account: AccountInfo,
    pub policy: Value,
}

#[derive(Serialize)]
pub struct AccountInfo {
    pub id: Uuid,
    pub email: String,
    pub license_tier: String,
}

pub async fn mobile_email(
    State(state): State<AppState>,
    Json(body): Json<MobileEmailBody>,
) -> ApiResult<Json<AuthResponse>> {
    let email = normalize_email(&body.email)?;
    let account_id = if body.signup {
        create_account(&state, &email, &body.password).await?
    } else {
        verify_account(&state, &email, &body.password).await?
    };

    let token = issue_token(&state, account_id, "access", ACCESS_TTL_DAYS).await?;
    let refresh_token = issue_token(&state, account_id, "refresh", REFRESH_TTL_DAYS).await?;

    Ok(Json(AuthResponse {
        token,
        refresh_token,
        account: AccountInfo {
            id: account_id,
            email,
            license_tier: "free".into(),
        },
        policy: policy_block(),
    }))
}

#[derive(Deserialize)]
pub struct RefreshBody {
    pub refresh_token: Uuid,
}

pub async fn mobile_refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshBody>,
) -> ApiResult<Json<Value>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT account_id FROM auth_sessions
          WHERE token = $1 AND kind = 'refresh' AND expires_at > now()",
    )
    .bind(body.refresh_token)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some((account_id,)) = row else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid or expired refresh token"})),
        ));
    };

    let token = issue_token(&state, account_id, "access", ACCESS_TTL_DAYS).await?;
    Ok(Json(json!({ "token": token })))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn policy_block() -> Value {
    json!({
        "mobile_enabled": true,
        "max_recording_seconds": crate::util::MAX_RECORDING_SECONDS,
        "streaming_enabled": true,
        "audio_retention_seconds": 0,
        "raw_text_retention": "none",
        "learning_mode": "insert_first_learn_later",
        "allow_transcript_history": true
    })
}

fn normalize_email(email: &str) -> ApiResult<String> {
    let email = email.trim().to_lowercase();
    let valid = email.contains('@')
        && email
            .split('@')
            .nth(1)
            .map(|domain| domain.contains('.'))
            .unwrap_or(false);
    if !valid {
        return Err(bad_request("valid email required"));
    }
    Ok(email)
}

async fn create_account(state: &AppState, email: &str, password: &str) -> ApiResult<Uuid> {
    if password.len() < 8 {
        return Err(bad_request("password must be >= 8 chars"));
    }

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE email = $1)")
        .bind(email)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;
    if exists {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "email already registered"})),
        ));
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "hash failed"})),
            )
        })?
        .to_string();

    let account_id: Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (email, password_hash) VALUES ($1, $2) RETURNING id",
    )
    .bind(email)
    .bind(&hash)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(account_id)
}

async fn verify_account(state: &AppState, email: &str, password: &str) -> ApiResult<Uuid> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM accounts WHERE email = $1")
            .bind(email)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    let (account_id, hash) = row.ok_or((
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "invalid credentials"})),
    ))?;

    let parsed = PasswordHash::new(&hash).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "hash parse failed"})),
        )
    })?;

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid credentials"})),
            )
        })?;

    Ok(account_id)
}

async fn issue_token(
    state: &AppState,
    account_id: Uuid,
    kind: &str,
    ttl_days: i64,
) -> ApiResult<Uuid> {
    let expires_at = Utc::now() + Duration::days(ttl_days);
    let token: Uuid = sqlx::query_scalar(
        "INSERT INTO auth_sessions (account_id, kind, expires_at)
         VALUES ($1, $2, $3) RETURNING token",
    )
    .bind(account_id)
    .bind(kind)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;
    Ok(token)
}
