//! Shared speech-to-text provider selection for local backend + control-plane.
//!
//! Deepgram is the only cloud STT vendor. The legacy ids `groq_whisper` and
//! `whisper_local` remain as local batch fallbacks.

use serde::{Deserialize, Serialize};

/// Active STT vendor. Deepgram is the only cloud vendor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttProvider {
    Deepgram,
}

impl SttProvider {
    pub fn parse(_raw: &str) -> Self {
        Self::Deepgram
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
        }
    }
}

pub const STT_PROVIDER_ENV: &str = "AIRNOTE_STT_PROVIDER";

/// Resolve the user's STT provider from SQLite preferences only.
pub fn resolve_provider_from_pref(pref: &str) -> String {
    normalize_toggle_stt_provider(pref)
}

/// Server deploy default — env wins when set, else Deepgram. Not used for desktop user prefs.
pub fn resolve_server_default_provider() -> String {
    if let Ok(env) = std::env::var(STT_PROVIDER_ENV) {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return trimmed.to_ascii_lowercase();
        }
    }
    "deepgram".to_string()
}

/// Normalize Settings toggle values. Legacy ids (`whisper_local`, `groq_whisper`) pass through.
pub fn normalize_toggle_stt_provider(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "deepgram" => "deepgram".to_string(),
        "" => "deepgram".to_string(),
        other => other.to_string(),
    }
}

pub fn is_deepgram(provider: &str) -> bool {
    provider.is_empty() || provider.eq_ignore_ascii_case("deepgram")
}

/// Providers that skip live WS pre-transcript and batch-STT the full WAV on release.
pub fn use_batch_stt_only(provider: &str) -> bool {
    provider == "groq_whisper" || provider == "whisper_local"
}

/// Model id recorded in per-run telemetry.
pub fn telemetry_stt_model(_provider: &str) -> &'static str {
    crate::deepgram::DEEPGRAM_MODEL
}

fn non_empty_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Deepgram key from SQLite prefs with env fallback.
pub fn resolve_deepgram_api_key(pref_key: Option<&str>) -> Option<String> {
    non_empty_opt(pref_key)
        .or_else(|| non_empty_opt(std::env::var("DEEPGRAM_API_KEY").ok().as_deref()))
}

/// How audio reached STT for this run (`ws_prewarm`, `http_batch`).
pub fn telemetry_stt_path(provider: &str, had_ws_pretranscript: bool) -> &'static str {
    if use_batch_stt_only(provider) || !had_ws_pretranscript {
        "http_batch"
    } else {
        "ws_prewarm"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_toggle_maps_ui_values() {
        assert_eq!(normalize_toggle_stt_provider("Deepgram"), "deepgram");
        assert_eq!(normalize_toggle_stt_provider("deepgram"), "deepgram");
        assert_eq!(normalize_toggle_stt_provider(""), "deepgram");
        assert_eq!(
            normalize_toggle_stt_provider("groq_whisper"),
            "groq_whisper"
        );
    }

    #[test]
    fn pref_resolution_ignores_empty() {
        assert_eq!(resolve_provider_from_pref(""), "deepgram");
        assert_eq!(resolve_provider_from_pref("deepgram"), "deepgram");
    }
}
