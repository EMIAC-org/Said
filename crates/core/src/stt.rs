//! Shared speech-to-text provider selection for local backend + control-plane.
//!
//! Deepgram is the default cloud STT vendor. `swift_local` is a macOS-only live
//! local path (Oriserve Swift). Legacy ids `groq_whisper` and `whisper_local`
//! remain as local batch fallbacks.

use serde::{Deserialize, Serialize};

/// Active STT vendor. Deepgram is the only cloud vendor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttProvider {
    Deepgram,
    SwiftLocal,
}

impl SttProvider {
    pub fn parse(raw: &str) -> Self {
        if is_swift_local(raw) {
            Self::SwiftLocal
        } else {
            Self::Deepgram
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
            Self::SwiftLocal => "swift_local",
        }
    }
}

/// Resolve the user's STT provider from SQLite preferences only.
pub fn resolve_provider_from_pref(pref: &str) -> String {
    normalize_toggle_stt_provider(pref)
}

/// Server-side STT vendor. Deepgram is the only server vendor, so this always
/// resolves to `"deepgram"` (kept as a function so call sites stay stable).
pub fn resolve_server_default_provider() -> String {
    "deepgram".to_string()
}

/// Normalize Settings toggle values. Legacy ids (`whisper_local`, `groq_whisper`) pass through.
pub fn normalize_toggle_stt_provider(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "deepgram" => "deepgram".to_string(),
        "swift_local" | "swift" | "swift-local" => "swift_local".to_string(),
        "" => "deepgram".to_string(),
        other => other.to_string(),
    }
}

pub fn is_deepgram(provider: &str) -> bool {
    provider.is_empty() || provider.eq_ignore_ascii_case("deepgram")
}

pub fn is_swift_local(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("swift_local")
}

/// Whether the Swift local STT option is supported on this platform build.
pub fn swift_local_platform_supported() -> bool {
    cfg!(target_os = "macos")
}

/// Resolve the dictation STT provider at runtime, falling back to Deepgram when
/// Swift is selected but unavailable (wrong platform or model not installed).
pub fn effective_dictation_provider(pref: &str, swift_model_installed: bool) -> String {
    let resolved = resolve_provider_from_pref(pref);
    if is_swift_local(&resolved) {
        if swift_local_platform_supported() && swift_model_installed {
            "swift_local".to_string()
        } else {
            "deepgram".to_string()
        }
    } else {
        resolved
    }
}

/// Providers that skip live WS pre-transcript and batch-STT the full WAV on release.
pub fn use_batch_stt_only(provider: &str) -> bool {
    provider == "groq_whisper" || provider == "whisper_local"
}

/// Model id recorded in per-run telemetry.
pub fn telemetry_stt_model(provider: &str) -> &'static str {
    if is_swift_local(provider) {
        "Oriserve/Whisper-Hindi2Hinglish-Swift"
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
        assert_eq!(normalize_toggle_stt_provider("swift_local"), "swift_local");
        assert_eq!(normalize_toggle_stt_provider("Swift"), "swift_local");
    }

    #[test]
    fn swift_local_not_batch_only() {
        assert!(!use_batch_stt_only("swift_local"));
    }

    #[test]
    fn effective_dictation_falls_back_without_model() {
        assert_eq!(
            effective_dictation_provider("swift_local", false),
            "deepgram"
        );
        assert_eq!(
            effective_dictation_provider("swift_local", true),
            if swift_local_platform_supported() {
                "swift_local"
            } else {
                "deepgram"
            }
        );
    }

    #[test]
    fn telemetry_model_for_swift() {
        assert_eq!(
            telemetry_stt_model("swift_local"),
            "Oriserve/Whisper-Hindi2Hinglish-Swift"
        );
    }

    #[test]
    fn pref_resolution_ignores_empty() {
        assert_eq!(resolve_provider_from_pref(""), "deepgram");
        assert_eq!(resolve_provider_from_pref("deepgram"), "deepgram");
    }
}
