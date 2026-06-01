//! Divo proxy — bridges AirNote desktop clients to the Divo agent backend.
//!
//!   POST /v1/divo/chat            — stream an instruction to Divo (SSE pass-through)
//!   GET  /v1/divo/threads/:id     — fetch a thread (recovery / post-approval answer)
//!
//! AirNote authenticates with its own control-plane session token (the `AuthUser`
//! extractor). The desktop never holds a Lark credential. On each call we look up
//! this account's stored Lark `user_access_token` (from `lark_tokens`, refreshing
//! it when it is near expiry), and forward the request to Divo with
//! `Authorization: Bearer <lark_user_access_token>` — exactly the shape Divo's API
//! expects. The powerful Lark credential therefore never leaves the server.
//!
//! The chat endpoint is a transparent SSE pipe: we stream Divo's frames straight
//! back to the desktop as they arrive (no buffering), so the HUD updates live.

use axum::{
    Json,
    body::Body,
    extract::{Path, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

// ── Error helpers ─────────────────────────────────────────────────────────────

fn db_err_resp() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
        .into_response()
}

/// The AirNote account has no Lark identity at all (e.g. email-only signup, or
/// never completed Lark OAuth). We can't speak to Divo on their behalf.
fn not_linked_resp() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "This account isn't linked to Lark. Sign in with Lark to use Divo."
        })),
    )
        .into_response()
}

// ── Lark token resolution (with refresh) ────────────────────────────────────────

/// Resolve this account's current Lark `user_access_token`, refreshing it when it
/// is within 60s of expiry (or always when `force_refresh` is set — used to retry
/// once after Divo rejects a token with 401).
async fn get_lark_token(
    state: &AppState,
    account_id: Uuid,
    force_refresh: bool,
) -> Result<String, Response> {
    let row: Option<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, access_token, refresh_token, token_expires_at
           FROM lark_tokens
          WHERE account_id = $1
          LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| db_err_resp())?;

    let (row_id, access_token, refresh_token, expires_at) = row.ok_or_else(not_linked_resp)?;

    if !force_refresh && expires_at > Utc::now() + Duration::seconds(60) {
        return Ok(access_token);
    }

    let new_tokens = crate::lark_client::refresh_access_token(
        &state.lark.app_id,
        &state.lark.app_secret,
        &refresh_token,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("lark token refresh failed: {e}")})),
        )
            .into_response()
    })?;

    let new_expires = Utc::now() + Duration::seconds(new_tokens.expires_in);
    let _ = sqlx::query(
        "UPDATE lark_tokens
            SET access_token     = $1,
                refresh_token    = $2,
                token_expires_at = $3,
                updated_at       = now()
          WHERE id = $4",
    )
    .bind(&new_tokens.access_token)
    .bind(&new_tokens.refresh_token)
    .bind(new_expires)
    .bind(row_id)
    .execute(&state.db)
    .await;

    Ok(new_tokens.access_token)
}

fn divo_base(state: &AppState) -> String {
    state.divo_base_url.trim_end_matches('/').to_string()
}

// ── POST /v1/divo/chat ──────────────────────────────────────────────────────────

/// Forward an instruction to Divo and stream the SSE response straight back.
/// The request body (`{requestId, message, threadId?, mode?}`) is passed through
/// opaquely — we don't model it, Divo owns that contract.
pub async fn chat(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Value>,
) -> Response {
    let client = reqwest::Client::new();
    let url = format!("{}/api/airnote/chat", divo_base(&state));

    // First attempt with the proactively-refreshed token.
    let mut token = match get_lark_token(&state, user.account_id, false).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let mut upstream = match post_chat(&client, &url, &token, &body).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Spec §2: on 401 the Lark token is stale/invalid — force a refresh and retry once.
    if upstream.status() == reqwest::StatusCode::UNAUTHORIZED {
        token = match get_lark_token(&state, user.account_id, true).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        upstream = match post_chat(&client, &url, &token, &body).await {
            Ok(r) => r,
            Err(resp) => return resp,
        };
    }

    let status = upstream.status();
    if !status.is_success() {
        return forward_error(status, upstream).await;
    }

    // Success — pipe the SSE byte stream through unbuffered. `X-Accel-Buffering: no`
    // stops an nginx reverse proxy from buffering the stream (which would defeat
    // live delivery — frames would arrive only after the whole run finished).
    let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    (
        code,
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
            ("x-accel-buffering", "no"),
        ],
        Body::from_stream(upstream.bytes_stream()),
    )
        .into_response()
}

async fn post_chat(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    body: &Value,
) -> Result<reqwest::Response, Response> {
    client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("divo unreachable: {e}")})),
            )
                .into_response()
        })
}

// ── GET /v1/divo/threads/:id ─────────────────────────────────────────────────────

/// Fetch a Divo thread (used to recover the answer after the SSE dropped, or to
/// pick up a post-approval result). Forwards the JSON response verbatim.
pub async fn thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let client = reqwest::Client::new();
    let base = divo_base(&state);
    let url = match query {
        Some(q) if !q.is_empty() => format!("{base}/api/airnote/threads/{thread_id}?{q}"),
        _ => format!("{base}/api/airnote/threads/{thread_id}"),
    };

    let token = match get_lark_token(&state, user.account_id, false).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let mut upstream = match get_thread(&client, &url, &token).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    if upstream.status() == reqwest::StatusCode::UNAUTHORIZED {
        let token = match get_lark_token(&state, user.account_id, true).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        upstream = match get_thread(&client, &url, &token).await {
            Ok(r) => r,
            Err(resp) => return resp,
        };
    }

    let status = upstream.status();
    let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = upstream.text().await.unwrap_or_default();
    (code, [(header::CONTENT_TYPE, "application/json")], text).into_response()
}

async fn get_thread(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<reqwest::Response, Response> {
    client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("divo unreachable: {e}")})),
            )
                .into_response()
        })
}

/// Forward a non-2xx upstream response (its status + JSON body) verbatim, so the
/// desktop sees Divo's own 400/403 messages (e.g. the "open Divo in Lark once" 403).
async fn forward_error(status: reqwest::StatusCode, upstream: reqwest::Response) -> Response {
    let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = upstream.text().await.unwrap_or_default();
    let body = if text.trim_start().starts_with('{') {
        text
    } else {
        json!({"error": if text.is_empty() { "divo request failed".to_string() } else { text }})
            .to_string()
    };
    (code, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}
