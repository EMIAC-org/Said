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
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

// ── Query / JWT types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Serialize, Deserialize)]
struct LarkOAuthState {
    mode: String,
    sub: String,
    exp: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    redirect_uri: Option<String>,
}

enum OAuthFlow {
    Admin(Uuid),
    Desktop { redirect_uri: Option<String> },
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String, // account_id
    email: String,
    exp: usize, // expiry timestamp
}

// ── GET /auth/lark (desktop browser entry — public) ─────────────────────────

#[derive(Deserialize)]
pub struct DesktopStartQuery {
    pub callback_port: Option<u16>,
    pub redirect_uri: Option<String>,
}

pub async fn desktop_start(
    State(state): State<AppState>,
    Query(query): Query<DesktopStartQuery>,
) -> Result<axum::response::Redirect, (StatusCode, Json<Value>)> {
    let redirect_uri = resolve_desktop_redirect_uri(query.redirect_uri, query.callback_port);
    let oauth_state = encode_desktop_oauth_state(&state, redirect_uri)?;

    let url = crate::lark_client::build_oauth_url(
        &state.lark.app_id,
        &state.lark.redirect_uri,
        &oauth_state,
    );

    Ok(axum::response::Redirect::temporary(&url))
}

fn resolve_desktop_redirect_uri(
    redirect_uri: Option<String>,
    callback_port: Option<u16>,
) -> Option<String> {
    if let Some(uri) = redirect_uri {
        if is_allowed_local_redirect(&uri) {
            return Some(uri);
        }
    }
    callback_port.map(|port| format!("http://127.0.0.1:{port}/callback"))
}

fn is_allowed_local_redirect(uri: &str) -> bool {
    uri.starts_with("http://127.0.0.1:") || uri.starts_with("http://localhost:")
}

// ── GET /v1/auth/lark/start ─────────────────────────────────────────────────

pub async fn start(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let oauth_state = encode_admin_oauth_state(&state, user.account_id)?;

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
    let oauth_state = decode_oauth_state(&state, &params.state)?;

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
    // Admin OAuth starts from an authenticated AirNote account, so bind Lark to
    // that account. Desktop OAuth has no existing session and still uses the
    // Lark email to create/find an account, then shows the copy-token bridge.
    let lark_email = lark_user
        .email
        .clone()
        .unwrap_or_else(|| format!("{}@lark.user", lark_user.open_id));

    let (account_id, email) = match &oauth_state {
        OAuthFlow::Admin(account_id) => {
            sqlx::query_as("SELECT id, email FROM accounts WHERE id = $1")
                .bind(account_id)
                .fetch_optional(&state.db)
                .await
                .map_err(db_err)?
                .ok_or_else(|| {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"error": "admin OAuth account no longer exists"})),
                    )
                })?
        }
        OAuthFlow::Desktop { .. } => {
            let existing: Option<(Uuid, String)> =
                sqlx::query_as("SELECT id, email FROM accounts WHERE email = $1")
                    .bind(&lark_email)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(db_err)?;

            if let Some(row) = existing {
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
            }
        }
    };

    // ── Find or auto-create org + membership ──────────────────────────────────
    let existing_org_id = resolve_lark_callback_org(&state, account_id, &oauth_state).await?;

    let (member_id, org_id, had_existing_membership) = if let Some(org_id) = existing_org_id {
        let member_id: Uuid =
            sqlx::query_scalar("SELECT id FROM org_members WHERE org_id = $1 AND account_id = $2")
                .bind(org_id)
                .bind(account_id)
                .fetch_one(&state.db)
                .await
                .map_err(db_err)?;
        (member_id, org_id, true)
    } else if matches!(oauth_state, OAuthFlow::Admin(_)) {
        return Ok(admin_oauth_bridge(
            None,
            "/admin/onboarding?lark=needs_org",
            "Create your organization first",
        ));
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
            "INSERT INTO org_members
                (org_id, account_id, role, lark_user_id, lark_name, lark_avatar_url, lark_department, auth_source)
             VALUES ($1, $2, 'COMPANY_ADMIN', $3, $4, $5, $6, 'lark')",
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

        (mid, oid, false)
    };

    // Update Lark profile fields on existing membership
    if had_existing_membership {
        sqlx::query(
            "UPDATE org_members
                SET lark_user_id    = $1,
                    lark_name       = $2,
                    lark_avatar_url = $3,
                    lark_department = $4,
                    auth_source     = 'lark'
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

    sqlx::query("UPDATE accounts SET active_org_id = COALESCE(active_org_id, $1) WHERE id = $2")
        .bind(org_id)
        .bind(account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    // ── Issue session token (for API calls) ────────────────────────────────
    let session_expires = Utc::now() + Duration::days(30);
    let session_token: Uuid = sqlx::query_scalar(
        "INSERT INTO sessions (account_id, expires_at, active_org_id)
         VALUES ($1, $2, $3) RETURNING token",
    )
    .bind(account_id)
    .bind(session_expires)
    .bind(org_id)
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

    if matches!(oauth_state, OAuthFlow::Admin(_)) {
        return Ok(admin_oauth_bridge(
            Some(session_token),
            "/admin/settings?lark=connected",
            "Lark connected",
        ));
    }

    let desktop_redirect_uri = match oauth_state {
        OAuthFlow::Desktop { redirect_uri } => redirect_uri,
        OAuthFlow::Admin(_) => None,
    };

    Ok(desktop_oauth_bridge(
        session_token,
        lark_name,
        &desktop_redirect_uri,
    ))
}

fn desktop_oauth_bridge(
    session_token: Uuid,
    lark_name: &str,
    localhost_redirect: &Option<String>,
) -> Response {
    let deep_link = format!("airnote://auth/callback?token={session_token}");
    let localhost_url = localhost_redirect
        .as_ref()
        .map(|base| format!("{base}?token={session_token}"));
    let localhost_json = localhost_url
        .as_ref()
        .map(|u| serde_json::to_string(u).unwrap_or_else(|_| "null".to_string()))
        .unwrap_or_else(|| "null".to_string());

    axum::response::Html(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>AirNote — Opening app</title>
<style>
  * {{ margin:0; padding:0; box-sizing:border-box }}
  body {{ font-family:Inter,system-ui,sans-serif; background:hsl(240 6% 6%); color:hsl(240 6% 92%); min-height:100vh; display:flex; align-items:center; justify-content:center }}
  .card {{ background:hsl(240 5% 10%); border:1px solid hsl(240 5% 18%); border-radius:20px; padding:40px; max-width:420px; width:calc(100vw - 32px); text-align:center }}
  .spinner {{ width:32px; height:32px; border:3px solid hsl(240 5% 18%); border-top-color:hsl(226 80% 78%); border-radius:999px; margin:0 auto 18px; animation:spin .8s linear infinite }}
  .icon {{ width:48px; height:48px; background:hsl(226 80% 78% / 0.14); border-radius:14px; display:none; align-items:center; justify-content:center; margin:0 auto 20px; color:hsl(226 80% 78%) }}
  h1 {{ font-size:20px; font-weight:600; margin-bottom:6px }}
  .sub {{ font-size:13px; color:hsl(240 4% 58%); margin-bottom:20px; line-height:1.5 }}
  .open-btn {{ background:hsl(226 80% 78%); color:hsl(240 8% 8%); border:none; padding:12px 24px; border-radius:12px; font-size:13px; font-weight:600; cursor:pointer; width:100%; margin-bottom:12px; text-decoration:none; display:block }}
  .open-btn:hover {{ filter:brightness(0.95) }}
  .fallback {{ margin-top:20px; padding-top:20px; border-top:1px solid hsl(240 5% 18%); text-align:left }}
  .fallback summary {{ font-size:12px; color:hsl(240 4% 58%); cursor:pointer; list-style:none }}
  .fallback summary::-webkit-details-marker {{ display:none }}
  .token-box {{ background:hsl(240 6% 6%); border:1px solid hsl(240 5% 18%); border-radius:12px; padding:12px 16px; font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:11px; color:hsl(226 80% 78%); word-break:break-all; margin:12px 0; cursor:pointer }}
  .copy-btn {{ background:hsl(240 5% 16%); color:hsl(226 80% 78%); border:1px solid hsl(240 5% 18%); padding:10px 24px; border-radius:12px; font-size:13px; font-weight:600; cursor:pointer; width:100% }}
  .hint {{ font-size:11px; color:hsl(240 4% 46%); margin-top:8px }}
  .copied {{ color:hsl(145 70% 60%); font-size:12px; font-weight:500; margin-top:8px; display:none }}
  .copied.error {{ color:hsl(38 90% 62%) }}
  @keyframes spin {{ to {{ transform:rotate(360deg) }} }}
</style>
</head>
<body>
<div class="card">
  <div class="spinner" id="spinner"></div>
  <div class="icon" id="icon">✓</div>
  <h1 id="title">Opening AirNote…</h1>
  <p class="sub" id="subtitle">Welcome, {lark_name}! You can close this tab once AirNote opens.</p>
  <a class="open-btn" id="open-app" href="{deep_link}">Open AirNote</a>
  <details class="fallback" id="fallback">
    <summary>AirNote didn&apos;t open? Show manual options</summary>
    <div class="token-box" id="token" onclick="copyToken()">{session_token}</div>
    <button class="copy-btn" onclick="copyToken()">Copy token</button>
    <p class="hint">Paste this token in AirNote if the app did not connect automatically.</p>
    <p class="copied" id="copied"></p>
  </details>
</div>
<script>
var deepLink = {deep_link_json};
var localhostRedirect = {localhost_json};
function openApp() {{
  try {{ window.location.href = deepLink; }} catch (_) {{}}
  try {{
    var a = document.createElement('a');
    a.href = deepLink;
    a.style.display = 'none';
    document.body.appendChild(a);
    a.click();
    a.remove();
  }} catch (_) {{}}
}}
function hitLocalhostCallback() {{
  if (!localhostRedirect) return;
  try {{
    var iframe = document.createElement('iframe');
    iframe.src = localhostRedirect;
    iframe.width = '1';
    iframe.height = '1';
    iframe.style.position = 'fixed';
    iframe.style.left = '-9999px';
    iframe.style.top = '-9999px';
    iframe.style.opacity = '0';
    iframe.setAttribute('aria-hidden', 'true');
    document.body.appendChild(iframe);
    setTimeout(function() {{
      try {{ iframe.remove(); }} catch (_) {{}}
    }}, 8000);
  }} catch (_) {{}}
}}
function showFallback() {{
  document.getElementById('spinner').style.display = 'none';
  document.getElementById('icon').style.display = 'flex';
  document.getElementById('title').textContent = 'Welcome, {lark_name}!';
  document.getElementById('subtitle').textContent = 'AirNote did not open automatically. Click Open AirNote or copy the token below.';
  document.getElementById('fallback').open = true;
}}
hitLocalhostCallback();
openApp();
setTimeout(hitLocalhostCallback, 350);
setTimeout(openApp, 700);
setTimeout(hitLocalhostCallback, 1200);
setTimeout(showFallback, 4000);
document.getElementById('open-app').addEventListener('click', function(e) {{
  e.preventDefault();
  hitLocalhostCallback();
  openApp();
  setTimeout(showFallback, 2500);
}});
function showCopyStatus(message, isError) {{
  var el = document.getElementById('copied');
  el.textContent = message;
  el.className = isError ? 'copied error' : 'copied';
  el.style.display = 'block';
  setTimeout(function() {{ el.style.display = 'none'; }}, 2400);
}}
async function copyToken() {{
  var token = document.getElementById('token').textContent.trim();
  if (navigator.clipboard && window.isSecureContext) {{
    try {{
      await navigator.clipboard.writeText(token);
      showCopyStatus('Copied to clipboard', false);
      return;
    }} catch (_) {{}}
  }}
  var textarea = document.createElement('textarea');
  textarea.value = token;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.left = '-9999px';
  document.body.appendChild(textarea);
  textarea.select();
  try {{
    var copied = document.execCommand('copy');
    textarea.remove();
    if (!copied) throw new Error('copy failed');
    showCopyStatus('Copied to clipboard', false);
  }} catch (_) {{
    textarea.remove();
    showCopyStatus('Press Cmd+C to copy selected token', true);
  }}
}}
</script>
</body>
</html>"#,
        lark_name = lark_name,
        session_token = session_token,
        deep_link = deep_link,
        deep_link_json = serde_json::to_string(&deep_link)
            .unwrap_or_else(|_| format!("\"{deep_link}\"")),
        localhost_json = localhost_json,
    ))
    .into_response()
}

fn encode_desktop_oauth_state(
    state: &AppState,
    redirect_uri: Option<String>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let exp = (Utc::now() + Duration::minutes(10)).timestamp() as usize;
    let claims = LarkOAuthState {
        mode: "desktop".to_string(),
        sub: Uuid::new_v4().to_string(),
        exp,
        redirect_uri,
    };

    let jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "failed to sign OAuth state"})),
        )
    })?;

    Ok(format!("desktop.{jwt}"))
}

fn encode_admin_oauth_state(
    state: &AppState,
    account_id: Uuid,
) -> Result<String, (StatusCode, Json<Value>)> {
    let exp = (Utc::now() + Duration::minutes(10)).timestamp() as usize;
    let claims = LarkOAuthState {
        mode: "admin".to_string(),
        sub: account_id.to_string(),
        exp,
        redirect_uri: None,
    };

    let jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "failed to sign OAuth state"})),
        )
    })?;

    Ok(format!("admin.{jwt}"))
}

fn decode_oauth_state(
    state: &AppState,
    oauth_state: &str,
) -> Result<OAuthFlow, (StatusCode, Json<Value>)> {
    if let Some(jwt) = oauth_state.strip_prefix("admin.") {
        let data = decode::<LarkOAuthState>(
            jwt,
            &DecodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid or expired OAuth state"})),
            )
        })?;

        if data.claims.mode != "admin" {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid OAuth state mode"})),
            ));
        }

        let account_id = Uuid::parse_str(&data.claims.sub).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid OAuth state account"})),
            )
        })?;

        return Ok(OAuthFlow::Admin(account_id));
    }

    if let Some(jwt) = oauth_state.strip_prefix("desktop.") {
        let data = decode::<LarkOAuthState>(
            jwt,
            &DecodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid or expired OAuth state"})),
            )
        })?;

        if data.claims.mode != "desktop" {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid OAuth state mode"})),
            ));
        }

        let redirect_uri = data
            .claims
            .redirect_uri
            .filter(|uri| is_allowed_local_redirect(uri));

        return Ok(OAuthFlow::Desktop { redirect_uri });
    }

    Uuid::parse_str(oauth_state).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid state parameter"})),
        )
    })?;

    Ok(OAuthFlow::Desktop { redirect_uri: None })
}

fn admin_oauth_bridge(session_token: Option<Uuid>, destination: &str, title: &str) -> Response {
    let token_script = session_token
        .map(|token| {
            format!(
                "localStorage.setItem('airnote:admin:token', '{token}');localStorage.removeItem('said:admin:token');"
            )
        })
        .unwrap_or_default();

    axum::response::Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>AirNote Enterprise — {title}</title>
<style>
  * {{ margin:0; padding:0; box-sizing:border-box }}
  body {{ font-family:'Inter',system-ui,sans-serif; background:#080b16; color:#e8eaf0; min-height:100vh; display:flex; align-items:center; justify-content:center }}
  .card {{ background:#0e1225; border:1px solid #1a2038; border-radius:20px; padding:34px; max-width:380px; width:calc(100vw - 32px); text-align:center }}
  .spinner {{ width:28px; height:28px; border:3px solid #26304f; border-top-color:#7591ef; border-radius:999px; margin:0 auto 18px; animation:spin .8s linear infinite }}
  h1 {{ font-size:18px; font-weight:600; margin-bottom:7px }}
  p {{ font-size:13px; color:#8f96b5; line-height:1.45 }}
  a {{ color:#9aaef3 }}
  @keyframes spin {{ to {{ transform:rotate(360deg) }} }}
</style>
</head>
<body>
<div class="card">
  <div class="spinner"></div>
  <h1>{title}</h1>
  <p>Returning to AirNote admin...</p>
  <p style="margin-top:14px"><a href="{destination}">Continue</a></p>
</div>
<script>
{token_script}
window.location.replace('{destination}');
</script>
</body>
</html>"#
    ))
    .into_response()
}

// ── POST /v1/auth/lark/refresh ──────────────────────────────────────────────

pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, refresh_token FROM lark_tokens WHERE account_id = $1 AND org_id = $2",
    )
    .bind(user.account_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let (token_row_id, refresh_token) = row.ok_or_else(|| {
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

async fn resolve_lark_callback_org(
    state: &AppState,
    account_id: Uuid,
    oauth_state: &OAuthFlow,
) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    if let Some(org_id) = tenant::resolve_ws_org_id(state, account_id).await? {
        return Ok(Some(org_id));
    }

    if matches!(oauth_state, OAuthFlow::Admin(_)) {
        return Ok(None);
    }

    let memberships = tenant::list_memberships(state, account_id).await?;
    Ok(memberships.first().map(|m| m.id))
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
