//! Hand off edit evidence to control-plane `POST /v1/runtime/profile/learn-from-edit`.
//!
//! Canonical profile learning path — does not touch legacy `personal_*` writers or
//! `client-events` / `confirm-batch` promotion flows.

use tracing::{info, warn};

use crate::{AppState, store::users};

/// Payload aligned with control-plane `LearnFromEditRequest`.
#[derive(Debug, Clone)]
pub struct ProfileLearnHandoff {
    pub edit_event_id: String,
    pub recording_id: String,
    pub raw_transcript: Option<String>,
    pub ai_output: String,
    pub user_kept: String,
    pub target_app: Option<String>,
    pub output_language: Option<String>,
    pub model_used: Option<String>,
    pub capture_confidence: Option<String>,
    pub client_run_id: Option<String>,
}

pub fn capture_confidence_label(capture_method: &str) -> &'static str {
    match capture_method {
        "ax" | "keystroke_verified" => "high",
        "clipboard" => "medium",
        "keystroke_only" => "low",
        _ => "medium",
    }
}

fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "desktop"
    }
}

/// Fire-and-forget: queue profile learn job on the enterprise control plane.
pub fn post_profile_learn_from_edit(state: AppState, payload: ProfileLearnHandoff) {
    tokio::spawn(async move {
        let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
            return;
        };
        let Some(token) = user.cloud_token.filter(|t| !t.trim().is_empty()) else {
            info!(
                "[profile-handoff] skipped edit_event={} client_run_id={} — no cloud token",
                payload.edit_event_id,
                payload.client_run_id.as_deref().unwrap_or("none"),
            );
            return;
        };
        let base_url = user
            .enterprise_server_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
        let url = format!(
            "{}/v1/runtime/profile/learn-from-edit",
            base_url.trim_end_matches('/')
        );
        let mut body = serde_json::json!({
            "edit_event_id": payload.edit_event_id,
            "recording_id": payload.recording_id,
            "raw_transcript": payload.raw_transcript,
            "ai_output": payload.ai_output,
            "user_kept": payload.user_kept,
            "target_app": payload.target_app,
            "platform": host_platform(),
            "output_language": payload.output_language,
            "model_used": payload.model_used,
            "capture_confidence": payload.capture_confidence,
        });
        if let Some(ref client_run_id) = payload.client_run_id {
            body["client_run_id"] = serde_json::Value::String(client_run_id.clone());
        }
        match state
            .http_client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
        {
            Ok(resp)
                if resp.status().is_success() || resp.status() == reqwest::StatusCode::ACCEPTED =>
            {
                info!(
                    "[profile-handoff] learn-from-edit queued edit_event={} recording={} client_run_id={} status={}",
                    payload.edit_event_id,
                    payload.recording_id,
                    payload.client_run_id.as_deref().unwrap_or("none"),
                    resp.status()
                );
            }
            Ok(resp) => {
                let status = resp.status();
                let preview = resp.text().await.unwrap_or_default();
                warn!(
                    "[profile-handoff] learn-from-edit failed edit_event={} client_run_id={} status={status} body={}",
                    payload.edit_event_id,
                    payload.client_run_id.as_deref().unwrap_or("none"),
                    said_core::text::truncate_utf8(&preview, 200)
                );
            }
            Err(e) => {
                warn!(
                    "[profile-handoff] learn-from-edit request failed edit_event={} client_run_id={}: {e}",
                    payload.edit_event_id,
                    payload.client_run_id.as_deref().unwrap_or("none"),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_confidence_maps_methods() {
        assert_eq!(capture_confidence_label("ax"), "high");
        assert_eq!(capture_confidence_label("keystroke_verified"), "high");
        assert_eq!(capture_confidence_label("clipboard"), "medium");
        assert_eq!(capture_confidence_label("keystroke_only"), "low");
    }
}
