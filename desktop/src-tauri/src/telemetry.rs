//! Fire-and-forget telemetry patches to the local backend outbox.

use std::sync::{LazyLock, Mutex};

use said_core::transcript::{TranscriptMeta, TranscriptOrigin};
use serde::Serialize;

use crate::{api, backend::BackendEndpoint};

#[derive(Debug, Clone, Default)]
struct LastRunMeta {
    run_id: String,
    finished_ms: u64,
    success: bool,
}

static LAST_RUN: LazyLock<Mutex<Option<LastRunMeta>>> = LazyLock::new(|| Mutex::new(None));

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn platform() -> &'static str {
    "macos"
}
#[cfg(target_os = "windows")]
fn platform() -> &'static str {
    "windows"
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform() -> &'static str {
    "other"
}

fn spawn_patch(ep: BackendEndpoint, run_id: String, patch: api::TelemetryRunPatch) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = api::patch_telemetry_run(&ep, &run_id, &patch).await {
            tracing::debug!("[telemetry] patch {run_id}: {e}");
        }
    });
}

pub fn on_run_start(ep: &BackendEndpoint, run_id: &str, mode: &str, target_app: Option<&str>) {
    if let Ok(mut guard) = LAST_RUN.lock() {
        if let Some(prev) = guard.take() {
            if !prev.success && now_ms().saturating_sub(prev.finished_ms) < 30_000 {
                let ep2 = ep.clone();
                spawn_patch(
                    ep2,
                    prev.run_id,
                    api::TelemetryRunPatch {
                        re_recorded_quickly: Some(true),
                        finalize: true,
                        ..Default::default()
                    },
                );
            }
        }
    }

    let patch = api::TelemetryRunPatch {
        mode: Some(mode.to_string()),
        target_app: target_app.map(str::to_string),
        platform: Some(platform().to_string()),
        app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        device_id: Some(said_core::paths::device_id()),
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineTelemetry {
    pub recording_id: String,
    pub mode: String,
    pub target_app: Option<String>,
    pub audio_seconds: f64,
    pub word_count: i32,
    pub char_count: i32,
    pub transcribe_ms: i32,
    pub embed_ms: i32,
    pub polish_ms: i32,
    pub total_ms: i32,
    pub success: bool,
    pub error_code: Option<String>,
    pub used_clipboard_fallback: bool,
    pub speech_provider: String,
    pub speech_model: String,
    pub speech_path: String,
    pub polished_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechTelemetryIdentity {
    pub provider: String,
    pub model: String,
    pub path: String,
}

/// Resolve telemetry from the transcript that actually won this run. Older
/// clients did not populate provider/path, so retain a conservative fallback
/// without consulting the currently selected Settings model.
pub fn speech_identity(meta: &TranscriptMeta) -> SpeechTelemetryIdentity {
    let model = if meta.model.trim().is_empty() {
        "unknown".to_string()
    } else {
        meta.model.clone()
    };
    let provider = if !meta.provider.trim().is_empty() {
        meta.provider.clone()
    } else if model.starts_with("deepinfra:") || meta.origin == TranscriptOrigin::DictationHosted {
        "deepinfra".to_string()
    } else if model.starts_with("local:") && model.to_ascii_lowercase().contains("nemotron") {
        "local_nemotron".to_string()
    } else if meta.origin == TranscriptOrigin::DictationLocal {
        "local_whisper".to_string()
    } else {
        "unknown".to_string()
    };
    let path = if !meta.path.trim().is_empty() {
        meta.path.clone()
    } else if provider == "deepinfra" {
        "http_batch".to_string()
    } else if provider.starts_with("local_") {
        "local_batch".to_string()
    } else {
        "unknown".to_string()
    };
    SpeechTelemetryIdentity {
        provider,
        model,
        path,
    }
}

fn derive_content_flags(text: &str) -> (bool, bool, bool, bool, bool, bool, bool, bool) {
    let t = text.trim();
    if t.is_empty() {
        return (false, false, false, false, false, false, false, false);
    }
    let has_numbers = t.chars().any(|c| c.is_ascii_digit());
    let has_currency =
        t.contains('₹') || t.contains('$') || t.contains(" rupee") || t.contains(" dollar");
    let has_percent = t.contains('%');
    let has_email = t.contains('@') && t.contains('.');
    let has_url = t.contains("http://") || t.contains("https://") || t.contains(".com");
    let has_code_like_terms =
        t.contains('_') || t.contains("()") || t.contains("API") || t.contains("SQL");
    let has_devanagari = t.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c));
    let has_latin = t.chars().any(|c| c.is_ascii_alphabetic());
    let mixed_language = has_devanagari && has_latin;
    let protected_term_hit = has_url || has_email || has_currency;
    (
        has_numbers,
        has_currency,
        has_percent,
        has_email,
        has_url,
        has_code_like_terms,
        mixed_language,
        protected_term_hit,
    )
}

fn edit_bucket_from_diff(polished: &str, kept: &str) -> (bool, &'static str, i32, i32) {
    let p = polished.trim();
    let k = kept.trim();
    if p == k || (p.is_empty() && k.is_empty()) {
        return (false, "none", 0, 0);
    }
    if k.is_empty() && !p.is_empty() {
        return (
            true,
            "full_replace",
            p.len() as i32,
            p.split_whitespace().count() as i32,
        );
    }
    if p.is_empty() && !k.is_empty() {
        return (
            true,
            "deleted",
            k.len() as i32,
            k.split_whitespace().count() as i32,
        );
    }
    let char_dist = (p.len() as i32 - k.len() as i32).unsigned_abs() as i32;
    let p_words: Vec<_> = p.split_whitespace().collect();
    let k_words: Vec<_> = k.split_whitespace().collect();
    let word_dist = (p_words.len() as i32 - k_words.len() as i32).unsigned_abs() as i32;
    let bucket = if word_dist <= 1 && char_dist <= 12 {
        "minor"
    } else if word_dist <= 3 {
        "small_phrase"
    } else if word_dist <= 8 {
        "medium"
    } else if !p_words.is_empty()
        && !k_words.is_empty()
        && word_dist as f32 / p_words.len().max(k_words.len()) as f32 > 0.6
    {
        "full_replace"
    } else {
        "heavy"
    };
    (true, bucket, char_dist, word_dist)
}

pub fn on_pipeline_done(ep: &BackendEndpoint, run_id: &str, t: PipelineTelemetry) {
    let flags = derive_content_flags(&t.polished_preview);
    let patch = api::TelemetryRunPatch {
        recording_id: Some(t.recording_id),
        mode: Some(t.mode),
        target_app: t.target_app,
        platform: Some(platform().to_string()),
        app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        device_id: Some(said_core::paths::device_id()),
        audio_seconds: Some(t.audio_seconds),
        word_count: Some(t.word_count),
        char_count: Some(t.char_count),
        transcribe_ms: Some(t.transcribe_ms),
        embed_ms: Some(t.embed_ms),
        polish_ms: Some(t.polish_ms),
        total_ms: Some(t.total_ms),
        success: Some(t.success),
        error_code: t.error_code,
        used_clipboard_fallback: Some(t.used_clipboard_fallback),
        speech_provider: Some(t.speech_provider),
        speech_model: Some(t.speech_model),
        speech_path: Some(t.speech_path),
        has_numbers: Some(flags.0),
        has_currency: Some(flags.1),
        has_percent: Some(flags.2),
        has_email: Some(flags.3),
        has_url: Some(flags.4),
        has_code_like_terms: Some(flags.5),
        mixed_language: Some(flags.6),
        protected_term_hit: Some(flags.7),
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);

    if let Ok(mut guard) = LAST_RUN.lock() {
        *guard = Some(LastRunMeta {
            run_id: run_id.to_string(),
            finished_ms: now_ms(),
            success: t.success,
        });
    }
}

/// Edit watch concluded with no meaningful user change (or field unreadable).
pub fn on_accepted_no_edit(ep: &BackendEndpoint, run_id: &str) {
    let patch = api::TelemetryRunPatch {
        accepted_as_is: Some(true),
        edit_detected: Some(false),
        edit_bucket: Some("none".to_string()),
        finalize: true,
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
}

/// The watcher observed activity but could no longer prove that it belonged to
/// AirNote's pasted span. This is unknown evidence, not an accepted dictation.
pub fn on_edit_excluded(ep: &BackendEndpoint, run_id: &str, reason: &str) {
    let patch = api::TelemetryRunPatch {
        accepted_as_is: None,
        edit_detected: Some(false),
        edit_bucket: Some(format!("excluded:{reason}")),
        finalize: true,
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
}

pub fn on_edit_outcome(
    ep: &BackendEndpoint,
    run_id: &str,
    polished: &str,
    kept: &str,
    accepted_as_is: bool,
    deleted_entire: bool,
    finalize: bool,
) {
    let (edit_detected, bucket, char_dist, word_dist) = edit_bucket_from_diff(polished, kept);
    let patch = api::TelemetryRunPatch {
        edit_detected: Some(edit_detected),
        edit_bucket: Some(bucket.to_string()),
        edit_distance_chars: Some(char_dist),
        edit_distance_words: Some(word_dist),
        accepted_as_is: Some(accepted_as_is),
        deleted_entire_output: Some(deleted_entire),
        finalize,
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
}

pub fn on_classify_result(ep: &BackendEndpoint, run_id: &str, resp: &api::ClassifyEditResponse) {
    let learning_modal = resp.notify
        || !resp.review_candidates.is_empty()
        || !resp.ambiguous_terms.is_empty()
        || resp.pending_id.is_some();
    let patch = api::TelemetryRunPatch {
        learning_candidate: Some(learning_modal || resp.learned),
        learning_modal_shown: Some(learning_modal),
        server_learning_saved: Some(resp.learned),
        learning_confirmed: Some(resp.learned && !resp.review_candidates.is_empty()),
        finalize: true,
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
}

pub fn on_learning_resolve(ep: &BackendEndpoint, run_id: &str, confirmed: bool, dismissed: bool) {
    let patch = api::TelemetryRunPatch {
        learning_confirmed: Some(confirmed),
        learning_dismissed: Some(dismissed),
        server_learning_saved: Some(confirmed),
        server_learning_blocked: Some(dismissed),
        finalize: true,
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
}

pub fn on_pipeline_error(ep: &BackendEndpoint, run_id: &str, error_code: Option<String>) {
    let patch = api::TelemetryRunPatch {
        success: Some(false),
        error_code,
        finalize: true,
        ..Default::default()
    };
    spawn_patch(ep.clone(), run_id.to_string(), patch);
    if let Ok(mut guard) = LAST_RUN.lock() {
        *guard = Some(LastRunMeta {
            run_id: run_id.to_string(),
            finished_ms: now_ms(),
            success: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::speech_identity;

    #[test]
    fn preserves_explicit_deepinfra_batch_identity() {
        let identity = speech_identity(&TranscriptMeta {
            model: "deepinfra:openai/whisper-large-v3-turbo".into(),
            provider: "deepinfra".into(),
            path: "http_batch".into(),
            origin: TranscriptOrigin::DictationHosted,
            ..TranscriptMeta::default()
        });
        assert_eq!(identity.provider, "deepinfra");
        assert_eq!(identity.path, "http_batch");
    }

    #[test]
    fn legacy_hosted_metadata_never_falls_back_to_selected_local_model() {
        let identity = speech_identity(&TranscriptMeta {
            model: "deepinfra:openai/whisper-large-v3-turbo".into(),
            origin: TranscriptOrigin::DictationHosted,
            ..TranscriptMeta::default()
        });
        assert_eq!(identity.provider, "deepinfra");
        assert_eq!(identity.path, "http_batch");
    }
}
