//! Server-side history and memory-sync endpoints.
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
//!   POST   /v1/runtime/memory/sync    (batch upsert personal memory)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, memory_hygiene, tenant};

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

// ── Memory sync types ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MemorySyncRequest {
    #[serde(default)]
    pub vocab_terms: Vec<VocabTermSyncItem>,
    #[serde(default)]
    pub stt_replacements: Vec<AliasSyncItem>,
    #[serde(default)]
    pub edit_policy_rules: Vec<PolicyRuleSyncItem>,
    #[serde(default)]
    pub email_memory: Vec<EmailMemorySyncItem>,
}

#[derive(Debug, Deserialize)]
pub struct VocabTermSyncItem {
    pub term: String,
    pub term_type: Option<String>,
    pub weight: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct AliasSyncItem {
    pub transcript_form: String,
    pub correct_form: String,
    pub edit_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PolicyRuleSyncItem {
    pub variant_form: String,
    pub correct_form: String,
    pub edit_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EmailMemorySyncItem {
    pub email: String,
}

#[derive(Debug, Serialize)]
pub struct MemorySyncResponse {
    pub accepted_vocab: usize,
    pub accepted_aliases: usize,
    pub accepted_policies: usize,
    pub accepted_emails: usize,
    pub blocked_vocab: usize,
    pub blocked_aliases: usize,
    pub skipped: usize,
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

// ── Memory: POST /sync ────────────────────────────────────────────────────────

pub async fn sync_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<MemorySyncRequest>,
) -> Result<Json<MemorySyncResponse>, (StatusCode, Json<Value>)> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let org_id = tenant_ctx.active_org_id;

    let mut accepted_vocab = 0usize;
    let mut blocked_vocab = 0usize;
    let mut accepted_aliases = 0usize;
    let mut blocked_aliases = 0usize;
    let mut accepted_policies = 0usize;
    let mut accepted_emails = 0usize;
    let mut skipped = 0usize;

    for item in &req.vocab_terms {
        let term = item.term.trim();
        if term.is_empty() {
            skipped += 1;
            continue;
        }
        let term_type = item.term_type.as_deref().unwrap_or("proper_noun");
        let weight = item.weight.unwrap_or(1.0).clamp(0.1, 10.0);
        let term_norm = norm(term);

        if is_common(&term_norm) || !allowed_type(term_type) || word_count(&term_norm) > 4 {
            blocked_vocab += 1;
            continue;
        }

        let res = sqlx::query(
            "INSERT INTO personal_vocab_terms
                 (account_id,org_id,term,term_norm,term_type,source,weight,positive_count,status)
             VALUES ($1,$2,$3,$4,$5,'desktop_sync',$6,1,'active')
             ON CONFLICT (account_id,term_norm) DO UPDATE SET
                 positive_count=personal_vocab_terms.positive_count+1,
                 weight=GREATEST(personal_vocab_terms.weight,EXCLUDED.weight),
                 status='active', last_seen_at=now(), updated_at=now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(term)
        .bind(&term_norm)
        .bind(term_type)
        .bind(weight)
        .execute(&state.db)
        .await;

        if res.is_ok() {
            accepted_vocab += 1;
        } else {
            skipped += 1;
        }
    }

    for item in &req.stt_replacements {
        let src = item.transcript_form.trim();
        let dst = item.correct_form.trim();
        if src.is_empty() || dst.is_empty() {
            skipped += 1;
            continue;
        }
        let sn = norm(src);
        let dn = norm(dst);

        if sn == dn
            || is_common(&sn)
            || is_common(&dn)
            || word_count(&sn) > 4
            || word_count(&dn) > 4
        {
            blocked_aliases += 1;
            continue;
        }

        let edit_type = item.edit_type.as_deref().unwrap_or("replace");

        let r1 = sqlx::query(
            "INSERT INTO personal_stt_replacements
                 (account_id,org_id,transcript_form,transcript_norm,
                  correct_form,correct_norm,positive_count,weight,status,safety_status)
             VALUES ($1,$2,$3,$4,$5,$6,1,1.0,'active','safe_jargon')
             ON CONFLICT (account_id,transcript_norm,correct_norm) DO UPDATE SET
                 positive_count=personal_stt_replacements.positive_count+1,
                 status='active', last_seen_at=now(), updated_at=now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(src)
        .bind(&sn)
        .bind(dst)
        .bind(&dn)
        .execute(&state.db)
        .await;

        if r1.is_err() {
            skipped += 1;
            continue;
        }

        let _ = sqlx::query(
            "INSERT INTO personal_edit_policy_rules
                 (account_id,org_id,variant_form,variant_norm,
                  correct_form,correct_norm,edit_type,positive_count,status)
             VALUES ($1,$2,$3,$4,$5,$6,$7,1,'active')
             ON CONFLICT (account_id,variant_norm,correct_norm,edit_type) DO UPDATE SET
                 positive_count=personal_edit_policy_rules.positive_count+1,
                 status='active', last_seen_at=now(), updated_at=now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(src)
        .bind(&sn)
        .bind(dst)
        .bind(&dn)
        .bind(edit_type)
        .execute(&state.db)
        .await;

        accepted_aliases += 1;
    }

    for item in &req.edit_policy_rules {
        let src = item.variant_form.trim();
        let dst = item.correct_form.trim();
        if src.is_empty() || dst.is_empty() {
            skipped += 1;
            continue;
        }
        let sn = norm(src);
        let dn = norm(dst);
        if sn == dn
            || is_common(&sn)
            || is_common(&dn)
            || word_count(&sn) > 4
            || word_count(&dn) > 4
        {
            skipped += 1;
            continue;
        }
        let edit_type = item.edit_type.as_deref().unwrap_or("replace");
        let _ = sqlx::query(
            "INSERT INTO personal_edit_policy_rules
                 (account_id,org_id,variant_form,variant_norm,
                  correct_form,correct_norm,edit_type,positive_count,status)
             VALUES ($1,$2,$3,$4,$5,$6,$7,1,'active')
             ON CONFLICT (account_id,variant_norm,correct_norm,edit_type) DO UPDATE SET
                 positive_count=personal_edit_policy_rules.positive_count+1,
                 status='active', last_seen_at=now(), updated_at=now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(src)
        .bind(&sn)
        .bind(dst)
        .bind(&dn)
        .bind(edit_type)
        .execute(&state.db)
        .await;
        accepted_policies += 1;
    }

    for item in &req.email_memory {
        let email = item.email.trim().to_lowercase();
        if email.is_empty() || !email.contains('@') {
            skipped += 1;
            continue;
        }
        let n = norm(&email);
        let _ = sqlx::query(
            "INSERT INTO personal_vocab_terms
                 (account_id,org_id,term,term_norm,term_type,source,weight,positive_count,status)
             VALUES ($1,$2,$3,$4,'proper_noun','email_memory',1.0,1,'active')
             ON CONFLICT (account_id,term_norm) DO UPDATE SET
                 positive_count=personal_vocab_terms.positive_count+1,
                 status='active', last_seen_at=now(), updated_at=now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(&email)
        .bind(&n)
        .execute(&state.db)
        .await;
        accepted_emails += 1;
    }

    if accepted_vocab > 0 || accepted_aliases > 0 || accepted_policies > 0 || accepted_emails > 0 {
        let _ = memory_hygiene::mark_memory_dirty(&state.db, user.account_id).await;
    }

    Ok(Json(MemorySyncResponse {
        accepted_vocab,
        accepted_aliases,
        accepted_policies,
        accepted_emails,
        blocked_vocab,
        blocked_aliases,
        skipped,
    }))
}

pub async fn mark_memory_dirty_route(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, StatusCode> {
    memory_hygiene::mark_memory_dirty(&state.db, user.account_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "ok" })))
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
    transcribe_ms: Option<i64>,
    polish_ms: Option<i64>,
) {
    let word_count = output.split_whitespace().count() as i32;
    let r = sqlx::query(
        "INSERT INTO runtime_history_items
             (account_id, org_id, run_id, client_run_id, recording_id, source,
              transcript, polished_output, final_text, model_used,
              word_count, transcribe_ms, polish_ms)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$10,$11,$12)
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

fn norm(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_common(n: &str) -> bool {
    const COMMON: &[&str] = &[
        "kaisa",
        "kaisi",
        "kaise",
        "aisa",
        "aisi",
        "aise",
        "laga",
        "lagi",
        "lage",
        "main",
        "mein",
        "hai",
        "hain",
        "tha",
        "thi",
        "the",
        "time",
        "can",
        "go",
        "do",
        "this",
        "for",
        "me",
        "one thing",
        "ek baar",
        "char log",
        "kaam",
        "kya",
        "kyun",
        "aur",
        "batao",
        "bolo",
        "a",
        "an",
        "is",
        "are",
        "was",
        "were",
        "be",
        "been",
        "being",
        "have",
        "has",
        "had",
        "does",
        "did",
        "will",
        "would",
        "could",
        "should",
        "may",
        "might",
        "must",
        "shall",
        "and",
        "or",
        "but",
        "if",
        "in",
        "on",
        "at",
        "to",
        "of",
        "it",
        "its",
        "that",
        "which",
        "who",
        "not",
        "no",
        "yes",
        "ok",
        "okay",
        "yeah",
        "yep",
        "nope",
    ];
    if COMMON.contains(&n) {
        return true;
    }
    let tokens: Vec<_> = n.split_whitespace().collect();
    !tokens.is_empty() && tokens.iter().all(|t| COMMON.contains(t))
}

fn allowed_type(t: &str) -> bool {
    matches!(t, "brand" | "acronym" | "code_identifier" | "proper_noun")
}

fn word_count(t: &str) -> usize {
    t.split_whitespace().count()
}

fn herr(msg: &str) -> (StatusCode, Json<Value>) {
    json_err(StatusCode::INTERNAL_SERVER_ERROR, msg)
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "message": msg, "error": msg })))
}
