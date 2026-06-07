//! Privacy-safe client event ingestion.
//!
//!   POST /v1/runtime/events  (and /v1/mobile/events)
//!
//! Idempotent on `(account_id, client_event_id)`. Context is sanitized server
//! side as defence-in-depth — no transcript/audio/secret keys are ever stored.

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, util::*};

#[derive(Debug, Deserialize)]
pub struct EventBody {
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub occurred_at: Option<DateTime<Utc>>,
    pub device_id: String,
    #[serde(default)]
    pub session_id: Option<Uuid>,
    #[serde(default)]
    pub client_request_id: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    pub event_type: String,
    #[serde(default)]
    pub redacted_context: Option<Value>,
}

pub async fn ingest_event(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<EventBody>,
) -> ApiResult<Json<Value>> {
    let device_id = clean_required(&body.device_id, MAX_DEVICE_ID_LEN, "device_id")?;
    let event_type = clean_required(&body.event_type, MAX_EVENT_TYPE_LEN, "event_type")?;
    let client_event_id = trim_optional(body.event_id, MAX_EVENT_ID_LEN);
    let client_request_id = trim_optional(body.client_request_id, MAX_CLIENT_REQUEST_ID_LEN);
    let build = trim_optional(body.build, 80);
    let platform = normalize_choice(body.platform.as_deref(), PLATFORMS, "ios");
    let surface = normalize_choice(body.surface.as_deref(), SURFACES, "ios_keyboard");
    let redacted_context = sanitize_context(body.redacted_context.unwrap_or_else(|| json!({})));

    if let Some(session_id) = body.session_id {
        let found: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM voice_sessions WHERE id = $1 AND account_id = $2",
        )
        .bind(session_id)
        .bind(user.account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?;
        if found.is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "voice session not found"})),
            ));
        }
    }

    sqlx::query(
        "INSERT INTO voice_events (
            session_id, account_id, device_id, client_event_id, client_request_id,
            build, platform, surface, event_type, redacted_context, occurred_at
         )
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
         ON CONFLICT (account_id, client_event_id) DO NOTHING",
    )
    .bind(body.session_id)
    .bind(user.account_id)
    .bind(&device_id)
    .bind(client_event_id)
    .bind(client_request_id)
    .bind(build)
    .bind(&platform)
    .bind(&surface)
    .bind(&event_type)
    .bind(redacted_context)
    .bind(body.occurred_at)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({ "ok": true, "accepted": 1 })))
}
