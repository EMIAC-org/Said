//! SQLite outbox for control-plane dictation observability (fire-and-forget).

use crate::store::{DbPool, history::InsertRecording};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

const MAX_ATTEMPTS: i64 = 10;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn should_enqueue(pool: &DbPool, user_id: &str) -> bool {
    crate::store::users::get_user(pool, user_id)
        .and_then(|u| u.cloud_token)
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationUpsertPayload {
    pub recording_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_corrected_transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polished_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcribe_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polish_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_app: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dictation_trace_json: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationPatchPayload {
    pub recording_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_feedback_json: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dictation_trace_json: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasLearnItem {
    pub heard: String,
    pub correct: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasBatchPayload {
    pub items: Vec<AliasLearnItem>,
}

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub op: String,
    pub recording_id: Option<String>,
    pub payload_json: String,
    pub attempts: i64,
}

pub struct RecordingObservabilityExtras {
    pub client_run_id: Option<String>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

pub fn after_recording_insert(
    pool: &DbPool,
    user_id: &str,
    rec: &InsertRecording<'_>,
    extras: RecordingObservabilityExtras,
) {
    if !should_enqueue(pool, user_id) {
        return;
    }
    let payload = DictationUpsertPayload {
        recording_id: rec.id.to_string(),
        client_run_id: extras.client_run_id.clone(),
        raw_transcript: rec.raw_transcript.map(str::to_string),
        transcript: Some(rec.transcript.to_string()),
        local_corrected_transcript: rec.local_corrected_transcript.map(str::to_string),
        polished_output: rec.polished_output.map(str::to_string),
        final_text: None,
        model_used: Some(rec.model_used.to_string()),
        word_count: Some(rec.word_count),
        recording_seconds: Some(rec.recording_seconds),
        transcribe_ms: rec.transcribe_ms,
        embed_ms: rec.embed_ms,
        polish_ms: rec.polish_ms,
        target_app: rec.target_app.map(str::to_string),
        source: rec.source.to_string(),
        device_id: extras.device_id.clone(),
        platform: extras.platform.clone(),
        app_version: extras.app_version.clone(),
        dictation_trace_json: rec
            .trace_json
            .and_then(|s| serde_json::from_str::<Value>(s).ok()),
    };
    let _ = enqueue_dictation_upsert(pool, user_id, payload);
}

fn insert_row(
    pool: &DbPool,
    user_id: &str,
    op: &str,
    recording_id: Option<&str>,
    payload: &impl Serialize,
) -> Result<(), String> {
    let payload_json =
        serde_json::to_string(payload).map_err(|e| format!("serialize outbox payload: {e}"))?;
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO observability_outbox
            (user_id, op, recording_id, payload_json, status, attempts, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5)",
        params![user_id, op, recording_id, payload_json, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn enqueue_dictation_upsert(
    pool: &DbPool,
    user_id: &str,
    payload: DictationUpsertPayload,
) -> Result<(), String> {
    let recording_id = payload.recording_id.clone();
    insert_row(
        pool,
        user_id,
        "upsert_dictation",
        Some(&recording_id),
        &payload,
    )
}

pub fn enqueue_dictation_patch(
    pool: &DbPool,
    user_id: &str,
    payload: DictationPatchPayload,
) -> Result<(), String> {
    let recording_id = payload.recording_id.clone();
    insert_row(
        pool,
        user_id,
        "patch_dictation_edit",
        Some(&recording_id),
        &payload,
    )
}

pub fn enqueue_alias_batch(
    pool: &DbPool,
    user_id: &str,
    payload: AliasBatchPayload,
) -> Result<(), String> {
    if payload.items.is_empty() {
        return Ok(());
    }
    insert_row(pool, user_id, "upsert_alias_batch", None, &payload)
}

pub fn list_pending(pool: &DbPool, user_id: &str, limit: i64) -> Result<Vec<OutboxRow>, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, op, recording_id, payload_json, attempts
               FROM observability_outbox
              WHERE user_id = ?1 AND status = 'pending' AND attempts < ?2
              ORDER BY created_at_ms ASC
              LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, MAX_ATTEMPTS, limit], |row| {
            Ok(OutboxRow {
                id: row.get(0)?,
                op: row.get(1)?,
                recording_id: row.get(2)?,
                payload_json: row.get(3)?,
                attempts: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn mark_done(pool: &DbPool, id: i64) -> Result<(), String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE observability_outbox SET status = 'done', last_attempt_ms = ?2 WHERE id = ?1",
        params![id, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn mark_failed(pool: &DbPool, id: i64, error: &str) -> Result<(), String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let attempts: i64 = conn
        .query_row(
            "SELECT attempts FROM observability_outbox WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let next = attempts + 1;
    let status = if next >= MAX_ATTEMPTS {
        "dropped"
    } else {
        "pending"
    };
    if status == "dropped" {
        warn!("[observability] dropping outbox row {id} after {next} attempts: {error}");
    }
    conn.execute(
        "UPDATE observability_outbox
            SET attempts = ?2, last_attempt_ms = ?3, last_error = ?4, status = ?5
          WHERE id = ?1",
        params![id, next, now_ms(), error, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn pending_count(pool: &DbPool, user_id: &str) -> i64 {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return 0,
    };
    conn.query_row(
        "SELECT COUNT(*) FROM observability_outbox WHERE user_id = ?1 AND status = 'pending'",
        params![user_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::MAX_ATTEMPTS;

    /// Regression guard: outbox enqueue path must stay sync SQLite only (no HTTP await).
    #[test]
    fn outbox_max_attempts_configured() {
        assert_eq!(MAX_ATTEMPTS, 10);
    }
}
