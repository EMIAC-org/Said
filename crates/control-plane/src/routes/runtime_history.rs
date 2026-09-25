//! Server-side history endpoints.
//!
//! History stores transcript/output/edit text for signed-in users.
//! Raw audio and screen context are never stored.
//!
//! Endpoints:
//!   GET    /v1/runtime/history
//!   GET    /v1/runtime/history/:id
//!   PATCH  /v1/runtime/history/:id
//!   DELETE /v1/runtime/history/:id
//!   POST   /v1/runtime/history/sync   (batch upsert from desktop)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

// ── History item ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RuntimeHistoryItem {
    pub id: Uuid,
    pub account_id: Uuid,
    pub org_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub client_run_id: Option<String>,
    pub recording_id: Option<String>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub source: String,
    pub raw_transcript: Option<String>,
    pub transcript: Option<String>,
    pub local_corrected_transcript: Option<String>,
    pub polished_output: Option<String>,
    pub final_text: Option<String>,
    pub model_used: Option<String>,
    pub word_count: Option<i32>,
    pub recording_seconds: Option<f64>,
    pub transcribe_ms: Option<i64>,
    pub embed_ms: Option<i64>,
    pub polish_ms: Option<i64>,
    pub target_app: Option<String>,
    pub formatter_trace_json: Value,
    pub resolver_trace_json: Value,
    pub edit_feedback_json: Value,
    pub privacy_json: Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct HistoryListQuery {
    #[serde(default = "default_history_limit")]
    pub limit: i64,
    pub before: Option<String>,
    #[serde(default)]
    pub include_deleted: bool,
}

fn default_history_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize)]
pub struct HistoryPatchRequest {
    pub final_text: Option<String>,
    pub edit_feedback_json: Option<Value>,
    /// true = soft-delete, false = restore
    pub deleted: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct HistorySyncRequest {
    pub items: Vec<HistorySyncItem>,
}

#[derive(Debug, Deserialize)]
pub struct HistorySyncItem {
    pub client_run_id: Option<String>,
    pub recording_id: Option<String>,
    pub source: Option<String>,
    pub raw_transcript: Option<String>,
    pub transcript: Option<String>,
    pub local_corrected_transcript: Option<String>,
    pub polished_output: Option<String>,
    pub final_text: Option<String>,
    pub model_used: Option<String>,
    pub word_count: Option<i32>,
    pub recording_seconds: Option<f64>,
    pub transcribe_ms: Option<i64>,
    pub embed_ms: Option<i64>,
    pub polish_ms: Option<i64>,
    pub target_app: Option<String>,
    pub created_at: Option<String>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub edit_feedback_json: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct HistorySyncResponse {
    pub accepted: usize,
    pub skipped: usize,
    pub failed: usize,
}

// ── History: GET list ─────────────────────────────────────────────────────────

pub async fn list_history(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<HistoryListQuery>,
) -> Result<Json<Vec<RuntimeHistoryItem>>, (StatusCode, Json<Value>)> {
    let limit = query.limit.clamp(1, 200);

    let rows = if query.include_deleted {
        if let Some(before) = query.before.as_deref() {
            let ts = parse_ts(before)?;
            sqlx::query_as::<_, RuntimeHistoryItem>(
                "SELECT * FROM runtime_history_items
                  WHERE account_id = $1 AND created_at < $2
                  ORDER BY created_at DESC LIMIT $3",
            )
            .bind(user.account_id)
            .bind(ts)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        } else {
            sqlx::query_as::<_, RuntimeHistoryItem>(
                "SELECT * FROM runtime_history_items
                  WHERE account_id = $1 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(user.account_id)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
    } else if let Some(before) = query.before.as_deref() {
        let ts = parse_ts(before)?;
        sqlx::query_as::<_, RuntimeHistoryItem>(
            "SELECT * FROM runtime_history_items
              WHERE account_id = $1 AND deleted_at IS NULL AND created_at < $2
              ORDER BY created_at DESC LIMIT $3",
        )
        .bind(user.account_id)
        .bind(ts)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, RuntimeHistoryItem>(
            "SELECT * FROM runtime_history_items
              WHERE account_id = $1 AND deleted_at IS NULL
              ORDER BY created_at DESC LIMIT $2",
        )
        .bind(user.account_id)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    };

    rows.map(Json).map_err(|e| {
        tracing::warn!("[history] list error: {e}");
        herr("database error")
    })
}

pub async fn get_history_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<RuntimeHistoryItem>, (StatusCode, Json<Value>)> {
    let row = sqlx::query_as::<_, RuntimeHistoryItem>(
        "SELECT * FROM runtime_history_items WHERE id = $1 AND account_id = $2",
    )
    .bind(id)
    .bind(user.account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| herr("database error"))?
    .ok_or_else(|| json_err(StatusCode::NOT_FOUND, "history item not found"))?;

    Ok(Json(row))
}

pub async fn patch_history_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<HistoryPatchRequest>,
) -> Result<Json<RuntimeHistoryItem>, (StatusCode, Json<Value>)> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_history_items WHERE id=$1 AND account_id=$2)",
    )
    .bind(id)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| herr("database error"))?;

    if !owned {
        return Err(json_err(StatusCode::NOT_FOUND, "history item not found"));
    }

    if let Some(text) = &req.final_text {
        sqlx::query("UPDATE runtime_history_items SET final_text=$2, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(text)
            .execute(&state.db)
            .await
            .map_err(|_| herr("database error"))?;
    }

    if let Some(fb) = &req.edit_feedback_json {
        sqlx::query(
            "UPDATE runtime_history_items SET edit_feedback_json=$2, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(fb)
        .execute(&state.db)
        .await
        .map_err(|_| herr("database error"))?;
    }

    if let Some(deleted) = req.deleted {
        let ts: Option<chrono::DateTime<chrono::Utc>> = if deleted {
            Some(chrono::Utc::now())
        } else {
            None
        };
        sqlx::query("UPDATE runtime_history_items SET deleted_at=$2, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(ts)
            .execute(&state.db)
            .await
            .map_err(|_| herr("database error"))?;
    }

    get_history_item(State(state), user, Path(id)).await
}

pub async fn delete_history_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let n = sqlx::query(
        "UPDATE runtime_history_items
            SET deleted_at=now(), updated_at=now()
          WHERE id=$1 AND account_id=$2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(|_| herr("database error"))?
    .rows_affected();

    if n == 0 {
        return Err(json_err(StatusCode::NOT_FOUND, "history item not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── History: POST /sync ───────────────────────────────────────────────────────

pub async fn sync_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<HistorySyncRequest>,
) -> Result<Json<HistorySyncResponse>, (StatusCode, Json<Value>)> {
    if req.items.is_empty() {
        return Ok(Json(HistorySyncResponse {
            accepted: 0,
            skipped: 0,
            failed: 0,
        }));
    }

    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let org_id = tenant_ctx.active_org_id;
    let mut accepted = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for item in &req.items {
        let created_at = item
            .created_at
            .as_deref()
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .unwrap_or_else(chrono::Utc::now);

        let source = item
            .source
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("desktop_sync");

        let word_count = item.word_count.or_else(|| {
            item.final_text
                .as_deref()
                .or(item.polished_output.as_deref())
                .map(|t| t.split_whitespace().count() as i32)
        });

        let empty_obj = Value::Object(Default::default());
        let edit_feedback = item.edit_feedback_json.as_ref().unwrap_or(&empty_obj);

        let result = sqlx::query(
            "INSERT INTO runtime_history_items
                 (account_id, org_id, client_run_id, recording_id, source,
                  device_id, platform, app_version,
                  raw_transcript, transcript, local_corrected_transcript,
                  polished_output, final_text, model_used,
                  word_count, recording_seconds,
                  transcribe_ms, embed_ms, polish_ms,
                  target_app, edit_feedback_json, created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
             ON CONFLICT DO NOTHING",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(item.client_run_id.as_deref().filter(|s| !s.is_empty()))
        .bind(item.recording_id.as_deref().filter(|s| !s.is_empty()))
        .bind(source)
        .bind(item.device_id.as_deref())
        .bind(item.platform.as_deref())
        .bind(item.app_version.as_deref())
        .bind(item.raw_transcript.as_deref())
        .bind(item.transcript.as_deref())
        .bind(item.local_corrected_transcript.as_deref())
        .bind(item.polished_output.as_deref())
        .bind(item.final_text.as_deref())
        .bind(item.model_used.as_deref())
        .bind(word_count)
        .bind(item.recording_seconds)
        .bind(item.transcribe_ms)
        .bind(item.embed_ms)
        .bind(item.polish_ms)
        .bind(item.target_app.as_deref())
        .bind(edit_feedback)
        .bind(created_at)
        .execute(&state.db)
        .await;

        match result {
            Ok(r) if r.rows_affected() > 0 => accepted += 1,
            Ok(_) => skipped += 1,
            Err(e) => {
                tracing::warn!("[history-sync] insert failed: {e}");
                failed += 1;
            }
        }
    }

    Ok(Json(HistorySyncResponse {
        accepted,
        skipped,
        failed,
    }))
}

// ── Called by voice_polish / voice_wav after successful completion ────────────

pub async fn write_history_from_runtime(
    state: &AppState,
    account_id: Uuid,
    org_id: Option<Uuid>,
    run_id: Uuid,
    client_run_id: Option<&str>,
    recording_id: Option<&str>,
    transcript: &str,
    output: &str,
    model_used: &str,
    source: &str,
    target_app: Option<&str>,
    transcribe_ms: Option<i64>,
    polish_ms: Option<i64>,
) {
    let word_count = output.split_whitespace().count() as i32;
    let r = sqlx::query(
        "INSERT INTO runtime_history_items
             (account_id, org_id, run_id, client_run_id, recording_id, source,
              transcript, polished_output, final_text, model_used,
              word_count, target_app, transcribe_ms, polish_ms)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$10,$11,$12,$13)
         ON CONFLICT DO NOTHING",
    )
    .bind(account_id)
    .bind(org_id)
    .bind(run_id)
    .bind(client_run_id)
    .bind(recording_id)
    .bind(source)
    .bind(transcript)
    .bind(output)
    .bind(model_used)
    .bind(word_count)
    .bind(target_app)
    .bind(transcribe_ms)
    .bind(polish_ms)
    .execute(&state.db)
    .await;

    if let Err(e) = r {
        tracing::warn!("[history] write_history_from_runtime failed: {e}");
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn parse_ts(s: &str) -> Result<chrono::DateTime<chrono::Utc>, (StatusCode, Json<Value>)> {
    s.parse::<chrono::DateTime<chrono::Utc>>()
        .map_err(|_| json_err(StatusCode::BAD_REQUEST, "invalid timestamp"))
}

fn herr(msg: &str) -> (StatusCode, Json<Value>) {
    json_err(StatusCode::INTERNAL_SERVER_ERROR, msg)
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "message": msg, "error": msg })))
}
