//! Org-admin dictation observability + desktop ingest endpoints.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

fn require_org_admin(role: &str) -> Result<(), StatusCode> {
    if role.eq_ignore_ascii_case("admin")
        || role.eq_ignore_ascii_case("owner")
        || role.eq_ignore_ascii_case("COMPANY_ADMIN")
    {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn window_days(days: Option<i32>) -> (i32, DateTime<Utc>) {
    let days = days.unwrap_or(30).clamp(1, 90);
    let since = Utc::now() - chrono::Duration::days(days as i64);
    (days, since)
}

fn herr(msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

async fn ensure_org_account_member(
    db: &sqlx::PgPool,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<(), StatusCode> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM org_members WHERE org_id = $1 AND account_id = $2
        )",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_one(db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if exists {
        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

// ── Admin list / detail ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DictationListQuery {
    pub account_id: Option<Uuid>,
    pub days: Option<i32>,
    #[serde(default = "default_dictation_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_dictation_limit() -> i64 {
    50
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DictationListItem {
    pub id: Uuid,
    pub account_id: Uuid,
    pub recording_id: Option<String>,
    pub client_run_id: Option<String>,
    pub target_app: Option<String>,
    pub word_count: Option<i32>,
    pub recording_seconds: Option<f64>,
    pub model_used: Option<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub edit_bucket: Option<String>,
    pub edit_detected: Option<bool>,
    pub total_ms: Option<i32>,
    pub has_edit_feedback: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DictationDetailItem {
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
    pub edit_feedback_json: Value,
    pub dictation_trace_json: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub edit_bucket: Option<String>,
    pub edit_detected: Option<bool>,
    pub total_ms: Option<i32>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AliasLearnEvent {
    pub id: Uuid,
    pub account_id: Uuid,
    pub recording_id: Option<String>,
    pub heard: String,
    pub correct: String,
    pub source: String,
    pub safety: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn list_org_dictation(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(q): Query<DictationListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| json_err(StatusCode::FORBIDDEN, "forbidden"))?;
    require_org_admin(&role).map_err(|_| json_err(StatusCode::FORBIDDEN, "admin required"))?;

    if let Some(account_id) = q.account_id {
        ensure_org_account_member(&state.db, org_id, account_id)
            .await
            .map_err(|s| json_err(s, "account not in org"))?;
    }

    let (days, since) = window_days(q.days);
    let limit = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
           FROM runtime_history_items h
          WHERE (h.org_id = $1 OR h.org_id IS NULL)
            AND h.deleted_at IS NULL
            AND h.created_at >= $2
            AND ($3::uuid IS NULL OR h.account_id = $3)",
    )
    .bind(org_id)
    .bind(since)
    .bind(q.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| herr("database error"))?;

    let items: Vec<DictationListItem> = sqlx::query_as(
        "SELECT
            h.id,
            h.account_id,
            h.recording_id,
            h.client_run_id,
            h.target_app,
            h.word_count,
            h.recording_seconds,
            h.model_used,
            h.source,
            h.created_at,
            r.edit_bucket,
            r.edit_detected,
            r.total_ms,
            (h.edit_feedback_json IS NOT NULL AND h.edit_feedback_json != '{}'::jsonb) AS has_edit_feedback
         FROM runtime_history_items h
         LEFT JOIN runtime_telemetry_runs r
           ON r.account_id = h.account_id
          AND (h.org_id IS NULL OR r.org_id = h.org_id)
          AND (
            (h.recording_id IS NOT NULL AND r.recording_id = h.recording_id)
            OR (h.client_run_id IS NOT NULL AND r.run_id = h.client_run_id)
          )
         WHERE (h.org_id = $1 OR h.org_id IS NULL)
           AND h.deleted_at IS NULL
           AND h.created_at >= $2
           AND ($3::uuid IS NULL OR h.account_id = $3)
         ORDER BY h.created_at DESC
         LIMIT $4 OFFSET $5",
    )
    .bind(org_id)
    .bind(since)
    .bind(q.account_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::warn!("[observability] list error: {e}");
        herr("database error")
    })?;

    Ok(Json(json!({
        "window_days": days,
        "total": total,
        "items": items,
    })))
}

pub async fn get_org_dictation_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, lookup_key)): Path<(Uuid, String)>,
    Query(q): Query<DictationListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| json_err(StatusCode::FORBIDDEN, "forbidden"))?;
    require_org_admin(&role).map_err(|_| json_err(StatusCode::FORBIDDEN, "admin required"))?;

    let lookup_key = lookup_key.trim();
    if lookup_key.is_empty() {
        return Err(json_err(StatusCode::BAD_REQUEST, "lookup key required"));
    }

    let row: DictationDetailItem = sqlx::query_as(
        "SELECT
            h.id,
            h.account_id,
            h.org_id,
            h.run_id,
            h.client_run_id,
            h.recording_id,
            h.device_id,
            h.platform,
            h.app_version,
            h.source,
            h.raw_transcript,
            h.transcript,
            h.local_corrected_transcript,
            h.polished_output,
            h.final_text,
            h.model_used,
            h.word_count,
            h.recording_seconds,
            h.transcribe_ms,
            h.embed_ms,
            h.polish_ms,
            h.target_app,
            h.edit_feedback_json,
            h.dictation_trace_json,
            h.created_at,
            h.updated_at,
            r.edit_bucket,
            r.edit_detected,
            r.total_ms
         FROM runtime_history_items h
         LEFT JOIN runtime_telemetry_runs r
           ON r.account_id = h.account_id
          AND (h.org_id IS NULL OR r.org_id = h.org_id)
          AND (
            (h.recording_id IS NOT NULL AND r.recording_id = h.recording_id)
            OR (h.client_run_id IS NOT NULL AND r.run_id = h.client_run_id)
          )
         WHERE (h.org_id = $1 OR h.org_id IS NULL)
           AND h.deleted_at IS NULL
           AND ($3::uuid IS NULL OR h.account_id = $3)
           AND (
             h.id::text = $2
             OR h.recording_id = $2
             OR h.client_run_id = $2
           )
         LIMIT 1",
    )
    .bind(org_id)
    .bind(lookup_key)
    .bind(q.account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::warn!("[observability] detail lookup failed key={lookup_key}: {e}");
        herr("database error")
    })?
    .ok_or_else(|| {
        tracing::warn!(
            "[observability] dictation not found org={org_id} key={lookup_key} account_id={:?}",
            q.account_id
        );
        json_err(StatusCode::NOT_FOUND, "dictation not found")
    })?;

    if let Some(account_id) = q.account_id {
        ensure_org_account_member(&state.db, org_id, account_id)
            .await
            .map_err(|s| json_err(s, "account not in org"))?;
        if row.account_id != account_id {
            return Err(json_err(StatusCode::NOT_FOUND, "dictation not found"));
        }
    }

    let aliases: Vec<AliasLearnEvent> =
        if let Some(rec_id) = row.recording_id.as_deref().filter(|s| !s.is_empty()) {
            sqlx::query_as(
                "SELECT id, account_id, recording_id, heard, correct, source, safety, created_at
               FROM runtime_alias_learn_events
              WHERE org_id = $1 AND account_id = $2 AND recording_id = $3
              ORDER BY created_at ASC",
            )
            .bind(org_id)
            .bind(row.account_id)
            .bind(rec_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
        } else {
            vec![]
        };

    Ok(Json(json!({
        "item": row,
        "alias_events": aliases,
    })))
}

pub async fn list_user_alias_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, account_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<DictationListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| json_err(StatusCode::FORBIDDEN, "forbidden"))?;
    require_org_admin(&role).map_err(|_| json_err(StatusCode::FORBIDDEN, "admin required"))?;
    ensure_org_account_member(&state.db, org_id, account_id)
        .await
        .map_err(|s| json_err(s, "account not in org"))?;

    let (days, since) = window_days(q.days);
    let limit = q.limit.clamp(1, 500);

    let items: Vec<AliasLearnEvent> = sqlx::query_as(
        "SELECT id, account_id, recording_id, heard, correct, source, safety, created_at
           FROM runtime_alias_learn_events
          WHERE org_id = $1 AND account_id = $2 AND created_at >= $3
          ORDER BY created_at DESC
          LIMIT $4",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(since)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|_| herr("database error"))?;

    Ok(Json(json!({ "window_days": days, "items": items })))
}

pub async fn org_observability_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(q): Query<DictationListQuery>,
) -> Result<Json<Value>, StatusCode> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    require_org_admin(&role)?;

    let (days, since) = window_days(q.days);

    let dictation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM runtime_history_items
          WHERE org_id = $1 AND deleted_at IS NULL AND created_at >= $2",
    )
    .bind(org_id)
    .bind(since)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let aliases_learned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM runtime_alias_learn_events
          WHERE org_id = $1 AND created_at >= $2",
    )
    .bind(org_id)
    .bind(since)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let edits_with_feedback: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM runtime_history_items
          WHERE org_id = $1 AND deleted_at IS NULL AND created_at >= $2
            AND edit_feedback_json IS NOT NULL AND edit_feedback_json != '{}'::jsonb",
    )
    .bind(org_id)
    .bind(since)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let stt_error_edits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM runtime_history_items
          WHERE org_id = $1 AND deleted_at IS NULL AND created_at >= $2
            AND LOWER(COALESCE(edit_feedback_json->>'class', '')) = 'stt_error'",
    )
    .bind(org_id)
    .bind(since)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let classify_stt_error_rate = if edits_with_feedback > 0 {
        (stt_error_edits as f64 / edits_with_feedback as f64 * 1000.0).round() / 1000.0
    } else {
        0.0
    };

    Ok(Json(json!({
        "window_days": days,
        "dictation_count": dictation_count,
        "aliases_learned": aliases_learned,
        "edits_detected": edits_with_feedback,
        "stt_error_edits": stt_error_edits,
        "classify_stt_error_rate": classify_stt_error_rate,
    })))
}

// ── Desktop ingest ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DictationUpsertRequest {
    pub recording_id: String,
    pub client_run_id: Option<String>,
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
    pub source: Option<String>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub dictation_trace_json: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct DictationPatchRequest {
    pub final_text: Option<String>,
    pub edit_feedback_json: Option<Value>,
    pub dictation_trace_json: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct AliasLearnEventItem {
    pub heard: String,
    pub correct: String,
    pub source: Option<String>,
    pub safety: Option<String>,
    pub recording_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AliasBatchRequest {
    pub items: Vec<AliasLearnEventItem>,
}

#[derive(Debug, Serialize)]
pub struct IngestOk {
    pub ok: bool,
}

/// Match an existing row by recording_id or client_run_id (dedup index prefers client_run_id).
const DICTATION_UPDATE_WHERE: &str = "
    WHERE account_id = $1 AND deleted_at IS NULL
      AND (
        recording_id = $2
        OR ($3::text IS NOT NULL AND client_run_id = $3)
      )";

async fn apply_dictation_update(
    db: &sqlx::PgPool,
    account_id: Uuid,
    recording_id: &str,
    client_run_id: Option<&str>,
    org_id: Option<Uuid>,
    req: &DictationUpsertRequest,
    source: &str,
    word_count: Option<i32>,
) -> Result<u64, sqlx::Error> {
    let updated = sqlx::query(&format!(
        "UPDATE runtime_history_items SET
            org_id = COALESCE($4, org_id),
            recording_id = COALESCE(recording_id, $2),
            client_run_id = COALESCE($3, client_run_id),
            raw_transcript = COALESCE($5, raw_transcript),
            transcript = COALESCE($6, transcript),
            local_corrected_transcript = COALESCE($7, local_corrected_transcript),
            polished_output = COALESCE($8, polished_output),
            final_text = COALESCE($9, final_text),
            model_used = COALESCE($10, model_used),
            word_count = COALESCE($11, word_count),
            recording_seconds = COALESCE($12, recording_seconds),
            transcribe_ms = COALESCE($13, transcribe_ms),
            embed_ms = COALESCE($14, embed_ms),
            polish_ms = COALESCE($15, polish_ms),
            target_app = COALESCE($16, target_app),
            source = COALESCE($17, source),
            device_id = COALESCE($18, device_id),
            platform = COALESCE($19, platform),
            app_version = COALESCE($20, app_version),
            dictation_trace_json = COALESCE($21, dictation_trace_json),
            updated_at = now()
         {DICTATION_UPDATE_WHERE}"
    ))
    .bind(account_id)
    .bind(recording_id)
    .bind(client_run_id)
    .bind(org_id)
    .bind(req.raw_transcript.as_deref())
    .bind(req.transcript.as_deref())
    .bind(req.local_corrected_transcript.as_deref())
    .bind(req.polished_output.as_deref())
    .bind(req.final_text.as_deref())
    .bind(req.model_used.as_deref())
    .bind(word_count)
    .bind(req.recording_seconds)
    .bind(req.transcribe_ms)
    .bind(req.embed_ms)
    .bind(req.polish_ms)
    .bind(req.target_app.as_deref())
    .bind(source)
    .bind(req.device_id.as_deref())
    .bind(req.platform.as_deref())
    .bind(req.app_version.as_deref())
    .bind(req.dictation_trace_json.as_ref())
    .execute(db)
    .await?
    .rows_affected();
    Ok(updated)
}

async fn upsert_dictation_row(
    db: &sqlx::PgPool,
    account_id: Uuid,
    org_id: Option<Uuid>,
    req: &DictationUpsertRequest,
) -> Result<(), sqlx::Error> {
    let recording_id = req.recording_id.trim();
    if recording_id.is_empty() {
        return Ok(());
    }

    let client_run_id = req.client_run_id.as_deref().filter(|s| !s.is_empty());

    let source = req
        .source
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("desktop_voice");

    let word_count = req.word_count.or_else(|| {
        req.polished_output
            .as_deref()
            .or(req.transcript.as_deref())
            .map(|t| t.split_whitespace().count() as i32)
    });

    if apply_dictation_update(
        db,
        account_id,
        recording_id,
        client_run_id,
        org_id,
        req,
        source,
        word_count,
    )
    .await?
        > 0
    {
        return Ok(());
    }

    let inserted = sqlx::query(
        "INSERT INTO runtime_history_items
            (account_id, org_id, client_run_id, recording_id, source,
             device_id, platform, app_version,
             raw_transcript, transcript, local_corrected_transcript,
             polished_output, final_text, model_used,
             word_count, recording_seconds,
             transcribe_ms, embed_ms, polish_ms, target_app, dictation_trace_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
         ON CONFLICT DO NOTHING",
    )
    .bind(account_id)
    .bind(org_id)
    .bind(client_run_id)
    .bind(recording_id)
    .bind(source)
    .bind(req.device_id.as_deref())
    .bind(req.platform.as_deref())
    .bind(req.app_version.as_deref())
    .bind(req.raw_transcript.as_deref())
    .bind(req.transcript.as_deref())
    .bind(req.local_corrected_transcript.as_deref())
    .bind(req.polished_output.as_deref())
    .bind(req.final_text.as_deref())
    .bind(req.model_used.as_deref())
    .bind(word_count)
    .bind(req.recording_seconds)
    .bind(req.transcribe_ms)
    .bind(req.embed_ms)
    .bind(req.polish_ms)
    .bind(req.target_app.as_deref())
    .bind(req.dictation_trace_json.as_ref())
    .execute(db)
    .await?
    .rows_affected();

    if inserted > 0 {
        return Ok(());
    }

    // Server runtime may have inserted first (client_run_id only, ON CONFLICT DO NOTHING).
    // Merge desktop fields without treating the duplicate as an error.
    let _ = apply_dictation_update(
        db,
        account_id,
        recording_id,
        client_run_id,
        org_id,
        req,
        source,
        word_count,
    )
    .await?;

    Ok(())
}

pub async fn ingest_dictation(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<DictationUpsertRequest>,
) -> Result<Json<IngestOk>, (StatusCode, Json<Value>)> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers)
        .await
        .map_err(|_| json_err(StatusCode::FORBIDDEN, "forbidden"))?;

    upsert_dictation_row(&state.db, user.account_id, tenant_ctx.active_org_id, &req)
        .await
        .map_err(|e| {
            tracing::warn!("[observability] ingest dictation failed: {e}");
            herr("database error")
        })?;

    // Fire-and-forget: enqueue a coalesced profiling+KB window job once the user crosses
    // the dictation threshold. Idempotent — at most one in-flight job per user.
    let enqueue_db = state.db.clone();
    let enqueue_account = user.account_id;
    let enqueue_scope = crate::profile::store::resolve_org_scope(tenant_ctx.active_org_id);
    tokio::spawn(async move {
        if let Err(e) =
            crate::profile::updater::batch::maybe_enqueue(&enqueue_db, enqueue_account, enqueue_scope).await
        {
            tracing::warn!("[profile-batch] enqueue failed: {e}");
        }
    });

    Ok(Json(IngestOk { ok: true }))
}

pub async fn patch_dictation(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(recording_id): Path<String>,
    Json(req): Json<DictationPatchRequest>,
) -> Result<Json<IngestOk>, (StatusCode, Json<Value>)> {
    let recording_id = recording_id.trim().to_string();
    let merged_trace = if let Some(incoming) = req.dictation_trace_json.as_ref() {
        let existing: Option<Value> = sqlx::query_scalar(
            "SELECT dictation_trace_json
               FROM runtime_history_items
              WHERE account_id = $1 AND recording_id = $2 AND deleted_at IS NULL
              LIMIT 1",
        )
        .bind(user.account_id)
        .bind(&recording_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| herr("database error"))?;
        Some(said_core::dictation_trace::merge_trace_values(
            existing.as_ref(),
            Some(incoming),
        ))
    } else {
        None
    };
    let n = sqlx::query(
        "UPDATE runtime_history_items SET
            final_text = COALESCE($3, final_text),
            edit_feedback_json = COALESCE($4, edit_feedback_json),
            dictation_trace_json = COALESCE($5, dictation_trace_json),
            updated_at = now()
         WHERE account_id = $1 AND recording_id = $2 AND deleted_at IS NULL",
    )
    .bind(user.account_id)
    .bind(&recording_id)
    .bind(req.final_text.as_deref())
    .bind(req.edit_feedback_json.as_ref())
    .bind(merged_trace.as_ref())
    .execute(&state.db)
    .await
    .map_err(|_| herr("database error"))?
    .rows_affected();

    if n == 0 {
        let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers)
            .await
            .map_err(|_| json_err(StatusCode::FORBIDDEN, "forbidden"))?;
        let empty = Value::Object(Default::default());
        let feedback = req.edit_feedback_json.as_ref().unwrap_or(&empty);
        let trace = merged_trace
            .as_ref()
            .or(req.dictation_trace_json.as_ref())
            .unwrap_or(&empty);
        sqlx::query(
            "INSERT INTO runtime_history_items
                (account_id, org_id, recording_id, source, final_text, edit_feedback_json, dictation_trace_json)
             VALUES ($1, $2, $3, 'desktop_voice', $4, $5, $6)
             ON CONFLICT DO NOTHING",
        )
        .bind(user.account_id)
        .bind(tenant_ctx.active_org_id)
        .bind(&recording_id)
        .bind(req.final_text.as_deref())
        .bind(feedback)
        .bind(trace)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::warn!("[observability] patch insert fallback failed: {e}");
            herr("database error")
        })?;
    }

    Ok(Json(IngestOk { ok: true }))
}

pub async fn ingest_aliases(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<AliasBatchRequest>,
) -> Result<Json<IngestOk>, (StatusCode, Json<Value>)> {
    if req.items.is_empty() {
        return Ok(Json(IngestOk { ok: true }));
    }

    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers)
        .await
        .map_err(|_| json_err(StatusCode::FORBIDDEN, "forbidden"))?;
    let org_id = tenant_ctx.active_org_id;

    for item in &req.items {
        let heard = item.heard.trim();
        let correct = item.correct.trim();
        if heard.is_empty() || correct.is_empty() {
            continue;
        }
        let source = item
            .source
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("classify");
        sqlx::query(
            "INSERT INTO runtime_alias_learn_events
                (account_id, org_id, recording_id, heard, correct, source, safety)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(item.recording_id.as_deref().filter(|s| !s.is_empty()))
        .bind(heard)
        .bind(correct)
        .bind(source)
        .bind(item.safety.as_deref())
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::warn!("[observability] ingest alias failed: {e}");
            herr("database error")
        })?;
    }

    Ok(Json(IngestOk { ok: true }))
}
