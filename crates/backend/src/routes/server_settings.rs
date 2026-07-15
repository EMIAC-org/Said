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
    AppState, cp_client, invalidate_prefs_cache,
    store::{
        prefs::{PrefsUpdate, update_prefs},
        server_settings, users,
    },
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
    match pull_and_apply_server_settings(&state).await {
        Ok(version) => (
            StatusCode::OK,
            Json(json!({"synced": true, "version": version})),
        ),
        Err((status, reason)) => (status, Json(json!({"synced": false, "reason": reason}))),
    }
}

/// Pull cross-device runtime settings from the control-plane and mirror them
/// into local SQLite prefs. Called from the sync route and after enterprise login.
pub async fn pull_and_apply_server_settings(state: &AppState) -> Result<i64, (StatusCode, String)> {
    let uid = state.default_user_id.to_string();
    let user = users::get_user(&state.pool, &uid);

    let token = user
        .as_ref()
        .and_then(|u| u.cloud_token.as_deref())
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .ok_or((StatusCode::PRECONDITION_FAILED, "not signed in".to_string()))?;

    let server_url = user
        .as_ref()
        .and_then(|u| u.enterprise_server_url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AIRNOTE_CONTROL_PLANE_URL").ok())
        .or_else(|| std::env::var("CLOUD_API_URL").ok())
        .ok_or((
            StatusCode::PRECONDITION_FAILED,
            "server URL not configured".to_string(),
        ))?;

    let server_account_id = user
        .as_ref()
        .map(|u| u.email.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let get_url = format!("{}/v1/runtime/settings", server_url.trim_end_matches('/'));

    let resp = cp_client::with_org_context(
        state
            .http_client
            .get(&get_url)
            .bearer_auth(&token)
            .timeout(std::time::Duration::from_secs(SYNC_TIMEOUT_SECS)),
        user.as_ref(),
    )
    .send()
    .await
    .map_err(|e| {
        let msg = format!("request failed: {e}");
        warn!("[server-settings] {msg}");
        server_settings::set_error(&state.pool, &uid, &server_account_id, &msg);
        (StatusCode::SERVICE_UNAVAILABLE, msg)
    })?;

    if !resp.status().is_success() {
        let msg = format!("server returned {}", resp.status().as_u16());
        warn!("[server-settings] {msg}");
        server_settings::set_error(&state.pool, &uid, &server_account_id, &msg);
        return Err((StatusCode::BAD_GATEWAY, msg));
    }

    let body = resp.json::<Value>().await.map_err(|e| {
        let msg = format!("parse error: {e}");
        warn!("[server-settings] {msg}");
        server_settings::set_error(&state.pool, &uid, &server_account_id, &msg);
        (StatusCode::BAD_GATEWAY, msg)
    })?;

    let version = body.get("version").and_then(Value::as_i64).unwrap_or(1);
    let settings_str = serde_json::to_string(&body).unwrap_or_default();
    server_settings::put(
        &state.pool,
        &uid,
        &server_account_id,
        &settings_str,
        version,
    );

    if let Some(update) = prefs_update_from_server_settings(&body) {
        if update_prefs(&state.pool, &uid, update).is_some() {
            invalidate_prefs_cache(&state.prefs_cache).await;
            info!("[server-settings] applied cross-device prefs from server version={version}");
        }
    }

    info!("[server-settings] synced from server version={version}");
    Ok(version)
}

fn prefs_update_from_server_settings(body: &Value) -> Option<PrefsUpdate> {
    let mut update = PrefsUpdate::default();
    let mut changed = false;

    if let Some(v) = body.get("selected_model").and_then(Value::as_str) {
        update.selected_model = Some(crate::store::prefs::validate_polish_model_key(v));
        changed = true;
    }
    if let Some(v) = body.get("output_language").and_then(Value::as_str) {
        update.output_language = Some(v.to_string());
        changed = true;
    }
    if let Some(v) = body.get("tone_preset").and_then(Value::as_str) {
        update.tone_preset = Some(v.to_string());
        changed = true;
    }
    if body.get("custom_prompt").is_some() {
        update.custom_prompt = Some(
            body.get("custom_prompt")
                .and_then(|v| v.as_str().map(str::to_string)),
        );
        changed = true;
    }
    if let Some(v) = body.get("auto_paste").and_then(Value::as_bool) {
        update.auto_paste = Some(v);
        changed = true;
    }
    if let Some(v) = body.get("edit_capture").and_then(Value::as_bool) {
        update.edit_capture = Some(v);
        changed = true;
    }
    if let Some(v) = body.get("learning_enabled").and_then(Value::as_bool) {
        update.learning_enabled = Some(v);
        changed = true;
    }
    if let Some(v) = body.get("server_runtime_enabled").and_then(Value::as_bool) {
        update.server_runtime_enabled = Some(v);
        changed = true;
    }
    if let Some(v) = body
        .get("server_audio_runtime_enabled")
        .and_then(Value::as_bool)
    {
        update.server_audio_runtime_enabled = Some(v);
        changed = true;
    }

    changed.then_some(update)
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

    if let Err(e) = cp_client::with_org_context(
        state
            .http_client
            .patch(&url)
            .bearer_auth(&token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(8)),
        user.as_ref(),
    )
    .send()
    .await
    {
        warn!("[server-settings] push after prefs patch failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_update_from_server_settings_maps_cross_device_fields() {
        let body = json!({
            "selected_model": "smart",
            "output_language": "english",
            "tone_preset": "professional",
            "custom_prompt": "keep it short",
            "auto_paste": true,
            "edit_capture": false,
            "learning_enabled": true,
            "server_runtime_enabled": true,
            "server_audio_runtime_enabled": false,
            "version": 3
        });

        let update = prefs_update_from_server_settings(&body).expect("update");
        assert_eq!(
            update.selected_model.as_deref(),
            Some("openrouter-gemma-4-nitro")
        );
        assert_eq!(update.output_language.as_deref(), Some("english"));
        assert_eq!(update.tone_preset.as_deref(), Some("professional"));
        assert_eq!(update.custom_prompt, Some(Some("keep it short".into())));
        assert_eq!(update.auto_paste, Some(true));
        assert_eq!(update.edit_capture, Some(false));
        assert_eq!(update.learning_enabled, Some(true));
        assert_eq!(update.server_runtime_enabled, Some(true));
        assert_eq!(update.server_audio_runtime_enabled, Some(false));
    }
}
