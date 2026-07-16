//! Metadata-only telemetry artifacts for local meetings.
//!
//! This module deliberately never serializes meeting content. The local backend
//! scans `meeting.telemetry.json` and owns authenticated control-plane delivery.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "meeting.telemetry.json";
const SCHEMA_VERSION: u8 = 1;
const BUNDLED_SCOPE: &str = "airnote_bundled";

fn file_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionPayload {
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderUsagePayload {
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionDraft {
    client_session_id: String,
    started_at_ms: i64,
    ended_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TelemetryArtifact {
    schema_version: u8,
    draft: SessionDraft,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<SessionPayload>,
    #[serde(default)]
    provider_usage: Vec<ProviderUsagePayload>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeepSeekUsage {
    pub prompt_tokens: i32,
    pub cache_hit_tokens: i32,
    pub cache_miss_tokens: i32,
    pub completion_tokens: i32,
    pub reasoning_tokens: Option<i32>,
}

pub struct CallGuard {
    artifact_dir: Option<PathBuf>,
    client_session_id: Option<String>,
    idempotency_key: String,
    feature_stage: String,
    provider: String,
    model: String,
    started: std::time::Instant,
    completed: bool,
}

impl CallGuard {
    pub fn new(
        artifact_dir: Option<&Path>,
        feature_stage: &str,
        provider: &str,
        model: &str,
        bundled_credential: bool,
    ) -> Self {
        let eligible = bundled_credential && provider == "deepseek" && model == "deepseek-v4-pro";
        let artifact_dir = eligible
            .then(|| artifact_dir.map(Path::to_path_buf))
            .flatten()
            .filter(|dir| dir.join(FILE_NAME).is_file());
        let client_session_id = artifact_dir
            .as_deref()
            .and_then(read_artifact)
            .map(|artifact| artifact.draft.client_session_id);
        let idempotency_key = client_session_id
            .as_deref()
            .map_or_else(String::new, |session_id| {
                format!(
                    "meeting:{session_id}:{feature_stage}:{}",
                    uuid::Uuid::new_v4()
                )
            });
        Self {
            artifact_dir,
            client_session_id,
            idempotency_key,
            feature_stage: feature_stage.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            started: std::time::Instant::now(),
            completed: false,
        }
    }

    pub fn success(mut self, usage: DeepSeekUsage) {
        self.completed = true;
        self.persist("success", usage);
    }

    fn persist(&self, result_status: &str, usage: DeepSeekUsage) {
        let (Some(dir), Some(client_session_id)) = (
            self.artifact_dir.as_deref(),
            self.client_session_id.as_deref(),
        ) else {
            return;
        };
        let payload = ProviderUsagePayload {
            client_session_id: client_session_id.to_string(),
            idempotency_key: self.idempotency_key.clone(),
            credential_scope: BUNDLED_SCOPE.to_string(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            feature_stage: self.feature_stage.clone(),
            prompt_tokens: usage.prompt_tokens,
            cache_hit_tokens: usage.cache_hit_tokens,
            cache_miss_tokens: usage.cache_miss_tokens,
            completion_tokens: usage.completion_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            latency_ms: self.started.elapsed().as_millis().min(i64::MAX as u128) as i64,
            result_status: result_status.to_string(),
            occurred_at_ms: now_ms(),
        };
        append_usage(dir, payload);
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.persist("error", DeepSeekUsage::default());
        }
    }
}

pub fn begin_session(
    artifact_dir: &Path,
    client_session_id: &str,
    started_at_ms: u64,
    ended_at_ms: u64,
) {
    if !artifact_dir.is_dir() {
        return;
    }
    let _guard = file_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut artifact = read_artifact(artifact_dir).unwrap_or(TelemetryArtifact {
        schema_version: SCHEMA_VERSION,
        draft: SessionDraft {
            client_session_id: client_session_id.to_string(),
            started_at_ms: clamp_ms(started_at_ms),
            ended_at_ms: clamp_ms(ended_at_ms.max(started_at_ms)),
        },
        session: None,
        provider_usage: Vec::new(),
    });
    if artifact.draft.client_session_id != client_session_id || artifact.session.is_some() {
        return;
    }
    artifact.draft.started_at_ms = clamp_ms(started_at_ms);
    artifact.draft.ended_at_ms = clamp_ms(ended_at_ms.max(started_at_ms));
    if let Err(error) = write_artifact(artifact_dir, &artifact) {
        tracing::warn!(error = %error, dir = %artifact_dir.display(), "[meeting_telemetry] failed to start artifact");
    }
}

pub fn finalize_session(artifact_dir: &Path, status: &str) {
    let _guard = file_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some(mut artifact) = read_artifact(artifact_dir) else {
        return;
    };
    if artifact.session.is_some() {
        return;
    }

    let transcript = read_json(artifact_dir.join("meeting.transcript.json"));
    let intelligence = read_json(artifact_dir.join("meeting.ai.json"));
    let transcript_text = transcript
        .as_ref()
        .and_then(|value| {
            value
                .get("cleaned_transcript")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            transcript
                .as_ref()
                .and_then(|value| value.get("transcript").and_then(|value| value.as_str()))
        })
        .unwrap_or("");
    let title = intelligence
        .as_ref()
        .and_then(|value| value.get("title").and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Untitled meeting")
        .chars()
        .take(200)
        .collect::<String>();
    let provider = json_string(&transcript, "provider");
    let model = json_string(&transcript, "model").map(|value| stable_model_identifier(&value));
    let latency_ms = json_i64(&transcript, "latency_ms");
    if artifact.draft.ended_at_ms <= artifact.draft.started_at_ms {
        if let Some(duration_ms) = json_i64(&transcript, "audio_duration_ms") {
            artifact.draft.ended_at_ms = artifact.draft.started_at_ms.saturating_add(duration_ms);
        }
    }
    let duration_seconds = artifact
        .draft
        .ended_at_ms
        .saturating_sub(artifact.draft.started_at_ms) as f64
        / 1_000.0;
    artifact.session = Some(SessionPayload {
        client_session_id: artifact.draft.client_session_id.clone(),
        title,
        status: status.to_string(),
        started_at_ms: artifact.draft.started_at_ms,
        ended_at_ms: artifact.draft.ended_at_ms,
        duration_seconds,
        transcript_word_count: transcript_text
            .split_whitespace()
            .count()
            .min(i32::MAX as usize) as i32,
        transcription_provider: provider,
        transcription_model: model,
        transcription_latency_ms: latency_ms,
        device_id: Some(said_core::paths::device_id()),
        platform: Some(std::env::consts::OS.to_string()),
        app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    });
    if let Err(error) = write_artifact(artifact_dir, &artifact) {
        tracing::warn!(error = %error, dir = %artifact_dir.display(), "[meeting_telemetry] failed to finalize artifact");
    }
}

/// Refresh a crash-recovered draft from the durable WAV-derived duration before
/// processing resumes. This is metadata only; no audio or transcript content is
/// copied into the telemetry artifact.
pub fn refresh_recovered_duration(artifact_dir: &Path, duration_ms: u64) {
    let _guard = file_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some(mut artifact) = read_artifact(artifact_dir) else {
        return;
    };
    if artifact.session.is_some() {
        return;
    }
    let recovered_end = artifact
        .draft
        .started_at_ms
        .saturating_add(clamp_ms(duration_ms));
    if recovered_end <= artifact.draft.ended_at_ms {
        return;
    }
    artifact.draft.ended_at_ms = recovered_end;
    if let Err(error) = write_artifact(artifact_dir, &artifact) {
        tracing::warn!(error = %error, dir = %artifact_dir.display(), "[meeting_telemetry] failed to refresh recovered duration");
    }
}

pub(crate) fn stable_model_identifier(value: &str) -> String {
    let trimmed = value.trim();
    let basename = trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("unknown-local-model");
    let mut end = basename.len().min(128);
    while !basename.is_char_boundary(end) {
        end -= 1;
    }
    basename[..end].to_string()
}

fn append_usage(artifact_dir: &Path, payload: ProviderUsagePayload) {
    let _guard = file_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some(mut artifact) = read_artifact(artifact_dir) else {
        return;
    };
    if artifact
        .provider_usage
        .iter()
        .any(|item| item.idempotency_key == payload.idempotency_key)
    {
        return;
    }
    artifact.provider_usage.push(payload);
    if let Err(error) = write_artifact(artifact_dir, &artifact) {
        tracing::warn!(error = %error, dir = %artifact_dir.display(), "[meeting_telemetry] failed to append usage");
    }
}

fn read_artifact(artifact_dir: &Path) -> Option<TelemetryArtifact> {
    let bytes = fs::read(artifact_dir.join(FILE_NAME)).ok()?;
    let artifact: TelemetryArtifact = serde_json::from_slice(&bytes).ok()?;
    (artifact.schema_version == SCHEMA_VERSION).then_some(artifact)
}

fn write_artifact(artifact_dir: &Path, artifact: &TelemetryArtifact) -> Result<(), String> {
    let path = artifact_dir.join(FILE_NAME);
    let temp = artifact_dir.join(format!("{FILE_NAME}.tmp"));
    let bytes = serde_json::to_vec_pretty(artifact).map_err(|error| error.to_string())?;
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp, &path).map_err(|error| error.to_string())
}

fn read_json(path: PathBuf) -> Option<serde_json::Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn json_string(value: &Option<serde_json::Value>, field: &str) -> Option<String> {
    value
        .as_ref()
        .and_then(|value| value.get(field))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn json_i64(value: &Option<serde_json::Value>, field: &str) -> Option<i64> {
    value
        .as_ref()
        .and_then(|value| value.get(field))
        .and_then(|value| value.as_i64())
        .filter(|value| *value >= 0)
}

fn clamp_ms(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_contains_metadata_only() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-telemetry-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        begin_session(&dir, "local-123", 1_000, 4_500);
        finalize_session(&dir, "completed");
        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("\"client_session_id\": \"local-123\""));
        assert!(text.contains("\"duration_seconds\": 3.5"));
        for forbidden in ["raw_transcript", "summary", "action_items", "chat", "audio"] {
            assert!(
                !text.contains(forbidden),
                "unexpected content field: {forbidden}"
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finalized_model_is_basename_and_recovery_uses_audio_duration() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-telemetry-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        begin_session(&dir, "local-recovered", 10_000, 10_000);
        fs::write(
            dir.join("meeting.transcript.json"),
            serde_json::to_vec(&serde_json::json!({
                "provider": "whisper.cpp",
                "model": "/Users/private/Library/Application Support/AirNote/models/ggml-small.bin",
                "audio_duration_ms": 3_250,
                "transcript": "private meeting words"
            }))
            .unwrap(),
        )
        .unwrap();
        refresh_recovered_duration(&dir, 3_250);
        finalize_session(&dir, "completed");

        let artifact = read_artifact(&dir).unwrap();
        let session = artifact.session.unwrap();
        assert_eq!(session.ended_at_ms, 13_250);
        assert_eq!(session.duration_seconds, 3.25);
        assert_eq!(
            session.transcription_model.as_deref(),
            Some("ggml-small.bin")
        );
        let serialized = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!serialized.contains("/Users/private"));
        assert!(!serialized.contains("private meeting words"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn call_guard_ignores_non_bundled_credentials() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-telemetry-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        begin_session(&dir, "local-456", 1_000, 2_000);
        CallGuard::new(
            Some(&dir),
            "meeting_cleanup",
            "deepseek",
            "deepseek-v4-pro",
            false,
        )
        .success(DeepSeekUsage::default());
        let artifact = read_artifact(&dir).unwrap();
        assert!(artifact.provider_usage.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn call_guard_persists_success_and_terminal_error() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-telemetry-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        begin_session(&dir, "local-789", 1_000, 2_000);
        CallGuard::new(
            Some(&dir),
            "meeting_intelligence",
            "deepseek",
            "deepseek-v4-pro",
            true,
        )
        .success(DeepSeekUsage {
            prompt_tokens: 10,
            cache_hit_tokens: 4,
            cache_miss_tokens: 6,
            completion_tokens: 3,
            reasoning_tokens: Some(1),
        });
        drop(CallGuard::new(
            Some(&dir),
            "meeting_intelligence_verify",
            "deepseek",
            "deepseek-v4-pro",
            true,
        ));

        let artifact = read_artifact(&dir).unwrap();
        assert_eq!(artifact.provider_usage.len(), 2);
        assert_eq!(artifact.provider_usage[0].result_status, "success");
        assert_eq!(artifact.provider_usage[0].cache_hit_tokens, 4);
        assert_eq!(artifact.provider_usage[1].result_status, "error");
        assert_eq!(artifact.provider_usage[1].prompt_tokens, 0);
        assert_ne!(
            artifact.provider_usage[0].idempotency_key,
            artifact.provider_usage[1].idempotency_key
        );
        let _ = fs::remove_dir_all(dir);
    }
}
