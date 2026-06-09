//! Cross-device runtime settings — per-user behaviour prefs stored server-side.
//!
//! Routes:
//!   GET   /v1/runtime/settings        — settings + credential summaries (never raw secrets)
//!   PATCH /v1/runtime/settings        — partial update, increments version, writes audit row
//!   POST  /v1/runtime/settings/sync   — first-launch merge: newer updated_at wins

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

// ── DB row ────────────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
struct SettingsRow {
    selected_model: String,
    output_language: String,
    tone_preset: String,
    custom_prompt: Option<String>,
    auto_paste: bool,
    edit_capture: bool,
    learning_enabled: bool,
    server_runtime_enabled: bool,
    server_audio_runtime_enabled: bool,
    message_polish_mode: bool,
    notification_prefs_json: Value,
    privacy_prefs_json: Value,
    version: i64,
    updated_at: DateTime<Utc>,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CredentialInfo {
    pub id: Uuid,
    pub provider: String,
    pub display_name: String,
    pub secret_last4: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct SettingsResponse {
    pub selected_model: String,
    pub output_language: String,
    pub tone_preset: String,
    pub custom_prompt: Option<String>,
    pub auto_paste: bool,
    pub edit_capture: bool,
    pub learning_enabled: bool,
    pub server_runtime_enabled: bool,
    pub server_audio_runtime_enabled: bool,
    pub message_polish_mode: bool,
    pub notification_prefs: Value,
    pub privacy_prefs: Value,
    pub version: i64,
    pub updated_at: DateTime<Utc>,
    pub credentials: Vec<CredentialInfo>,
}

#[derive(Debug, Serialize)]
pub struct SyncSettingsResponse {
    pub action: String,
    pub settings: SettingsResponse,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PatchSettingsRequest {
    pub selected_model: Option<String>,
    pub output_language: Option<String>,
    pub tone_preset: Option<String>,
    /// `null` = clear the custom prompt; a string = set it; absent = no change.
    pub custom_prompt: Option<Value>,
    pub auto_paste: Option<bool>,
    pub edit_capture: Option<bool>,
    pub learning_enabled: Option<bool>,
    pub server_runtime_enabled: Option<bool>,
    pub server_audio_runtime_enabled: Option<bool>,
    pub message_polish_mode: Option<bool>,
    pub notification_prefs: Option<Value>,
    pub privacy_prefs: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct SyncSettingsRequest {
    pub settings: PatchSettingsRequest,
    pub local_updated_at: Option<DateTime<Utc>>,
    pub source: Option<String>,
}

// ── GET /v1/runtime/settings ──────────────────────────────────────────────────

pub async fn get_settings(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<SettingsResponse>, (StatusCode, Json<Value>)> {
    let row = ensure_settings(&state, user.account_id).await?;
    let creds = load_cred_infos(&state, user.account_id).await;
    Ok(Json(into_response(row, creds)))
}

// ── PATCH /v1/runtime/settings ────────────────────────────────────────────────

pub async fn patch_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<PatchSettingsRequest>,
) -> Result<Json<SettingsResponse>, (StatusCode, Json<Value>)> {
    validate_patch(&req)?;
    let current = ensure_settings(&state, user.account_id).await?;
    let updated = apply_and_write(&state, user.account_id, req, &current, "desktop").await?;
    let creds = load_cred_infos(&state, user.account_id).await;
    Ok(Json(into_response(updated, creds)))
}

// ── POST /v1/runtime/settings/sync ───────────────────────────────────────────

pub async fn sync_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<SyncSettingsRequest>,
) -> Result<Json<SyncSettingsResponse>, (StatusCode, Json<Value>)> {
    validate_patch(&req.settings)?;

    let source = req.source.as_deref().unwrap_or("migration");
    let source = if ["desktop", "mobile", "admin", "migration"].contains(&source) {
        source
    } else {
        "migration"
    };

    let existing = sqlx::query_as::<_, SettingsRow>(
        "SELECT selected_model, output_language, tone_preset, custom_prompt,
                auto_paste, edit_capture, learning_enabled, server_runtime_enabled,
                server_audio_runtime_enabled, message_polish_mode,
                notification_prefs_json, privacy_prefs_json, version, updated_at
           FROM runtime_user_settings WHERE account_id = $1",
    )
    .bind(user.account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let (action, row) = match (existing, req.local_updated_at) {
        (None, _) => {
            let current = ensure_settings(&state, user.account_id).await?;
            let row =
                apply_and_write(&state, user.account_id, req.settings, &current, source).await?;
            ("created", row)
        }
        (Some(current), Some(local_ts)) if local_ts > current.updated_at => {
            let row =
                apply_and_write(&state, user.account_id, req.settings, &current, source).await?;
            ("updated", row)
        }
        (Some(current), _) => ("no_change", current),
    };

    Ok(Json(SyncSettingsResponse {
        action: action.to_string(),
        settings: into_response(row, vec![]),
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn validate_patch(req: &PatchSettingsRequest) -> Result<(), (StatusCode, Json<Value>)> {
    if let Some(m) = &req.selected_model {
        if !["fast", "smart"].contains(&m.as_str()) {
            return Err(json_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "selected_model must be 'fast' or 'smart'",
            ));
        }
    }
    if let Some(l) = &req.output_language {
        if !["hinglish", "english"].contains(&l.as_str()) {
            return Err(json_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "output_language must be 'hinglish' or 'english'",
            ));
        }
    }
    Ok(())
}

async fn ensure_settings(
    state: &AppState,
    account_id: Uuid,
) -> Result<SettingsRow, (StatusCode, Json<Value>)> {
    sqlx::query(
        "INSERT INTO runtime_user_settings (account_id) VALUES ($1)
         ON CONFLICT (account_id) DO NOTHING",
    )
    .bind(account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    sqlx::query_as::<_, SettingsRow>(
        "SELECT selected_model, output_language, tone_preset, custom_prompt,
                auto_paste, edit_capture, learning_enabled, server_runtime_enabled,
                server_audio_runtime_enabled, message_polish_mode,
                notification_prefs_json, privacy_prefs_json, version, updated_at
           FROM runtime_user_settings WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)
}

async fn apply_and_write(
    state: &AppState,
    account_id: Uuid,
    req: PatchSettingsRequest,
    current: &SettingsRow,
    source: &str,
) -> Result<SettingsRow, (StatusCode, Json<Value>)> {
    let had_model = req.selected_model.is_some();
    let had_lang = req.output_language.is_some();
    let had_tone = req.tone_preset.is_some();
    let had_prompt = req.custom_prompt.is_some();
    let had_paste = req.auto_paste.is_some();
    let had_capture = req.edit_capture.is_some();
    let had_learn = req.learning_enabled.is_some();
    let had_srv = req.server_runtime_enabled.is_some();
    let had_audio = req.server_audio_runtime_enabled.is_some();
    let had_msg = req.message_polish_mode.is_some();
    let had_notif = req.notification_prefs.is_some();
    let had_privacy = req.privacy_prefs.is_some();

    let selected_model = req
        .selected_model
        .unwrap_or_else(|| current.selected_model.clone());
    let output_language = req
        .output_language
        .unwrap_or_else(|| current.output_language.clone());
    let tone_preset = req
        .tone_preset
        .unwrap_or_else(|| current.tone_preset.clone());
    let custom_prompt: Option<String> = match req.custom_prompt {
        None => current.custom_prompt.clone(),
        Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.chars().take(2000).collect()),
        Some(_) => current.custom_prompt.clone(),
    };
    let auto_paste = req.auto_paste.unwrap_or(current.auto_paste);
    let edit_capture = req.edit_capture.unwrap_or(current.edit_capture);
    let learning_enabled = req.learning_enabled.unwrap_or(current.learning_enabled);
    let server_runtime_enabled = req
        .server_runtime_enabled
        .unwrap_or(current.server_runtime_enabled);
    let server_audio_runtime_enabled = req
        .server_audio_runtime_enabled
        .unwrap_or(current.server_audio_runtime_enabled);
    let message_polish_mode = req
        .message_polish_mode
        .unwrap_or(current.message_polish_mode);
    let notification_prefs = req
        .notification_prefs
        .unwrap_or_else(|| current.notification_prefs_json.clone());
    let privacy_prefs = req
        .privacy_prefs
        .unwrap_or_else(|| current.privacy_prefs_json.clone());

    let row = sqlx::query_as::<_, SettingsRow>(
        "UPDATE runtime_user_settings
            SET selected_model               = $2,
                output_language              = $3,
                tone_preset                  = $4,
                custom_prompt                = $5,
                auto_paste                   = $6,
                edit_capture                 = $7,
                learning_enabled             = $8,
                server_runtime_enabled       = $9,
                server_audio_runtime_enabled = $10,
                message_polish_mode          = $11,
                notification_prefs_json      = $12,
                privacy_prefs_json           = $13,
                version                      = version + 1,
                updated_at                   = now()
          WHERE account_id = $1
          RETURNING selected_model, output_language, tone_preset, custom_prompt,
                    auto_paste, edit_capture, learning_enabled, server_runtime_enabled,
                    server_audio_runtime_enabled, message_polish_mode,
                    notification_prefs_json, privacy_prefs_json, version, updated_at",
    )
    .bind(account_id)
    .bind(&selected_model)
    .bind(&output_language)
    .bind(&tone_preset)
    .bind(&custom_prompt)
    .bind(auto_paste)
    .bind(edit_capture)
    .bind(learning_enabled)
    .bind(server_runtime_enabled)
    .bind(server_audio_runtime_enabled)
    .bind(message_polish_mode)
    .bind(&notification_prefs)
    .bind(&privacy_prefs)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    // Audit: only record fields that were explicitly provided in the request.
    let mut changed = serde_json::Map::new();
    if had_model {
        changed.insert("selected_model".to_string(), json!(&selected_model));
    }
    if had_lang {
        changed.insert("output_language".to_string(), json!(&output_language));
    }
    if had_tone {
        changed.insert("tone_preset".to_string(), json!(&tone_preset));
    }
    if had_prompt {
        changed.insert("custom_prompt".to_string(), json!(&custom_prompt));
    }
    if had_paste {
        changed.insert("auto_paste".to_string(), json!(auto_paste));
    }
    if had_capture {
        changed.insert("edit_capture".to_string(), json!(edit_capture));
    }
    if had_learn {
        changed.insert("learning_enabled".to_string(), json!(learning_enabled));
    }
    if had_srv {
        changed.insert(
            "server_runtime_enabled".to_string(),
            json!(server_runtime_enabled),
        );
    }
    if had_audio {
        changed.insert(
            "server_audio_runtime_enabled".to_string(),
            json!(server_audio_runtime_enabled),
        );
    }
    if had_msg {
        changed.insert(
            "message_polish_mode".to_string(),
            json!(message_polish_mode),
        );
    }
    if had_notif {
        changed.insert("notification_prefs".to_string(), notification_prefs.clone());
    }
    if had_privacy {
        changed.insert("privacy_prefs".to_string(), privacy_prefs.clone());
    }

    let _ = sqlx::query(
        "INSERT INTO runtime_settings_audit_log
                (account_id, changed_by, changed_fields_json, source)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(account_id)
    .bind(account_id)
    .bind(Value::Object(changed))
    .bind(source)
    .execute(&state.db)
    .await;

    Ok(row)
}

async fn load_cred_infos(state: &AppState, account_id: Uuid) -> Vec<CredentialInfo> {
    #[derive(sqlx::FromRow)]
    struct CredRow {
        id: Uuid,
        provider: String,
        display_name: String,
        secret_last4: String,
        status: String,
        updated_at: DateTime<Utc>,
    }

    sqlx::query_as::<_, CredRow>(
        "SELECT id, provider, display_name, secret_last4, status, updated_at
           FROM runtime_provider_credentials
          WHERE account_id = $1 AND status <> 'revoked'
          ORDER BY updated_at DESC",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| CredentialInfo {
        id: r.id,
        provider: r.provider,
        display_name: r.display_name,
        secret_last4: r.secret_last4,
        status: r.status,
        updated_at: r.updated_at,
    })
    .collect()
}

fn into_response(row: SettingsRow, credentials: Vec<CredentialInfo>) -> SettingsResponse {
    SettingsResponse {
        selected_model: row.selected_model,
        output_language: row.output_language,
        tone_preset: row.tone_preset,
        custom_prompt: row.custom_prompt,
        auto_paste: row.auto_paste,
        edit_capture: row.edit_capture,
        learning_enabled: row.learning_enabled,
        server_runtime_enabled: row.server_runtime_enabled,
        server_audio_runtime_enabled: row.server_audio_runtime_enabled,
        message_polish_mode: row.message_polish_mode,
        notification_prefs: row.notification_prefs_json,
        privacy_prefs: row.privacy_prefs_json,
        version: row.version,
        updated_at: row.updated_at,
        credentials,
    }
}

fn db_err(e: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::error!("[runtime-settings] db error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": msg})))
}
