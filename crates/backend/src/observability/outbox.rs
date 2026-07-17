//! SQLite outbox for control-plane dictation observability (fire-and-forget).

use crate::store::{DbPool, history::InsertRecording};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

const MAX_ATTEMPTS: i64 = 10;
// Meeting delivery is durable across launches. Retry from 30 seconds with
// exponential spacing, capped at six hours so long outages do not create a
// network request every uploader tick. Keep list_pending_at's SQL CASE aligned.
const MEETING_RETRY_BASE_MS: i64 = 30_000;
const MEETING_RETRY_MAX_MS: i64 = 6 * 60 * 60 * 1_000;

fn is_meeting_op(op: &str) -> bool {
    matches!(
        op,
        "upsert_meeting_session" | "upsert_meeting_provider_usage"
    )
}

fn meeting_retry_delay_ms(attempts: i64) -> i64 {
    if attempts <= 0 {
        return 0;
    }
    let shift = (attempts - 1).clamp(0, 20) as u32;
    MEETING_RETRY_BASE_MS
        .saturating_mul(1_i64 << shift)
        .min(MEETING_RETRY_MAX_MS)
}

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
    let active_org_id = crate::store::users::get_user(pool, user_id)
        .and_then(|user| user.active_org_id)
        .filter(|org_id| !org_id.trim().is_empty());
    insert_row_with_org(
        pool,
        user_id,
        op,
        recording_id,
        payload,
        active_org_id.as_deref(),
    )
    .map(|_| ())
}

fn insert_row_with_org(
    pool: &DbPool,
    user_id: &str,
    op: &str,
    recording_id: Option<&str>,
    payload: &impl Serialize,
    active_org_id: Option<&str>,
) -> Result<bool, String> {
    let payload_json =
        serde_json::to_string(payload).map_err(|e| format!("serialize outbox payload: {e}"))?;
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO observability_outbox
            (user_id, op, recording_id, payload_json, status, attempts, created_at_ms, active_org_id)
         VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?6)",
        params![user_id, op, recording_id, payload_json, now_ms(), active_org_id],
    )
    .map(|changed| changed > 0)
    .map_err(|e| e.to_string())
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

/// Queue a meeting session in the workspace that was active when recording
/// started. Unlike the generic enqueue path, this must never look up the user's
/// current workspace: an offline meeting can be scanned after a workspace switch.
pub fn enqueue_meeting_session_for_org(
    pool: &DbPool,
    user_id: &str,
    payload: MeetingSessionPayload,
    origin_org_id: &str,
) -> Result<bool, String> {
    let origin_org_id = origin_org_id.trim();
    if origin_org_id.is_empty() || origin_org_id.len() > 128 {
        return Err("meeting session origin workspace is empty".to_string());
    }
    let event_key = payload.client_session_id.clone();
    insert_row_with_org(
        pool,
        user_id,
        "upsert_meeting_session",
        Some(&event_key),
        &payload,
        Some(origin_org_id),
    )
}

pub fn enqueue_meeting_provider_usage(
    pool: &DbPool,
    user_id: &str,
    payload: MeetingProviderUsagePayload,
) -> Result<(), String> {
    enqueue_meeting_provider_usage_with_outcome(pool, user_id, payload).map(|_| ())
}

/// Queue provider usage and report whether scanning added work that still needs
/// uploading. The usage inherits the parent session's immutable workspace.
pub fn enqueue_meeting_provider_usage_with_outcome(
    pool: &DbPool,
    user_id: &str,
    payload: MeetingProviderUsagePayload,
) -> Result<bool, String> {
    let event_key = payload.idempotency_key.clone();
    let parent_org =
        meeting_session_org(pool, user_id, &payload.client_session_id)?.ok_or_else(|| {
            "meeting usage has no parent session outbox row with an active org".to_string()
        })?;
    let inserted = insert_row_with_org(
        pool,
        user_id,
        "upsert_meeting_provider_usage",
        Some(&event_key),
        &payload,
        Some(&parent_org),
    )?;

    // Repair a pending row created by an older scanner after an org switch.
    // INSERT OR IGNORE preserves event idempotency, while this update restores
    // the immutable org ownership inherited from the parent meeting session.
    let conn = pool.get().map_err(|e| e.to_string())?;
    let repaired = conn
        .execute(
            "UPDATE observability_outbox
            SET active_org_id = ?3
          WHERE user_id = ?1 AND op = 'upsert_meeting_provider_usage'
            AND recording_id = ?2 AND status != 'done' AND active_org_id IS NOT ?3",
            params![user_id, event_key, parent_org],
        )
        .map_err(|e| e.to_string())?
        > 0;
    Ok(inserted || repaired)
}

pub fn list_pending(pool: &DbPool, user_id: &str, limit: i64) -> Result<Vec<OutboxRow>, String> {
    list_pending_at(pool, user_id, limit, now_ms())
}

fn list_pending_at(
    pool: &DbPool,
    user_id: &str,
    limit: i64,
    eligible_at_ms: i64,
) -> Result<Vec<OutboxRow>, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let delivered_sessions = {
        let mut stmt = conn
            .prepare(
                "SELECT recording_id, active_org_id
                   FROM observability_outbox
                  WHERE user_id = ?1 AND op = 'upsert_meeting_session'
                    AND status = 'done' AND recording_id IS NOT NULL
                    AND active_org_id IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(|e| e.to_string())?
    };
    let mut stmt = conn
        .prepare(
            "SELECT id, op, recording_id, payload_json, attempts, active_org_id
               FROM observability_outbox
              WHERE user_id = ?1
                AND (
                    (status = 'pending'
                        AND op NOT IN ('upsert_meeting_session', 'upsert_meeting_provider_usage')
                        AND attempts < ?2)
                    OR
                    (status IN ('pending', 'dropped')
                        AND op IN ('upsert_meeting_session', 'upsert_meeting_provider_usage')
                        AND (
                            last_attempt_ms IS NULL
                            OR last_attempt_ms + CASE
                                WHEN attempts <= 0 THEN 0
                                WHEN attempts = 1 THEN 30000
                                WHEN attempts = 2 THEN 60000
                                WHEN attempts = 3 THEN 120000
                                WHEN attempts = 4 THEN 240000
                                WHEN attempts = 5 THEN 480000
                                WHEN attempts = 6 THEN 960000
                                WHEN attempts = 7 THEN 1920000
                                WHEN attempts = 8 THEN 3840000
                                WHEN attempts = 9 THEN 7680000
                                WHEN attempts = 10 THEN 15360000
                                ELSE 21600000
                            END <= ?3
                        )
                    )
                )
              ORDER BY created_at_ms ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, MAX_ATTEMPTS, eligible_at_ms], |row| {
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
    let candidates = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(candidates
        .into_iter()
        .filter(|row| {
            if row.op != "upsert_meeting_provider_usage" {
                return true;
            }
            let Ok(payload) =
                serde_json::from_str::<MeetingProviderUsagePayload>(&row.payload_json)
            else {
                // Keep malformed rows visible so the uploader can account for
                // the failed attempt rather than silently hiding them forever.
                return true;
            };
            delivered_sessions
                .get(&payload.client_session_id)
                .map(String::as_str)
                == row.active_org_id.as_deref()
        })
        .take(limit.max(0) as usize)
        .collect())
}

fn meeting_session_org(
    pool: &DbPool,
    user_id: &str,
    client_session_id: &str,
) -> Result<Option<String>, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT active_org_id FROM observability_outbox
          WHERE user_id = ?1 AND op = 'upsert_meeting_session'
            AND recording_id = ?2",
        params![user_id, client_session_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .map(|org| org.flatten().filter(|value| !value.trim().is_empty()))
    .map_err(|error| error.to_string())
}

pub fn meeting_session_done_for_org(
    pool: &DbPool,
    user_id: &str,
    client_session_id: &str,
    active_org_id: Option<&str>,
) -> bool {
    let Some(active_org_id) = active_org_id.filter(|value| !value.trim().is_empty()) else {
        return false;
    };
    let Ok(conn) = pool.get() else {
        return false;
    };
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM observability_outbox
             WHERE user_id = ?1 AND op = 'upsert_meeting_session'
               AND recording_id = ?2 AND status = 'done' AND active_org_id = ?3
        )",
        params![user_id, client_session_id, active_org_id],
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
    let (op, attempts): (String, i64) = conn
        .query_row(
            "SELECT op, attempts FROM observability_outbox WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or_else(|_| (String::new(), 0));
    let next = attempts + 1;
    let meeting = is_meeting_op(&op);
    let status = if !meeting && next >= MAX_ATTEMPTS {
        "dropped"
    } else {
        "pending"
    };
    if status == "dropped" {
        warn!("[observability] dropping outbox row {id} after {next} attempts: {error}");
    } else if meeting {
        let retry_delay_ms = meeting_retry_delay_ms(next);
        warn!(
            "[observability] meeting outbox row {id} attempt {next} failed; retrying in {retry_delay_ms}ms: {error}"
        );
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
        "SELECT COUNT(*) FROM observability_outbox
          WHERE user_id = ?1
            AND (
                status = 'pending'
                OR (status = 'dropped' AND op IN (
                    'upsert_meeting_session', 'upsert_meeting_provider_usage'
                ))
            )",
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
        assert_eq!(meeting_retry_delay_ms(0), 0);
        assert_eq!(meeting_retry_delay_ms(1), 30_000);
        assert_eq!(meeting_retry_delay_ms(2), 60_000);
        assert_eq!(meeting_retry_delay_ms(100), MEETING_RETRY_MAX_MS);
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
        assert!(!meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-1",
            Some("org-a")
        ));
        mark_done(&pool, rows[0].id).unwrap();
        assert!(meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-1",
            Some("org-a")
        ));
        enqueue_meeting_provider_usage(
            &pool,
            &user_id,
            MeetingProviderUsagePayload {
                client_session_id: "local-1".into(),
                idempotency_key: "meeting:local-1:summary:1".into(),
                credential_scope: "airnote_bundled".into(),
                provider: "deepseek".into(),
                model: "deepseek-chat".into(),
                feature_stage: "summary".into(),
                prompt_tokens: 10,
                cache_hit_tokens: 0,
                cache_miss_tokens: 10,
                completion_tokens: 5,
                reasoning_tokens: None,
                latency_ms: 25,
                result_status: "success".into(),
                occurred_at_ms: 2_000,
            },
        )
        .unwrap();
        let rows = list_pending(&pool, &user_id, 20).unwrap();
        let usage = rows
            .iter()
            .find(|row| row.op == "upsert_meeting_provider_usage")
            .unwrap();
        assert_eq!(usage.active_org_id.as_deref(), Some("org-a"));
        assert!(meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-1",
            usage.active_org_id.as_deref()
        ));
        assert!(!meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-1",
            Some("org-b")
        ));

        drop(pool);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn meeting_session_survives_ten_failures_and_usage_resumes_after_parent() {
        let path = std::env::temp_dir().join(format!(
            "airnote-observability-retry-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let pool = crate::store::open(&path);
        let user_id = crate::store::ensure_default_user(&pool);
        crate::store::users::update_cloud_auth(&pool, &user_id, "token", "pro", None);
        crate::store::users::update_active_org(&pool, &user_id, Some("org-retry"));

        enqueue_meeting_session(
            &pool,
            &user_id,
            MeetingSessionPayload {
                client_session_id: "local-retry".into(),
                title: "Retry Planning".into(),
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
            },
        )
        .unwrap();
        enqueue_meeting_provider_usage(
            &pool,
            &user_id,
            MeetingProviderUsagePayload {
                client_session_id: "local-retry".into(),
                idempotency_key: "meeting:local-retry:cleanup:1".into(),
                credential_scope: "airnote_bundled".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                feature_stage: "transcript_cleanup".into(),
                prompt_tokens: 10,
                cache_hit_tokens: 4,
                cache_miss_tokens: 6,
                completion_tokens: 2,
                reasoning_tokens: None,
                latency_ms: 50,
                result_status: "success".into(),
                occurred_at_ms: 1_500,
            },
        )
        .unwrap();

        let rows = list_pending_at(&pool, &user_id, 20, now_ms()).unwrap();
        let session_id = rows
            .iter()
            .find(|row| row.op == "upsert_meeting_session")
            .unwrap()
            .id;
        for attempt in 1..=12 {
            mark_failed(&pool, session_id, &format!("offline {attempt}")).unwrap();
        }

        let (status, attempts, stored_org): (String, i64, Option<String>) = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT status, attempts, active_org_id FROM observability_outbox WHERE id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(attempts, 12);
        assert_eq!(stored_org.as_deref(), Some("org-retry"));
        assert!(!meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-retry",
            Some("org-retry")
        ));

        // Simulate a row already dropped by the pre-amendment build. Durable
        // meeting eligibility must revive it without relying on INSERT OR IGNORE.
        pool.get()
            .unwrap()
            .execute(
                "UPDATE observability_outbox SET status = 'dropped' WHERE id = ?1",
                params![session_id],
            )
            .unwrap();
        assert!(pending_count(&pool, &user_id) >= 2);

        let before_backoff = list_pending_at(&pool, &user_id, 20, now_ms()).unwrap();
        assert!(!before_backoff.iter().any(|row| row.id == session_id));
        let after_backoff = list_pending_at(
            &pool,
            &user_id,
            20,
            now_ms().saturating_add(MEETING_RETRY_MAX_MS + 1),
        )
        .unwrap();
        assert!(after_backoff.iter().any(|row| row.id == session_id));

        // The uploader uses this exact gate: usage waits while the parent is
        // pending, then becomes deliverable immediately after acknowledgement.
        assert!(!meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-retry",
            Some("org-retry")
        ));
        mark_done(&pool, session_id).unwrap();
        assert!(meeting_session_done_for_org(
            &pool,
            &user_id,
            "local-retry",
            Some("org-retry")
        ));
        assert!(
            !after_backoff
                .iter()
                .any(|row| row.op == "upsert_meeting_provider_usage")
        );
        let after_parent = list_pending_at(&pool, &user_id, 20, i64::MAX).unwrap();
        assert!(
            after_parent
                .iter()
                .any(|row| row.op == "upsert_meeting_provider_usage")
        );

        drop(pool);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn blocked_meeting_usage_does_not_consume_the_pending_batch() {
        let path = std::env::temp_dir().join(format!(
            "airnote-observability-hol-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let pool = crate::store::open(&path);
        let user_id = crate::store::ensure_default_user(&pool);
        crate::store::users::update_cloud_auth(&pool, &user_id, "token", "pro", None);
        crate::store::users::update_active_org(&pool, &user_id, Some("org-a"));
        enqueue_meeting_session(
            &pool,
            &user_id,
            MeetingSessionPayload {
                client_session_id: "blocked-parent".into(),
                title: "Offline meeting".into(),
                status: "completed".into(),
                started_at_ms: 1_000,
                ended_at_ms: 2_000,
                duration_seconds: 1.0,
                transcript_word_count: 0,
                transcription_provider: None,
                transcription_model: None,
                transcription_latency_ms: None,
                device_id: None,
                platform: None,
                app_version: None,
            },
        )
        .unwrap();
        let parent_id: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM observability_outbox WHERE recording_id = 'blocked-parent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        mark_failed(&pool, parent_id, "offline").unwrap();
        for index in 0..25 {
            enqueue_meeting_provider_usage(
                &pool,
                &user_id,
                MeetingProviderUsagePayload {
                    client_session_id: "blocked-parent".into(),
                    idempotency_key: format!("meeting:blocked-parent:chat:{index}"),
                    credential_scope: "airnote_bundled".into(),
                    provider: "deepseek".into(),
                    model: "deepseek-chat".into(),
                    feature_stage: "chat".into(),
                    prompt_tokens: 1,
                    cache_hit_tokens: 0,
                    cache_miss_tokens: 1,
                    completion_tokens: 1,
                    reasoning_tokens: None,
                    latency_ms: 1,
                    result_status: "success".into(),
                    occurred_at_ms: 2_000 + index,
                },
            )
            .unwrap();
        }
        insert_row(
            &pool,
            &user_id,
            "generic_test",
            Some("new-unrelated-work"),
            &serde_json::json!({ "safe": true }),
        )
        .unwrap();

        let rows = list_pending_at(&pool, &user_id, 20, now_ms()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].op, "generic_test");

        drop(pool);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn generic_rows_still_drop_after_ten_failures() {
        let path = std::env::temp_dir().join(format!(
            "airnote-observability-generic-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let pool = crate::store::open(&path);
        let user_id = crate::store::ensure_default_user(&pool);
        insert_row(
            &pool,
            &user_id,
            "generic_test",
            Some("generic-1"),
            &serde_json::json!({ "safe": true }),
        )
        .unwrap();
        let row = list_pending_at(&pool, &user_id, 20, now_ms())
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        for attempt in 1..=MAX_ATTEMPTS {
            mark_failed(&pool, row.id, &format!("offline {attempt}")).unwrap();
        }
        let status: String = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT status FROM observability_outbox WHERE id = ?1",
                params![row.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "dropped");
        assert_eq!(pending_count(&pool, &user_id), 0);
        assert!(
            list_pending_at(&pool, &user_id, 20, i64::MAX)
                .unwrap()
                .is_empty()
        );
        drop(pool);
        let _ = std::fs::remove_file(path);
    }
}
