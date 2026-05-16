//! Lark OAuth routes:
//!   GET  /v1/auth/lark/start     — generate OAuth URL (requires auth)
//!   GET  /v1/auth/lark/callback  — OAuth redirect handler (public)
//!   POST /v1/auth/lark/refresh   — refresh Lark access token (requires auth)

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

// ── Query / JWT types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String, // account_id
    email: String,
    exp: usize, // expiry timestamp
}

// ── GET /v1/auth/lark/start ─────────────────────────────────────────────────

pub async fn start(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let oauth_state = Uuid::new_v4().to_string();

    let url = crate::lark_client::build_oauth_url(
        &state.lark.app_id,
        &state.lark.redirect_uri,
        &oauth_state,
    );

    Ok(Json(json!({
        "url":   url,
        "state": oauth_state,
    })))
}

// ── GET /v1/auth/lark/callback ──────────────────────────────────────────────

pub async fn callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackQuery>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    // Validate state is a valid UUID
    Uuid::parse_str(&params.state).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid state parameter"})),
        )
    })?;

    // Exchange auth code for tokens
    let tokens =
        crate::lark_client::exchange_code(&state.lark.app_id, &state.lark.app_secret, &params.code)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("token exchange failed: {e}")})),
                )
            })?;

    // Get Lark user profile
    let lark_user = crate::lark_client::get_user_info(&tokens.access_token)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("failed to fetch lark user info: {e}")})),
            )
        })?;

    // ── Find or create account ──────────────────────────────────────────────
    let lark_email = lark_user
        .email
        .clone()
        .unwrap_or_else(|| format!("{}@lark.user", lark_user.open_id));
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, email FROM accounts WHERE email = $1")
            .bind(&lark_email)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    let (account_id, email) = if let Some(row) = existing {
        row
    } else {
        // Create account with a random password hash (user authenticates via Lark)
        let salt = SaltString::generate(&mut OsRng);
        let random_pw = Uuid::new_v4().to_string();
        let hash = Argon2::default()
            .hash_password(random_pw.as_bytes(), &salt)
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "hash failed"})),
                )
            })?
            .to_string();

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO accounts (email, password_hash) VALUES ($1, $2) RETURNING id",
        )
        .bind(&lark_email)
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;

        // Create free license key
        sqlx::query(
            "INSERT INTO license_keys (account_id, tier, active) VALUES ($1, 'free', true)",
        )
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

        (id, lark_email.clone())
    };

    // ── Find or auto-create org + membership ──────────────────────────────────
    let org_row: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT id, org_id FROM org_members WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    let (member_id, org_id) = if let Some(row) = org_row {
        row
    } else {
        // First Lark login and no org yet — auto-create one from the email domain
        let domain = lark_email.split('@').nth(1).unwrap_or("default");
        let slug = domain.split('.').next().unwrap_or("org");
        let org_name = format!("{} (auto)", slug);

        // Reuse existing org with this slug, or create new
        let existing_org: Option<Uuid> = sqlx::query_scalar("SELECT id FROM orgs WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

        let oid = if let Some(id) = existing_org {
            id
        } else {
            sqlx::query_scalar("INSERT INTO orgs (name, slug) VALUES ($1, $2) RETURNING id")
                .bind(&org_name)
                .bind(slug)
                .fetch_one(&state.db)
                .await
                .map_err(db_err)?
        };

        sqlx::query(
            "INSERT INTO org_members (org_id, account_id, role, lark_user_id, lark_name, lark_avatar_url, lark_department)
             VALUES ($1, $2, 'COMPANY_ADMIN', $3, $4, $5, $6)",
        )
        .bind(oid)
        .bind(account_id)
        .bind(&lark_user.user_id)
        .bind(&lark_user.name)
        .bind(&lark_user.avatar_url)
        .bind(&lark_user.department)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

        let mid: Uuid =
            sqlx::query_scalar("SELECT id FROM org_members WHERE org_id = $1 AND account_id = $2")
                .bind(oid)
                .bind(account_id)
                .fetch_one(&state.db)
                .await
                .map_err(db_err)?;

        (mid, oid)
    };

    // Update Lark profile fields on existing membership
    if org_row.is_some() {
        sqlx::query(
            "UPDATE org_members
                SET lark_user_id    = $1,
                    lark_name       = $2,
                    lark_avatar_url = $3,
                    lark_department = $4
              WHERE id = $5",
        )
        .bind(&lark_user.user_id)
        .bind(&lark_user.name)
        .bind(&lark_user.avatar_url)
        .bind(&lark_user.department)
        .bind(member_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
    }

    // ── Store Lark tokens ──────────────────────────────────────────────────
    let expires_at = Utc::now() + Duration::seconds(tokens.expires_in);

    sqlx::query(
        "INSERT INTO lark_tokens (account_id, org_id, access_token, refresh_token, token_expires_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (account_id, org_id)
         DO UPDATE SET access_token     = EXCLUDED.access_token,
                       refresh_token    = EXCLUDED.refresh_token,
                       token_expires_at = EXCLUDED.token_expires_at,
                       updated_at       = now()",
    )
    .bind(account_id)
    .bind(org_id)
    .bind(&tokens.access_token)
    .bind(&tokens.refresh_token)
    .bind(expires_at)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    // ── Issue session token (for API calls) ────────────────────────────────
    let session_expires = Utc::now() + Duration::days(30);
    let session_token: Uuid = sqlx::query_scalar(
        "INSERT INTO sessions (account_id, expires_at) VALUES ($1, $2) RETURNING token",
    )
    .bind(account_id)
    .bind(session_expires)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    // ── Issue JWT (legacy, also valid for auth) ─────────────────────────────
    let exp = (Utc::now() + Duration::hours(24)).timestamp() as usize;
    let claims = Claims {
        sub: account_id.to_string(),
        email: email.clone(),
        exp,
    };

    let _jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "failed to sign JWT"})),
        )
    })?;

    let lark_name = if lark_user.name.is_empty() {
        "User"
    } else {
        &lark_user.name
    };

    Ok(axum::response::Html(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Said Enterprise — Connected</title>
<style>
  * {{ margin:0; padding:0; box-sizing:border-box }}
  body {{ font-family:'Inter',system-ui,sans-serif; background:#080b16; color:#e8eaf0; min-height:100vh; display:flex; align-items:center; justify-content:center }}
  .card {{ background:#0e1225; border:1px solid #1a2038; border-radius:20px; padding:40px; max-width:420px; width:100%; text-align:center }}
  .icon {{ width:48px; height:48px; background:rgba(117,145,239,0.15); border-radius:14px; display:flex; align-items:center; justify-content:center; margin:0 auto 20px }}
  .icon svg {{ color:#9aaef3 }}
  h1 {{ font-size:20px; font-weight:600; margin-bottom:6px }}
  .sub {{ font-size:13px; color:#7a80a0; margin-bottom:24px }}
  .token-box {{ background:#080b16; border:1px solid #1a2038; border-radius:12px; padding:12px 16px; font-family:'SF Mono',monospace; font-size:11px; color:#9aaef3; word-break:break-all; margin-bottom:16px; cursor:pointer; position:relative }}
  .token-box:hover {{ border-color:#9aaef3 }}
  .copy-btn {{ background:#7591ef; color:#fff; border:none; padding:10px 24px; border-radius:12px; font-size:13px; font-weight:600; cursor:pointer; width:100%; margin-bottom:12px }}
  .copy-btn:hover {{ background:#5a7ae8 }}
  .hint {{ font-size:11px; color:#4a5070 }}
  .copied {{ color:#4ade80; font-size:12px; font-weight:500; margin-top:8px }}
</style>
</head>
<body>
<div class="card">
  <div class="icon">
    <svg viewBox="0 0 24 24" fill="none" width="22" height="22">
      <rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor"/>
      <rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor"/>
      <rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor"/>
      <rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor"/>
    </svg>
  </div>
  <h1>Welcome, {lark_name}!</h1>
  <p class="sub">You're now connected to Said Enterprise.<br/>Copy the token below and paste it in the Said desktop app.</p>
  <div class="token-box" id="token" onclick="copyToken()">{session_token}</div>
  <button class="copy-btn" onclick="copyToken()">Copy Token</button>
  <p class="hint">Go to Said → Settings → Enterprise → Paste this token</p>
  <p class="copied" id="copied" style="display:none">✓ Copied to clipboard</p>
</div>
<script>
function copyToken() {{
  navigator.clipboard.writeText('{session_token}').then(function() {{
    document.getElementById('copied').style.display = 'block';
    setTimeout(function() {{ document.getElementById('copied').style.display = 'none' }}, 2000);
  }});
}}
</script>
</body>
</html>"#,
        lark_name = lark_name,
        session_token = session_token,
    )).into_response())
}

// ── POST /v1/auth/lark/refresh ──────────────────────────────────────────────

pub async fn refresh(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Look up refresh token
    let row: Option<(Uuid, String, Uuid)> = sqlx::query_as(
        "SELECT id, refresh_token, org_id FROM lark_tokens WHERE account_id = $1 LIMIT 1",
    )
    .bind(user.account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let (token_row_id, refresh_token, _org_id) = row.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "no lark tokens found for this account"})),
        )
    })?;

    // Call Lark to refresh
    let new_tokens = crate::lark_client::refresh_access_token(
        &state.lark.app_id,
        &state.lark.app_secret,
        &refresh_token,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("token refresh failed: {e}")})),
        )
    })?;

    let expires_at = Utc::now() + Duration::seconds(new_tokens.expires_in);

    // Update stored tokens
    sqlx::query(
        "UPDATE lark_tokens
            SET access_token     = $1,
                refresh_token    = $2,
                token_expires_at = $3,
                updated_at       = now()
          WHERE id = $4",
    )
    .bind(&new_tokens.access_token)
    .bind(&new_tokens.refresh_token)
    .bind(expires_at)
    .bind(token_row_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({
        "ok":         true,
        "expires_at": expires_at.to_rfc3339(),
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
