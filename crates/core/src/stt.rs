//! Shared speech-to-text provider selection for local backend + control-plane.

use serde::{Deserialize, Serialize};

/// Active STT vendor. Toggle in Settings (`preferences.stt_provider`) or server deploy env.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttProvider {
    Deepgram,
    Sarvam,
}

impl SttProvider {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "sarvam" => Self::Sarvam,
            _ => Self::Deepgram,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
            Self::Sarvam => "sarvam",
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

/// Alias for [`resolve_provider_from_pref`] — keeps older call sites readable.
pub fn resolve_provider_string(pref: &str) -> String {
    resolve_provider_from_pref(pref)
}

pub fn resolve_stt_provider(pref: &str) -> SttProvider {
    SttProvider::parse(&resolve_provider_from_pref(pref))
}

/// Normalize Settings toggle values. Legacy ids (`whisper_local`, `groq_whisper`) pass through.
pub fn normalize_toggle_stt_provider(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sarvam" => "sarvam".to_string(),
        "deepgram" => "deepgram".to_string(),
        "" => "deepgram".to_string(),
        other => other.to_string(),
    }
}

pub fn is_sarvam(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("sarvam")
}

pub fn is_deepgram(provider: &str) -> bool {
    provider.is_empty() || provider.eq_ignore_ascii_case("deepgram")
}

/// Providers that skip live WS pre-transcript and batch-STT the full WAV on release.
pub fn use_batch_stt_only(provider: &str) -> bool {
    is_sarvam(provider) || provider == "groq_whisper" || provider == "whisper_local"
}

pub const SARVAM_TELEMETRY_MODEL: &str = "saaras:v3";

/// Model id recorded in per-run telemetry.
pub fn telemetry_stt_model(provider: &str) -> &'static str {
    if is_sarvam(provider) {
        SARVAM_TELEMETRY_MODEL
    } else {
        crate::deepgram::DEEPGRAM_MODEL
    }
}

fn non_empty_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Sarvam key from SQLite prefs with dev env fallback.
pub fn resolve_sarvam_api_key(pref_key: Option<&str>) -> Option<String> {
    non_empty_opt(pref_key)
        .or_else(|| non_empty_opt(std::env::var("SARVAM_API_KEY").ok().as_deref()))
}

/// Deepgram key from SQLite prefs with env fallback.
pub fn resolve_deepgram_api_key(pref_key: Option<&str>) -> Option<String> {
    non_empty_opt(pref_key)
        .or_else(|| non_empty_opt(std::env::var("DEEPGRAM_API_KEY").ok().as_deref()))
}

/// Runtime STT vendor after key availability checks (Sarvam pref without key → Deepgram).
pub fn resolve_effective_stt_provider(
    pref: &str,
    has_sarvam_key: bool,
    _has_deepgram_key: bool,
) -> String {
    let normalized = normalize_toggle_stt_provider(pref);
    if is_sarvam(&normalized) {
        if has_sarvam_key {
            "sarvam".to_string()
        } else {
            "deepgram".to_string()
        }
    } else {
        normalized
    }
}

/// How audio reached STT for this run (`ws_prewarm`, `ws_stream`, `http_batch`).
pub fn telemetry_stt_path(provider: &str, had_ws_pretranscript: bool) -> &'static str {
    if use_batch_stt_only(provider) || !had_ws_pretranscript {
        "http_batch"
    } else if is_sarvam(provider) {
        "ws_stream"
    } else {
        "ws_prewarm"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_toggle_maps_ui_values() {
        assert_eq!(normalize_toggle_stt_provider("Sarvam"), "sarvam");
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
        assert_eq!(resolve_provider_from_pref("sarvam"), "sarvam");
    }

    #[test]
    fn effective_provider_falls_back_without_sarvam_key() {
        assert_eq!(
            resolve_effective_stt_provider("sarvam", false, true),
            "deepgram"
        );
        assert_eq!(
            resolve_effective_stt_provider("sarvam", true, false),
            "sarvam"
        );
        assert_eq!(
            resolve_effective_stt_provider("deepgram", false, false),
            "deepgram"
        );
    }
}
