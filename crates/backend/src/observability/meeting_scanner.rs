//! Scanner for metadata-only meeting telemetry artifacts produced by Tauri.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use tracing::warn;

use super::outbox::{
    MeetingProviderUsagePayload, MeetingSessionPayload,
    enqueue_meeting_provider_usage_with_outcome, enqueue_meeting_session_for_org,
};
use crate::store::DbPool;

const FILE_NAME: &str = "meeting.telemetry.json";
const SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Deserialize)]
struct TelemetryArtifact {
    schema_version: u8,
    #[serde(default)]
    origin_org_id: Option<String>,
    session: Option<MeetingSessionPayload>,
    #[serde(default)]
    provider_usage: Vec<MeetingProviderUsagePayload>,
}

pub fn scan_and_enqueue(pool: &DbPool, user_id: &str) -> Result<usize, String> {
    scan_artifacts(
        pool,
        user_id,
        &said_core::paths::data_dir().join("meetings"),
    )
}

fn scan_artifacts(pool: &DbPool, user_id: &str, meetings_dir: &Path) -> Result<usize, String> {
    let Some(user) = crate::store::users::get_user(pool, user_id) else {
        return Ok(0);
    };
    if user
        .cloud_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
    {
        return Ok(0);
    }

    // Artifacts written before origin_org_id existed must retain their historical
    // behavior. New artifacts always use their write-once origin instead of this
    // fallback, even when the user has since activated another workspace.
    let legacy_active_org = user
        .active_org_id
        .filter(|org_id| !org_id.trim().is_empty());
    let entries = match fs::read_dir(meetings_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    let mut enqueued = 0usize;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path().join(FILE_NAME);
        if !path.is_file() {
            continue;
        }
        let artifact: TelemetryArtifact = match fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<TelemetryArtifact>(&bytes)
                    .map_err(|error| error.to_string())
            }) {
            Ok(artifact) if artifact.schema_version == SCHEMA_VERSION => artifact,
            Ok(_) => continue,
            Err(error) => {
                warn!(error = %error, path = %path.display(), "[meeting_telemetry] skipped unreadable artifact");
                continue;
            }
        };
        let Some(session) = artifact.session else {
            // Usage may already be present while final transcription/summary is
            // still running. Queue nothing until the immutable session payload
            // is final so the parent always uploads first.
            continue;
        };
        let origin_org_id = artifact
            .origin_org_id
            .as_deref()
            .map(str::trim)
            .filter(|org_id| !org_id.is_empty() && org_id.len() <= 128)
            .or(legacy_active_org.as_deref());
        let Some(origin_org_id) = origin_org_id else {
            continue;
        };
        if enqueue_meeting_session_for_org(pool, user_id, session, origin_org_id)? {
            enqueued += 1;
        }
        for usage in artifact.provider_usage {
            if enqueue_meeting_provider_usage_with_outcome(pool, user_id, usage)? {
                enqueued += 1;
            }
        }
    }
    Ok(enqueued)
}

pub fn spawn_scanner(pool: DbPool, user_id: String, http: reqwest::Client) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            match scan_and_enqueue(&pool, &user_id) {
                Ok(count) if count > 0 => {
                    super::uploader::maybe_upload_after_enqueue(&pool, &user_id, &http);
                }
                Ok(_) => {}
                Err(error) => warn!(error = %error, "[meeting_telemetry] artifact scan failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn telemetry_schema_rejects_content_fields_from_payload_types() {
        let value = serde_json::json!({
            "client_session_id": "local-1",
            "title": "Planning",
            "status": "completed",
            "started_at_ms": 1,
            "ended_at_ms": 2,
            "duration_seconds": 0.001,
            "transcript_word_count": 4,
            "transcription_provider": "whisper",
            "transcription_model": "small",
            "transcription_latency_ms": 9,
            "device_id": null,
            "platform": "macos",
            "app_version": "1.0.0"
        });
        let payload: MeetingSessionPayload = serde_json::from_value(value).unwrap();
        let serialized = serde_json::to_string(&payload).unwrap();
        for forbidden in ["transcript_text", "summary", "audio", "chat"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn scanner_keeps_artifact_workspace_across_a_switch_and_acks_completed_rows() {
        let root =
            std::env::temp_dir().join(format!("airnote-meeting-scanner-{}", uuid::Uuid::new_v4()));
        let meeting_dir = root.join("meeting-1");
        fs::create_dir_all(&meeting_dir).unwrap();
        let session = MeetingSessionPayload {
            client_session_id: "meeting-1".into(),
            title: "Planning".into(),
            status: "completed".into(),
            started_at_ms: 1_000,
            ended_at_ms: 2_000,
            duration_seconds: 1.0,
            transcript_word_count: 4,
            transcription_provider: Some("whisper.cpp".into()),
            transcription_model: Some("ggml-small.bin".into()),
            transcription_latency_ms: Some(50),
            device_id: None,
            platform: Some("macos".into()),
            app_version: Some("1.0.0".into()),
        };
        let usage = MeetingProviderUsagePayload {
            client_session_id: "meeting-1".into(),
            idempotency_key: "meeting:meeting-1:summary:1".into(),
            credential_scope: "airnote_bundled".into(),
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            feature_stage: "summary".into(),
            prompt_tokens: 10,
            cache_hit_tokens: 4,
            cache_miss_tokens: 6,
            completion_tokens: 2,
            reasoning_tokens: None,
            latency_ms: 25,
            result_status: "success".into(),
            occurred_at_ms: 2_000,
        };
        fs::write(
            meeting_dir.join(FILE_NAME),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "origin_org_id": "org-a",
                "session": session,
                "provider_usage": [usage],
            }))
            .unwrap(),
        )
        .unwrap();

        let db_path = root.join("backend.sqlite");
        let pool = crate::store::open(&db_path);
        let user_id = crate::store::ensure_default_user(&pool);
        crate::store::users::update_cloud_auth(&pool, &user_id, "token", "pro", None);
        // Simulate the user switching workspaces before the scanner first sees
        // the completed artifact.
        crate::store::users::update_active_org(&pool, &user_id, Some("org-b"));

        assert_eq!(scan_artifacts(&pool, &user_id, &root).unwrap(), 2);
        let rows: Vec<(String, String)> = pool
            .get()
            .unwrap()
            .prepare("SELECT op, active_org_id FROM observability_outbox ORDER BY id ASC")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, org_id)| org_id == "org-a"));

        // A second scanner pass sees the durable artifact but no longer reports
        // it as newly queued work, avoiding an unnecessary upload trigger.
        assert_eq!(scan_artifacts(&pool, &user_id, &root).unwrap(), 0);
        let outbox_count: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM observability_outbox",
                params![],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outbox_count, 2);

        drop(pool);
        let _ = fs::remove_dir_all(root);
    }
}
