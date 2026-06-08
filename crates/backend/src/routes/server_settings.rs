//! Local mirror of server-side runtime settings.
//!
//! Routes:
//!   GET  /v1/server-settings/status  — sync state + cached settings
//!   POST /v1/server-settings/sync    — pull from server and cache locally

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::{
    AppState,
    store::{server_settings, users},
};

const SYNC_TIMEOUT_SECS: u64 = 10;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServerSettingsStatus {
    pub synced: bool,
    pub server_version: i64,
    pub last_synced_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub settings: Option<Value>,
    pub signed_in: bool,
}

// ── GET /v1/server-settings/status ───────────────────────────────────────────

pub async fn status(State(state): State<AppState>) -> Json<ServerSettingsStatus> {
    let uid = state.default_user_id.to_string();
    let user = users::get_user(&state.pool, &uid);

    let signed_in = user
        .as_ref()
        .and_then(|u| u.cloud_token.as_deref())
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);

    let server_account_id = user
        .as_ref()
        .map(|u| u.email.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let cached = server_settings::get(&state.pool, &uid, &server_account_id);
    let settings: Option<Value> = cached
        .as_ref()
        .and_then(|c| serde_json::from_str(&c.settings_json).ok());

    Json(ServerSettingsStatus {
        synced: cached
            .as_ref()
            .map(|c| c.last_error.is_none() && c.server_version > 0)
            .unwrap_or(false),
        server_version: cached.as_ref().map(|c| c.server_version).unwrap_or(0),
        last_synced_at_ms: cached.as_ref().and_then(|c| c.last_synced_at_ms),
        last_error: cached.as_ref().and_then(|c| c.last_error.clone()),
        settings,
        signed_in,
    })
}

// ── POST /v1/server-settings/sync ────────────────────────────────────────────

pub async fn sync(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let uid = state.default_user_id.to_string();
    let user = users::get_user(&state.pool, &uid);

    let token = match user
        .as_ref()
        .and_then(|u| u.cloud_token.as_deref())
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
    {
        Some(t) => t,
        None => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(json!({"synced": false, "reason": "not signed in"})),
            );
        }
    };

    let server_url = user
        .as_ref()
        .and_then(|u| u.enterprise_server_url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AIRNOTE_CONTROL_PLANE_URL").ok())
        .or_else(|| std::env::var("CLOUD_API_URL").ok());

    let base = match server_url {
        Some(u) => u,
        None => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(json!({"synced": false, "reason": "server URL not configured"})),
            );
        }
    };

    let server_account_id = user
        .as_ref()
        .map(|u| u.email.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let get_url = format!("{}/v1/runtime/settings", base.trim_end_matches('/'));

    match state
        .http_client
        .get(&get_url)
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(SYNC_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(body) => {
                let version = body.get("version").and_then(Value::as_i64).unwrap_or(1);
                let settings_str = serde_json::to_string(&body).unwrap_or_default();
                server_settings::put(
                    &state.pool,
                    &uid,
                    &server_account_id,
                    &settings_str,
                    version,
                );
                info!("[server-settings] synced from server version={version}");
                (
                    StatusCode::OK,
                    Json(json!({"synced": true, "version": version})),
                )
            }
            Err(e) => {
                let msg = format!("parse error: {e}");
                warn!("[server-settings] {msg}");
                server_settings::set_error(&state.pool, &uid, &server_account_id, &msg);
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"synced": false, "reason": msg})),
                )
            }
        },
        Ok(resp) => {
            let code = resp.status().as_u16();
            let msg = format!("server returned {code}");
            warn!("[server-settings] {msg}");
            server_settings::set_error(&state.pool, &uid, &server_account_id, &msg);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"synced": false, "reason": msg})),
            )
        }
        Err(e) => {
            let msg = format!("request failed: {e}");
            warn!("[server-settings] {msg}");
            server_settings::set_error(&state.pool, &uid, &server_account_id, &msg);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"synced": false, "reason": msg})),
            )
        }
    }
}

// ── Cross-device push (called from prefs PATCH for server-owned fields) ───────

/// Fire-and-forget push of cross-device prefs to the server settings endpoint.
/// Never panics; all errors are logged and silently dropped.
pub async fn push_cross_device_settings_to_server(
    state: AppState,
    selected_model: String,
    output_language: String,
    tone_preset: String,
    custom_prompt: Option<String>,
    auto_paste: bool,
    edit_capture: bool,
    learning_enabled: bool,
    server_runtime_enabled: bool,
    server_audio_runtime_enabled: bool,
) {
    let uid = state.default_user_id.to_string();
    let user = users::get_user(&state.pool, &uid);

    let token = match user
        .as_ref()
        .and_then(|u| u.cloud_token.as_deref())
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
    {
        Some(t) => t,
        None => return,
    };

    let base = match user
        .as_ref()
        .and_then(|u| u.enterprise_server_url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AIRNOTE_CONTROL_PLANE_URL").ok())
        .or_else(|| std::env::var("CLOUD_API_URL").ok())
    {
        Some(u) => u,
        None => return,
    };

    let url = format!("{}/v1/runtime/settings", base.trim_end_matches('/'));
    let body = json!({
        "selected_model":               selected_model,
        "output_language":              output_language,
        "tone_preset":                  tone_preset,
        "custom_prompt":                custom_prompt,
        "auto_paste":                   auto_paste,
        "edit_capture":                 edit_capture,
        "learning_enabled":             learning_enabled,
        "server_runtime_enabled":       server_runtime_enabled,
        "server_audio_runtime_enabled": server_audio_runtime_enabled,
    });

    if let Err(e) = state
        .http_client
        .patch(&url)
        .bearer_auth(&token)
        .json(&body)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
    {
        warn!("[server-settings] push after prefs patch failed: {e}");
    }
}
