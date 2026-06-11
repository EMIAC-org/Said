//! Guest browser join routes.
//!
//! Hosts can mint a short invite URL for a scheduled/live meeting. Guests use
//! that public URL to create a temporary guest account and receive a meeting-
//! scoped WebSocket JWT for browser audio capture.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use rand::{Rng, distributions::Alphanumeric};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

const GUEST_JOIN_HTML: &str = include_str!("../../admin/guest-join.html");

#[derive(Deserialize)]
pub struct GuestAuthBody {
    pub display_name: String,
}

#[derive(Serialize)]
struct GuestClaims {
    sub: String,
    email: String,
    meeting_id: String,
    display_name: String,
    is_guest: bool,
    exp: usize,
}

struct InviteInfo {
    meeting_id: Uuid,
    meeting_title: String,
    meeting_status: String,
    host_name: String,
    expires_at: Option<DateTime<Utc>>,
    max_uses: Option<i32>,
    use_count: i32,
}

// ── POST /v1/meetings/:id/guest-link ───────────────────────────────────────

pub async fn create_guest_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let host_ok: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1
              FROM meetings m
              JOIN meeting_participants mp
                ON mp.meeting_id = m.id AND mp.account_id = $3
             WHERE m.id = $1
               AND m.org_id = $2
               AND m.created_by = $3
               AND m.status IN ('scheduled', 'live')
        )",
    )
    .bind(meeting_id)
    .bind(org_id)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    if !host_ok {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "only the meeting host can create a guest link"})),
        ));
    }

    let token = random_token();
    let expires_at = Utc::now() + Duration::hours(24);

    sqlx::query(
        "INSERT INTO guest_invites (meeting_id, token, created_by, expires_at, max_uses)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(meeting_id)
    .bind(&token)
    .bind(user.account_id)
    .bind(expires_at)
    .bind(25_i32)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({
        "guest_link": format!("/join/{token}"),
        "token": token,
        "expires_at": expires_at,
        "max_uses": 25,
    })))
}

// ── GET /join/:token ────────────────────────────────────────────────────────

pub async fn guest_page(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, impl IntoResponse)> {
    let invite = load_invite(&state, &token)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("database error".to_string()),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Html("guest link not found".to_string()),
            )
        })?;

    if !invite_is_usable(&invite) {
        return Err((
            StatusCode::GONE,
            Html("guest link expired or meeting ended".to_string()),
        ));
    }

    let metadata = json!({
        "token": token,
        "meetingId": invite.meeting_id,
        "meetingTitle": invite.meeting_title,
        "hostName": invite.host_name,
    });
    let metadata_json = serde_json::to_string(&metadata).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("failed to render guest page".to_string()),
        )
    })?;

    let html = GUEST_JOIN_HTML.replace("__AIRNOTE_GUEST_DATA__", &metadata_json);
    Ok(Html(html))
}

// ── POST /join/:token/auth ─────────────────────────────────────────────────

pub async fn guest_auth(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<GuestAuthBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let display_name = normalize_display_name(&body.display_name).ok_or_else(|| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "display_name is required"})),
        )
    })?;

    let invite = load_invite(&state, &token)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "guest link not found"})),
            )
        })?;

    if !invite_is_usable(&invite) {
        return Err((
            StatusCode::GONE,
            Json(json!({"error": "guest link expired or meeting ended"})),
        ));
    }

    // Deduplicate: reuse an existing guest account for the same name+meeting rather than
    // creating a new one on every auth call (browser retries, page reloads, etc.).
    let email_prefix = guest_local_name(&display_name);
    let existing: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT a.id, a.email
           FROM accounts a
           JOIN meeting_participants mp ON mp.account_id = a.id
          WHERE mp.meeting_id = $1
            AND a.is_guest = true
            AND (a.email LIKE $2 || '-%@airnote.guest' OR a.email LIKE $2 || '-%@said.guest')
          LIMIT 1",
    )
    .bind(invite.meeting_id)
    .bind(&email_prefix)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let (guest_id, email) = if let Some((existing_id, existing_email)) = existing {
        (existing_id, existing_email)
    } else {
        let guest_id = Uuid::new_v4();
        let email = guest_email(&display_name, guest_id);

        sqlx::query(
            "INSERT INTO accounts (id, email, password_hash, is_guest) VALUES ($1, $2, '', true)",
        )
        .bind(guest_id)
        .bind(&email)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

        sqlx::query(
            "INSERT INTO meeting_participants (meeting_id, account_id, status)
             VALUES ($1, $2, 'invited')
             ON CONFLICT (meeting_id, account_id) DO NOTHING",
        )
        .bind(invite.meeting_id)
        .bind(guest_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

        sqlx::query("UPDATE guest_invites SET use_count = use_count + 1 WHERE token = $1")
            .bind(&token)
            .execute(&state.db)
            .await
            .map_err(db_err)?;

        (guest_id, email)
    };

    let exp = (Utc::now() + Duration::hours(24)).timestamp() as usize;
    let claims = GuestClaims {
        sub: guest_id.to_string(),
        email,
        meeting_id: invite.meeting_id.to_string(),
        display_name,
        is_guest: true,
        exp,
    };
    let ws_token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.lark.jwt_secret.as_bytes()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "failed to sign guest token"})),
        )
    })?;

    Ok(Json(json!({
        "ws_token": ws_token,
        "guest_id": guest_id,
        "meeting_id": invite.meeting_id,
    })))
}

async fn load_invite(state: &AppState, token: &str) -> Result<Option<InviteInfo>, sqlx::Error> {
    let row: Option<(
        Uuid,
        String,
        String,
        String,
        Option<DateTime<Utc>>,
        Option<i32>,
        i32,
    )> = sqlx::query_as(
        "SELECT gi.meeting_id,
                m.title,
                m.status,
                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)),
                gi.expires_at,
                gi.max_uses,
                gi.use_count
           FROM guest_invites gi
           JOIN meetings m ON m.id = gi.meeting_id
           JOIN accounts a ON a.id = m.created_by
           LEFT JOIN org_members om ON om.account_id = m.created_by AND om.org_id = m.org_id
          WHERE gi.token = $1",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await?;

    Ok(row.map(
        |(
            meeting_id,
            meeting_title,
            meeting_status,
            host_name,
            expires_at,
            max_uses,
            use_count,
        )| InviteInfo {
            meeting_id,
            meeting_title,
            meeting_status,
            host_name,
            expires_at,
            max_uses,
            use_count,
        },
    ))
}

fn invite_is_usable(invite: &InviteInfo) -> bool {
    if invite.meeting_status == "ended" {
        return false;
    }
    if invite
        .expires_at
        .is_some_and(|expires_at| expires_at <= Utc::now())
    {
        return false;
    }
    if invite.max_uses.is_some_and(|max| invite.use_count >= max) {
        return false;
    }
    true
}

fn random_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect()
}

fn normalize_display_name(input: &str) -> Option<String> {
    let name = input.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.chars().take(80).collect())
    }
}

fn guest_local_name(display_name: &str) -> String {
    let local: String = display_name
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_whitespace() || ch == '-' || ch == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(24)
        .collect();
    if local.is_empty() {
        "guest".to_string()
    } else {
        local
    }
}

fn guest_email(display_name: &str, guest_id: Uuid) -> String {
    format!(
        "{}-{}@airnote.guest",
        guest_local_name(display_name),
        guest_id.simple()
    )
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
