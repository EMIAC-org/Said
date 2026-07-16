//! Scanner for metadata-only meeting telemetry artifacts produced by Tauri.

use std::fs;

use serde::Deserialize;
use tracing::warn;

use super::outbox::{
    MeetingProviderUsagePayload, MeetingSessionPayload, enqueue_meeting_provider_usage,
    enqueue_meeting_session,
};
use crate::store::DbPool;

const FILE_NAME: &str = "meeting.telemetry.json";
const SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Deserialize)]
struct TelemetryArtifact {
    schema_version: u8,
    session: Option<MeetingSessionPayload>,
    #[serde(default)]
    provider_usage: Vec<MeetingProviderUsagePayload>,
}

pub fn scan_and_enqueue(pool: &DbPool, user_id: &str) -> Result<usize, String> {
    let Some(user) = crate::store::users::get_user(pool, user_id) else {
        return Ok(0);
    };
    if user
        .cloud_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
        || user
            .active_org_id
            .as_deref()
            .is_none_or(|org_id| org_id.trim().is_empty())
    {
        return Ok(0);
    }

    let meetings_dir = said_core::paths::data_dir().join("meetings");
    let entries = match fs::read_dir(&meetings_dir) {
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
        enqueue_meeting_session(pool, user_id, session)?;
        enqueued += 1;
        for usage in artifact.provider_usage {
            enqueue_meeting_provider_usage(pool, user_id, usage)?;
            enqueued += 1;
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
}
