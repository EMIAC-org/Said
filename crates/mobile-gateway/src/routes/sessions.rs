//! Voice session creation.
//!
//!   POST /v1/runtime/sessions  (and /v1/mobile/sessions)
//!
//! Creates a short-lived voice session and returns the streaming + batch URLs
//! and a session token. Cursor/keyboard context is reduced to redacted counts —
//! raw text is never stored.

use axum::{Json, extract::State};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, runtime, util::*};

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub client_request_id: String,
    pub device_id: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    pub language_hint: String,
    pub style: String,
    #[serde(default)]
    pub keyboard_context: Option<KeyboardContext>,
    #[serde(default)]
    pub target_app: Option<TargetApp>,
    #[serde(default)]
    pub cursor_context: Option<CursorContext>,
    #[serde(default)]
    pub vocab_snapshot_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KeyboardContext {
    #[serde(default)]
    pub before_text: String,
    #[serde(default)]
    pub after_text: String,
    #[serde(default)]
    pub selected_text: String,
    #[serde(default)]
    pub host_app_label: String,
    #[serde(default)]
    pub field_hint: String,
}

#[derive(Debug, Deserialize)]
pub struct TargetApp {
    pub label: String,
    #[serde(default)]
    pub bundle_id: Option<String>,
    pub field_hint: String,
}

#[derive(Debug, Deserialize)]
pub struct CursorContext {
    #[serde(default)]
    pub before_text: String,
    #[serde(default)]
    pub after_text: String,
    #[serde(default)]
    pub selected_text: String,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub schema: &'static str,
    pub session_id: Uuid,
    pub session_token: Uuid,
    pub expires_at: DateTime<Utc>,
    pub streaming_enabled: bool,
    pub current_vocab_hash: String,
    pub voice_ws_url: String,
    pub batch_url: String,
    pub max_recording_seconds: i32,
}

pub async fn create_session(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateSessionRequest>,
) -> ApiResult<Json<CreateSessionResponse>> {
    let device_id = clean_required(&body.device_id, MAX_DEVICE_ID_LEN, "device_id")?;
    let client_request_id = clean_required(
        &body.client_request_id,
        MAX_CLIENT_REQUEST_ID_LEN,
        "client_request_id",
    )?;
    let platform = normalize_choice(body.platform.as_deref(), PLATFORMS, "ios");
    let surface = normalize_choice(body.surface.as_deref(), SURFACES, "ios_keyboard");
    let language_hint = normalize_choice(Some(&body.language_hint), LANGUAGES, "auto");
    let style = normalize_choice(Some(&body.style), STYLES, "work");
    let current_vocab_hash = runtime::vocab::current_hash(&state.db, user.account_id).await;
    let context_json = redacted_session_context(&body)?;
    let expires_at = Utc::now() + Duration::minutes(SESSION_TTL_MINUTES);

    // Best-effort device registration / last-seen update.
    let _ = sqlx::query(
        "INSERT INTO mobile_devices (account_id, device_id, platform)
         VALUES ($1, $2, $3)
         ON CONFLICT (account_id, device_id)
         DO UPDATE SET last_seen_at = now(), platform = EXCLUDED.platform",
    )
    .bind(user.account_id)
    .bind(&device_id)
    .bind(&platform)
    .execute(&state.db)
    .await;

    let (session_id, session_token, expires_at, current_vocab_hash): (
        Uuid,
        Uuid,
        DateTime<Utc>,
        String,
    ) = sqlx::query_as(
        "INSERT INTO voice_sessions (
            account_id, device_id, client_request_id, platform, surface,
            language_hint, style, context_json, vocab_snapshot_hash,
            current_vocab_hash, streaming_enabled, max_recording_seconds, expires_at, status
         )
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,true,$11,$12,'created')
         ON CONFLICT (account_id, device_id, client_request_id) DO UPDATE
           SET platform = EXCLUDED.platform,
               surface = EXCLUDED.surface,
               language_hint = EXCLUDED.language_hint,
               style = EXCLUDED.style,
               context_json = EXCLUDED.context_json,
               vocab_snapshot_hash = EXCLUDED.vocab_snapshot_hash,
               current_vocab_hash = EXCLUDED.current_vocab_hash,
               streaming_enabled = true,
               max_recording_seconds = EXCLUDED.max_recording_seconds,
               expires_at = EXCLUDED.expires_at,
               status = 'created',
               completed_at = NULL
         RETURNING id, session_token, expires_at, current_vocab_hash",
    )
    .bind(user.account_id)
    .bind(&device_id)
    .bind(&client_request_id)
    .bind(&platform)
    .bind(&surface)
    .bind(&language_hint)
    .bind(&style)
    .bind(context_json)
    .bind(trim_optional(body.vocab_snapshot_hash.clone(), 128))
    .bind(&current_vocab_hash)
    .bind(MAX_RECORDING_SECONDS)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(CreateSessionResponse {
        schema: "airnote.runtime.session.v1",
        session_id,
        session_token,
        expires_at,
        streaming_enabled: true,
        current_vocab_hash,
        voice_ws_url: format!(
            "/v1/runtime/voice?session_id={session_id}&session_token={session_token}"
        ),
        batch_url: "/v1/runtime/voice/batch".into(),
        max_recording_seconds: MAX_RECORDING_SECONDS,
    }))
}

/// Reduce client context to non-identifying counts + app label/field hint. Raw
/// before/after/selected text is counted, never stored.
fn redacted_session_context(body: &CreateSessionRequest) -> ApiResult<Value> {
    let keyboard = body.keyboard_context.as_ref();
    let target = body.target_app.as_ref();
    let cursor = body.cursor_context.as_ref();

    let host_app_label = keyboard
        .map(|ctx| ctx.host_app_label.as_str())
        .or_else(|| target.map(|ctx| ctx.label.as_str()))
        .map(|raw| clean_optional(raw, MAX_LABEL_LEN))
        .transpose()?;
    let field_hint = keyboard
        .map(|ctx| ctx.field_hint.as_str())
        .or_else(|| target.map(|ctx| ctx.field_hint.as_str()))
        .map(|raw| clean_optional(raw, MAX_FIELD_HINT_LEN))
        .transpose()?;

    let before_text_chars = keyboard
        .map(|ctx| bounded_char_count(&ctx.before_text))
        .or_else(|| cursor.map(|ctx| bounded_char_count(&ctx.before_text)))
        .unwrap_or(0);
    let after_text_chars = keyboard
        .map(|ctx| bounded_char_count(&ctx.after_text))
        .or_else(|| cursor.map(|ctx| bounded_char_count(&ctx.after_text)))
        .unwrap_or(0);
    let selected_text_chars = keyboard
        .map(|ctx| bounded_char_count(&ctx.selected_text))
        .or_else(|| cursor.map(|ctx| bounded_char_count(&ctx.selected_text)))
        .unwrap_or(0);

    Ok(json!({
        "host_app_label": host_app_label,
        "field_hint": field_hint,
        "target_bundle_known": target.and_then(|ctx| ctx.bundle_id.as_ref()).is_some(),
        "before_text_chars": before_text_chars,
        "after_text_chars": after_text_chars,
        "selected_text_chars": selected_text_chars
    }))
}
