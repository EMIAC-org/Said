//! Auth routes:
//!   POST /v1/auth/signup   — create account + free license + session
//!   POST /v1/auth/login    — verify password + issue session
//!   POST /v1/auth/logout   — delete session
//!   GET  /v1/auth/me       — current account + license

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{Json, extract::State, http::StatusCode};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuthBody {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: Uuid,
    pub account: AccountInfo,
}

#[derive(Serialize)]
pub struct AccountInfo {
    pub id: Uuid,
    pub email: String,
    pub license_tier: String,
}

// ── Signup ────────────────────────────────────────────────────────────────────

pub async fn signup(
    State(state): State<AppState>,
    Json(body): Json<AuthBody>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<Value>)> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || body.password.len() < 8 {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "email required and password must be >= 8 chars"})),
        ));
    }

    // Check email uniqueness
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE email = $1)")
        .bind(&email)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;

    if exists {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "email already registered"})),
        ));
    }

    // Hash password
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "hash failed"})),
            )
        })?
        .to_string();

    // Insert account
    let account_id: Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (email, password_hash) VALUES ($1, $2) RETURNING id",
    )
    .bind(&email)
    .bind(&hash)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    // Create free license key
    sqlx::query("INSERT INTO license_keys (account_id, tier, active) VALUES ($1, 'free', true)")
        .bind(account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    // Create session (30 days)
    let token = issue_session(&state, account_id).await?;

    Ok(Json(AuthResponse {
        token,
        account: AccountInfo {
            id: account_id,
            email,
            license_tier: "free".into(),
        },
    }))
}

// ── Login ─────────────────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<AuthBody>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<Value>)> {
    let email = body.email.trim().to_lowercase();

    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM accounts WHERE email = $1")
            .bind(&email)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    let (account_id, hash) = row.ok_or_else(|| {
        // Constant-time failure (don't leak account existence)
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid credentials"})),
        )
    })?;

    let parsed = PasswordHash::new(&hash).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "hash parse failed"})),
        )
    })?;

    Argon2::default()
        .verify_password(body.password.as_bytes(), &parsed)
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid credentials"})),
            )
        })?;

    let token = issue_session(&state, account_id).await?;

    // Fetch license tier
    let tier: String = sqlx::query_scalar(
        "SELECT tier FROM license_keys
          WHERE account_id = $1 AND active = true
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?
    .unwrap_or_else(|| "free".into());

    Ok(Json(AuthResponse {
        token,
        account: AccountInfo {
            id: account_id,
            email,
            license_tier: tier,
        },
    }))
}

// ── Logout ────────────────────────────────────────────────────────────────────

pub async fn logout(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    // Delete all sessions for this account (single-device in v1)
    sqlx::query("DELETE FROM sessions WHERE account_id = $1")
        .bind(user.account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Me ────────────────────────────────────────────────────────────────────────

pub async fn me(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tier: String = sqlx::query_scalar(
        "SELECT tier FROM license_keys
          WHERE account_id = $1 AND active = true
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user.account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?
    .unwrap_or_else(|| "free".into());

    let features = license_features(&tier);

    Ok(Json(json!({
        "account": {
            "id":    user.account_id,
            "email": user.email,
        },
        "license": {
            "tier":     tier,
            "active":   true,
            "features": features,
        },
    })))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn issue_session(
    state: &AppState,
    account_id: Uuid,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let expires_at = Utc::now() + Duration::days(30);
    let token: Uuid = sqlx::query_scalar(
        "INSERT INTO sessions (account_id, expires_at)
         VALUES ($1, $2) RETURNING token",
    )
    .bind(account_id)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;
    Ok(token)
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}

/// Return the feature set for a given tier.
pub fn license_features(tier: &str) -> Value {
    match tier {
        "pro" | "team" => json!({
            "rag_examples":   10,
            "history_days":   90,
            "models":         ["fast", "smart", "claude", "gemini"],
            "custom_persona": true,
        }),
        _ => json!({                           // "free"
            "rag_examples":   5,
            "history_days":   7,
            "models":         ["fast", "smart"],
            "custom_persona": false,
        }),
    }
}
