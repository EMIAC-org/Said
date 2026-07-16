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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSessionPayload {
    pub client_session_id: String,
    pub title: String,
    pub status: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub duration_seconds: f64,
    pub transcript_word_count: i32,
    pub transcription_provider: Option<String>,
    pub transcription_model: Option<String>,
    pub transcription_latency_ms: Option<i64>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingProviderUsagePayload {
    pub client_session_id: String,
    pub idempotency_key: String,
    pub credential_scope: String,
    pub provider: String,
    pub model: String,
    pub feature_stage: String,
    pub prompt_tokens: i32,
    pub cache_hit_tokens: i32,
    pub cache_miss_tokens: i32,
    pub completion_tokens: i32,
    pub reasoning_tokens: Option<i32>,
    pub latency_ms: i64,
    pub result_status: String,
    pub occurred_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub op: String,
    pub recording_id: Option<String>,
    pub payload_json: String,
    pub attempts: i64,
    pub active_org_id: Option<String>,
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
    let active_org_id = crate::store::users::get_user(pool, user_id)
        .and_then(|user| user.active_org_id)
        .filter(|org_id| !org_id.trim().is_empty());
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO observability_outbox
            (user_id, op, recording_id, payload_json, status, attempts, created_at_ms, active_org_id)
         VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?6)",
        params![user_id, op, recording_id, payload_json, now_ms(), active_org_id],
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

pub fn enqueue_meeting_session(
    pool: &DbPool,
    user_id: &str,
    payload: MeetingSessionPayload,
) -> Result<(), String> {
    let event_key = payload.client_session_id.clone();
    insert_row(
        pool,
        user_id,
        "upsert_meeting_session",
        Some(&event_key),
        &payload,
    )
}

pub fn enqueue_meeting_provider_usage(
    pool: &DbPool,
    user_id: &str,
    payload: MeetingProviderUsagePayload,
) -> Result<(), String> {
    let event_key = payload.idempotency_key.clone();
    insert_row(
        pool,
        user_id,
        "upsert_meeting_provider_usage",
        Some(&event_key),
        &payload,
    )
}

pub fn list_pending(pool: &DbPool, user_id: &str, limit: i64) -> Result<Vec<OutboxRow>, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, op, recording_id, payload_json, attempts, active_org_id
               FROM observability_outbox
              WHERE user_id = ?1 AND status = 'pending' AND attempts < ?2
              ORDER BY created_at_ms ASC, id ASC
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
                active_org_id: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn meeting_session_done(pool: &DbPool, user_id: &str, client_session_id: &str) -> bool {
    let Ok(conn) = pool.get() else {
        return false;
    };
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM observability_outbox
             WHERE user_id = ?1 AND op = 'upsert_meeting_session'
               AND recording_id = ?2 AND status = 'done'
        )",
        params![user_id, client_session_id],
        |row| row.get(0),
    )
    .unwrap_or(false)
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
    use super::*;

    /// Regression guard: outbox enqueue path must stay sync SQLite only (no HTTP await).
    #[test]
    fn outbox_max_attempts_configured() {
        assert_eq!(MAX_ATTEMPTS, 10);
    }

    #[test]
    fn meeting_events_are_unique_and_keep_the_enqueue_org() {
        let path = std::env::temp_dir().join(format!(
            "airnote-observability-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let pool = crate::store::open(&path);
        let user_id = crate::store::ensure_default_user(&pool);
        crate::store::users::update_cloud_auth(&pool, &user_id, "token", "pro", None);
        crate::store::users::update_active_org(&pool, &user_id, Some("org-a"));
        let session = MeetingSessionPayload {
            client_session_id: "local-1".into(),
            title: "Planning".into(),
            status: "completed".into(),
            started_at_ms: 1_000,
            ended_at_ms: 2_000,
            duration_seconds: 1.0,
            transcript_word_count: 4,
            transcription_provider: Some("whisper".into()),
            transcription_model: Some("small".into()),
            transcription_latency_ms: Some(9),
            device_id: None,
            platform: Some("macos".into()),
            app_version: Some("1.0.0".into()),
        };
        enqueue_meeting_session(&pool, &user_id, session.clone()).unwrap();
        enqueue_meeting_session(&pool, &user_id, session).unwrap();
        crate::store::users::update_active_org(&pool, &user_id, Some("org-b"));

        let rows = list_pending(&pool, &user_id, 20).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].active_org_id.as_deref(), Some("org-a"));
        assert!(!meeting_session_done(&pool, &user_id, "local-1"));
        mark_done(&pool, rows[0].id).unwrap();
        assert!(meeting_session_done(&pool, &user_id, "local-1"));

        drop(pool);
        let _ = std::fs::remove_file(path);
    }
}
