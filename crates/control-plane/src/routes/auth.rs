//! Auth routes:
//!   POST /v1/auth/signup   — create account + free license + session
//!   POST /v1/auth/login    — verify password + issue session
//!   POST /v1/auth/logout   — delete session
//!   GET  /v1/auth/me       — current account + license

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuthBody {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct DesktopEmailAuthBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub signup: bool,
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

// ── Desktop email auth ────────────────────────────────────────────────────────

pub async fn desktop_email(
    State(state): State<AppState>,
    Json(body): Json<DesktopEmailAuthBody>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<Value>)> {
    let email = normalize_email(&body.email)?;
    let account_id = if body.signup {
        create_account(&state, &email, &body.password).await?
    } else {
        verify_account(&state, &email, &body.password).await?
    };

    ensure_email_org_membership(&state, account_id, &email).await?;
    let token = issue_session(&state, account_id).await?;
    let tier = active_license_tier(&state, account_id).await?;

    Ok(Json(AuthResponse {
        token,
        account: AccountInfo {
            id: account_id,
            email,
            license_tier: tier,
        },
    }))
}

// ── Login ─────────────────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<AuthBody>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<Value>)> {
    let email = normalize_email(&body.email)?;
    let account_id = verify_account(&state, &email, &body.password).await?;
    let token = issue_session(&state, account_id).await?;
    let tier = active_license_tier(&state, account_id).await?;

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
    headers: HeaderMap,
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
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let orgs = tenant::list_memberships(&state, user.account_id).await?;

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
        "orgs": orgs,
        "active_org_id": tenant_ctx.active_org_id,
        "personal_mode": tenant_ctx.personal_mode,
        "org_role": tenant_ctx.org_role,
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

fn normalize_email(email: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let email = email.trim().to_lowercase();
    let valid = email.contains('@')
        && email
            .split('@')
            .nth(1)
            .map(|domain| domain.contains('.'))
            .unwrap_or(false);
    if !valid {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "valid email required"})),
        ));
    }
    Ok(email)
}

async fn create_account(
    state: &AppState,
    email: &str,
    password: &str,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    if password.len() < 8 {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "password must be >= 8 chars"})),
        ));
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

    sqlx::query("INSERT INTO license_keys (account_id, tier, active) VALUES ($1, 'free', true)")
        .bind(account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    Ok(account_id)
}

async fn verify_account(
    state: &AppState,
    email: &str,
    password: &str,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM accounts WHERE email = $1")
            .bind(email)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    let (account_id, hash) = row.ok_or_else(|| {
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
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid credentials"})),
            )
        })?;

    Ok(account_id)
}

async fn active_license_tier(
    state: &AppState,
    account_id: Uuid,
) -> Result<String, (StatusCode, Json<Value>)> {
    sqlx::query_scalar(
        "SELECT tier FROM license_keys
          WHERE account_id = $1 AND active = true
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)
    .map(|tier| tier.unwrap_or_else(|| "free".into()))
}

async fn ensure_email_org_membership(
    state: &AppState,
    account_id: Uuid,
    email: &str,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT org_id FROM org_members WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;
    if let Some(org_id) = existing {
        sqlx::query(
            "UPDATE org_members
                SET auth_source = COALESCE(auth_source, 'email')
              WHERE org_id = $1 AND account_id = $2 AND lark_user_id IS NULL",
        )
        .bind(org_id)
        .bind(account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
        return Ok(org_id);
    }

    let org_id = resolve_email_org(state, email).await?;
    let role = if org_member_count(state, org_id).await? == 0 {
        "COMPANY_ADMIN"
    } else {
        "MEMBER"
    };

    sqlx::query(
        "INSERT INTO org_members (org_id, account_id, role, auth_source)
         VALUES ($1, $2, $3, 'email')
         ON CONFLICT (org_id, account_id)
         DO UPDATE SET auth_source = COALESCE(org_members.auth_source, 'email')",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(role)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(org_id)
}

async fn resolve_email_org(
    state: &AppState,
    email: &str,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    if let Some(org_id) = single_existing_org(state).await? {
        return Ok(org_id);
    }

    let domain = email.split('@').nth(1).unwrap_or("personal");
    let slug = slug_from_domain(domain);
    if let Some(org_id) = sqlx::query_scalar("SELECT id FROM orgs WHERE slug = $1")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    {
        return Ok(org_id);
    }

    let name = format!("{} (email signup)", slug);
    sqlx::query_scalar("INSERT INTO orgs (name, slug) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(slug)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)
}

async fn single_existing_org(state: &AppState) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    let rows: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM orgs ORDER BY created_at ASC LIMIT 2")
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?;
    Ok(if rows.len() == 1 { Some(rows[0]) } else { None })
}

async fn org_member_count(
    state: &AppState,
    org_id: Uuid,
) -> Result<i64, (StatusCode, Json<Value>)> {
    sqlx::query_scalar("SELECT COUNT(*) FROM org_members WHERE org_id = $1")
        .bind(org_id)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)
}

fn slug_from_domain(domain: &str) -> String {
    let first = domain.split('.').next().unwrap_or("personal");
    let slug: String = first
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "personal".to_string()
    } else {
        slug.to_string()
    }
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
