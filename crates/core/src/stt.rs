//! Shared speech-to-text provider selection for local backend + control-plane.
//!
//! Deepgram is the default cloud STT vendor. `swift_local` is a macOS-only live
//! local path (Oriserve Swift). Legacy ids `groq_whisper` and `whisper_local`
//! remain as local batch fallbacks.

use serde::{Deserialize, Serialize};

/// The dictation STT engine, as a typed first-class value. This is the single
/// source of truth for "which engine" — prefer it over ad-hoc string checks.
///
/// - `Deepgram`   — cloud (build-bundled key). Also the bucket for legacy cloud
///   ids like `groq_whisper`.
/// - `SwiftLocal` — on-device, macOS-only Python sidecar (Oriserve Swift).
/// - `WhisperLocal` — on-device, native whisper.cpp (Oriserve Hinglish GGML).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttProvider {
    Deepgram,
    SwiftLocal,
    WhisperLocal,
}

impl SttProvider {
    /// Parse a (possibly un-normalized) provider id. Anything that is not a known
    /// local engine — including `groq_whisper` and empty/unknown ids — is treated
    /// as cloud `Deepgram`.
    pub fn parse(raw: &str) -> Self {
        match normalize_toggle_stt_provider(raw).as_str() {
            "swift_local" => Self::SwiftLocal,
            "whisper_local" => Self::WhisperLocal,
            _ => Self::Deepgram,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
            Self::SwiftLocal => "swift_local",
            Self::WhisperLocal => "whisper_local",
        }
    }

    /// On-device engine (audio never leaves the machine).
    pub fn is_local(self) -> bool {
        matches!(self, Self::SwiftLocal | Self::WhisperLocal)
    }

    /// Cloud engine (Deepgram).
    pub fn is_cloud(self) -> bool {
        matches!(self, Self::Deepgram)
    }

    /// The transcript origin a pre-transcript MUST carry to be accepted as this
    /// provider's authoritative local output. `None` for cloud.
    pub fn expected_local_origin(self) -> Option<TranscriptOrigin> {
        match self {
            Self::SwiftLocal => Some(TranscriptOrigin::SwiftLocal),
            Self::WhisperLocal => Some(TranscriptOrigin::WhisperLocal),
            Self::Deepgram => None,
        }
    }
}

/// Where a transcript was produced. First-class provenance, kept SEPARATE from
/// `TranscriptMeta.stt_mode` (which is the bias *language* mode: "hi"/"multi").
/// Serialized with the transcript so the backend reads a real enum instead of
/// sniffing a string. Unknown/absent values degrade to `Unspecified`/`Unknown`
/// so mixed desktop↔backend builds stay compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptOrigin {
    /// Producer did not declare an origin (e.g. a stale desktop build).
    #[default]
    Unspecified,
    DeepgramWs,
    DeepgramBatch,
    WhisperLocal,
    SwiftLocal,
    SwiftLocalLivePartial,
    GroqWhisper,
    /// A future id this build doesn't recognize (forward-compat).
    #[serde(other)]
    Unknown,
}

impl TranscriptOrigin {
    /// Produced by an on-device engine.
    pub fn is_local(self) -> bool {
        matches!(
            self,
            Self::WhisperLocal | Self::SwiftLocal | Self::SwiftLocalLivePartial
        )
    }

    /// The engine that produced this transcript, when unambiguous.
    pub fn provider(self) -> Option<SttProvider> {
        match self {
            Self::DeepgramWs | Self::DeepgramBatch => Some(SttProvider::Deepgram),
            Self::WhisperLocal => Some(SttProvider::WhisperLocal),
            Self::SwiftLocal | Self::SwiftLocalLivePartial => Some(SttProvider::SwiftLocal),
            Self::GroqWhisper | Self::Unspecified | Self::Unknown => None,
        }
    }
}

/// What the backend should do with the inbound audio + optional pre-transcript.
/// Produced by [`decide_stt_plan`] — one place, readable, unit-tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SttPlan {
    /// Trust the inbound local pre-transcript as authoritative (no cloud).
    UseInboundLocal,
    /// Use the inbound Deepgram WS pre-transcript (rescue-eligible).
    UseInboundCloudWs,
    /// No pre-transcript; run cloud Deepgram batch on the WAV.
    CloudBatch,
    /// No usable local pre-transcript; re-run the on-device engine here.
    LocalOnDeviceBatch,
    /// Local engine genuinely produced nothing AND no on-device engine is
    /// available in this build → cloud Deepgram as the genuine-failure safety net.
    CloudFallbackAfterLocalFail,
}

/// Decide what to do with the audio + optional pre-transcript for the selected
/// provider. Pure (no I/O) so it is exhaustively unit-tested.
///
/// `local_batch_available` is whether THIS process can re-run the selected local
/// engine itself (e.g. `cfg!(feature = "local-stt")` for whisper.cpp). Dev
/// backends are built without it, so a local provider with no usable inbound
/// transcript correctly resolves to the Deepgram safety net.
pub fn decide_stt_plan(
    selected: SttProvider,
    pre_present: bool,
    pre_origin: TranscriptOrigin,
    local_batch_available: bool,
) -> SttPlan {
    match selected {
        SttProvider::Deepgram => {
            if pre_present {
                SttPlan::UseInboundCloudWs
            } else {
                SttPlan::CloudBatch
            }
        }
        local @ (SttProvider::SwiftLocal | SttProvider::WhisperLocal) => {
            // Accept the inbound transcript only when it actually came from the
            // selected local engine. `Unspecified` (stale desktop that doesn't
            // tag origin yet) is trusted, since the provider was locally selected.
            let origin_ok =
                pre_origin == TranscriptOrigin::Unspecified || pre_origin.provider() == Some(local);
            if pre_present && origin_ok {
                SttPlan::UseInboundLocal
            } else if local_batch_available {
                SttPlan::LocalOnDeviceBatch
            } else {
                SttPlan::CloudFallbackAfterLocalFail
            }
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
        "whisper_local" | "whisper" | "whisper-local" | "turbo_q5" | "q5_turbo" => {
            "whisper_local".to_string()
        }
        // Default to the native, Python-free on-device whisper.cpp path.
        "" => "whisper_local".to_string(),
        other => other.to_string(),
    }
}

pub fn is_deepgram(provider: &str) -> bool {
    provider.is_empty() || provider.eq_ignore_ascii_case("deepgram")
}

pub fn is_swift_local(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("swift_local")
}

pub fn is_whisper_local(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("whisper_local")
}

/// whisper.cpp Turbo Q5 model filename (shared with Meetings).
pub const WHISPER_TURBO_Q5_MODEL: &str = "ggml-large-v3-turbo-q5_0.bin";

/// Whether the Swift local STT option is supported on this platform build.
pub fn swift_local_platform_supported() -> bool {
    cfg!(target_os = "macos")
}

/// Resolve the *effective* dictation STT provider — the one actually usable
/// right now, given which on-device models are installed. This is what the
/// Settings UI should pre-select and what telemetry reports.
///
/// Rules (installed-aware, so the selection follows what onboarding set up):
/// - Explicit `deepgram` → cloud.
/// - `swift_local` → on-device Swift only on macOS AND when its model is
///   installed; otherwise cloud.
/// - `whisper_local` or the default (unset) → on-device whisper.cpp only when
///   its model is installed; otherwise fall back to an installed Swift model
///   (macOS), else cloud Deepgram.
///
/// So: a user who downloaded the local model gets it auto-selected; one who
/// didn't (or chose cloud) gets Deepgram. The bundled Deepgram key means cloud
/// is always available as the fallback.
pub fn effective_dictation_provider(
    pref: &str,
    swift_model_installed: bool,
    whisper_model_installed: bool,
) -> String {
    // Dev/testing override: force a specific dictation STT provider regardless of
    // prefs or install state (e.g. AIRNOTE_FORCE_STT_PROVIDER=whisper_local).
    if let Ok(forced) = std::env::var("AIRNOTE_FORCE_STT_PROVIDER") {
        let norm = normalize_toggle_stt_provider(&forced);
        if !norm.is_empty() {
            return norm;
        }
    }
    let swift_available = swift_local_platform_supported() && swift_model_installed;
    let resolved = resolve_provider_from_pref(pref);
    if is_deepgram(&resolved) {
        // Explicit cloud opt-in.
        return "deepgram".to_string();
    }
    if is_swift_local(&resolved) {
        // Legacy Python-sidecar path, only if explicitly chosen, on macOS, and
        // its model is present. Otherwise the cloud key is always available.
        return if swift_available {
            "swift_local".to_string()
        } else {
            "deepgram".to_string()
        };
    }
    // `whisper_local` or the default: native, Python-free on-device whisper.cpp —
    // but only when the model has actually been downloaded.
    if whisper_model_installed {
        "whisper_local".to_string()
    } else if swift_available {
        "swift_local".to_string()
    } else {
        "deepgram".to_string()
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
    } else if is_whisper_local(provider) {
        WHISPER_TURBO_Q5_MODEL
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

pub const DEEPGRAM_ENV_KEY_CANDIDATES: &[&str] = &[
    "DEEPGRAM_API_KEY_1",
    "DEEPGRAM_API_KEY",
    "DEEPGRAM_API_KEY_2",
    "DEEPGRAM_API_KEY_3",
];

/// Deepgram STT key baked into the build at compile time. Mirrors the bundled
/// DeepSeek/Resend keys: set `DEEPGRAM_API_KEY` in the build environment
/// (`build-dmg.sh` exports it from `.env`) and it is baked into the binary so
/// the shipped app ships with a working key — end users never enter one. The
/// value is captured at compile time and never written to a tracked file, so it
/// is never committed to git. `None` in dev builds where it wasn't baked, so the
/// caller falls back to a runtime env var.
pub const BUNDLED_DEEPGRAM_API_KEY: Option<&str> = option_env!("DEEPGRAM_API_KEY");

/// Resolve the Deepgram key: runtime env (dev/server) → build-time bundled key
/// (shipped app) → legacy saved preference. Users no longer supply a Deepgram
/// key, so old/stale preference values must not override the managed key.
pub fn resolve_deepgram_api_key(pref_key: Option<&str>) -> Option<String> {
    DEEPGRAM_ENV_KEY_CANDIDATES
        .iter()
        .find_map(|key| non_empty_opt(std::env::var(key).ok().as_deref()))
        .or_else(|| non_empty_opt(BUNDLED_DEEPGRAM_API_KEY))
        .or_else(|| non_empty_opt(pref_key))
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
        // Empty pref defaults to the native, Python-free on-device whisper.cpp path.
        assert_eq!(normalize_toggle_stt_provider(""), "whisper_local");
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
    fn effective_dictation_is_installed_aware() {
        // A local provider whose model isn't installed falls back to cloud, so the
        // auto-selected option matches what the user actually downloaded.
        assert_eq!(
            effective_dictation_provider("swift_local", false, false),
            "deepgram"
        );
        // Swift selected + installed (macOS only) → swift; otherwise cloud.
        let swift_installed_expected = if swift_local_platform_supported() {
            "swift_local"
        } else {
            "deepgram"
        };
        assert_eq!(
            effective_dictation_provider("swift_local", true, false),
            swift_installed_expected
        );
        // whisper_local only when its model is downloaded.
        assert_eq!(
            effective_dictation_provider("whisper_local", false, true),
            "whisper_local"
        );
        assert_eq!(
            effective_dictation_provider("whisper_local", false, false),
            "deepgram"
        );
        // Default (unset) is installed-aware too: local if downloaded, else cloud.
        assert_eq!(
            effective_dictation_provider("", false, true),
            "whisper_local"
        );
        assert_eq!(effective_dictation_provider("", false, false), "deepgram");
        // Explicit cloud always wins.
        assert_eq!(
            effective_dictation_provider("deepgram", false, true),
            "deepgram"
        );
    }

    #[test]
    fn whisper_local_is_batch_only() {
        assert!(use_batch_stt_only("whisper_local"));
    }

    #[test]
    fn normalize_whisper_aliases() {
        assert_eq!(normalize_toggle_stt_provider("turbo_q5"), "whisper_local");
        assert_eq!(normalize_toggle_stt_provider("Q5_Turbo"), "whisper_local");
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
        // Empty pref → native on-device whisper.cpp default.
        assert_eq!(resolve_provider_from_pref(""), "whisper_local");
        assert_eq!(resolve_provider_from_pref("deepgram"), "deepgram");
    }

    #[test]
    fn provider_parse_and_class() {
        assert_eq!(SttProvider::parse("swift_local"), SttProvider::SwiftLocal);
        assert_eq!(
            SttProvider::parse("whisper_local"),
            SttProvider::WhisperLocal
        );
        assert_eq!(SttProvider::parse("turbo_q5"), SttProvider::WhisperLocal);
        assert_eq!(SttProvider::parse("deepgram"), SttProvider::Deepgram);
        assert_eq!(SttProvider::parse("groq_whisper"), SttProvider::Deepgram);
        assert_eq!(SttProvider::WhisperLocal.as_str(), "whisper_local");
        assert!(SttProvider::WhisperLocal.is_local());
        assert!(SttProvider::SwiftLocal.is_local());
        assert!(!SttProvider::Deepgram.is_local());
        assert!(SttProvider::Deepgram.is_cloud());
    }

    #[test]
    fn transcript_origin_classification() {
        assert!(TranscriptOrigin::WhisperLocal.is_local());
        assert!(TranscriptOrigin::SwiftLocalLivePartial.is_local());
        assert!(!TranscriptOrigin::DeepgramWs.is_local());
        assert_eq!(
            TranscriptOrigin::WhisperLocal.provider(),
            Some(SttProvider::WhisperLocal)
        );
        assert_eq!(
            TranscriptOrigin::SwiftLocalLivePartial.provider(),
            Some(SttProvider::SwiftLocal)
        );
        assert_eq!(TranscriptOrigin::Unspecified.provider(), None);
        assert_eq!(TranscriptOrigin::default(), TranscriptOrigin::Unspecified);
    }

    #[test]
    fn transcript_origin_serde_compat() {
        // Round-trip.
        assert_eq!(
            serde_json::to_string(&TranscriptOrigin::WhisperLocal).unwrap(),
            "\"whisper_local\""
        );
        // Unknown future id degrades to Unknown, not an error (forward-compat).
        let unknown: TranscriptOrigin = serde_json::from_str("\"future_engine\"").unwrap();
        assert_eq!(unknown, TranscriptOrigin::Unknown);
    }

    #[test]
    fn decide_plan_uses_local_when_origin_matches() {
        // The exact bug: whisper_local + a whisper-origin pre-transcript → use it.
        assert_eq!(
            decide_stt_plan(
                SttProvider::WhisperLocal,
                true,
                TranscriptOrigin::WhisperLocal,
                false,
            ),
            SttPlan::UseInboundLocal
        );
        // Stale desktop that doesn't tag origin is still trusted for a local provider.
        assert_eq!(
            decide_stt_plan(
                SttProvider::WhisperLocal,
                true,
                TranscriptOrigin::Unspecified,
                false,
            ),
            SttPlan::UseInboundLocal
        );
        assert_eq!(
            decide_stt_plan(
                SttProvider::SwiftLocal,
                true,
                TranscriptOrigin::SwiftLocal,
                false,
            ),
            SttPlan::UseInboundLocal
        );
    }

    #[test]
    fn decide_plan_local_failure_falls_back() {
        // Dev build (no on-device engine) + no pre-transcript → Deepgram safety net.
        assert_eq!(
            decide_stt_plan(
                SttProvider::WhisperLocal,
                false,
                TranscriptOrigin::Unspecified,
                false,
            ),
            SttPlan::CloudFallbackAfterLocalFail
        );
        // Swift unified with whisper: genuine failure → Deepgram fallback.
        assert_eq!(
            decide_stt_plan(
                SttProvider::SwiftLocal,
                false,
                TranscriptOrigin::Unspecified,
                false,
            ),
            SttPlan::CloudFallbackAfterLocalFail
        );
        // Release build can re-run the engine locally instead.
        assert_eq!(
            decide_stt_plan(
                SttProvider::WhisperLocal,
                false,
                TranscriptOrigin::Unspecified,
                true,
            ),
            SttPlan::LocalOnDeviceBatch
        );
        // Mismatched provenance (stale Deepgram WS partial) is not trusted as local.
        assert_eq!(
            decide_stt_plan(
                SttProvider::WhisperLocal,
                true,
                TranscriptOrigin::DeepgramWs,
                false,
            ),
            SttPlan::CloudFallbackAfterLocalFail
        );
    }

    #[test]
    fn decide_plan_deepgram_paths() {
        assert_eq!(
            decide_stt_plan(
                SttProvider::Deepgram,
                true,
                TranscriptOrigin::DeepgramWs,
                false,
            ),
            SttPlan::UseInboundCloudWs
        );
        assert_eq!(
            decide_stt_plan(
                SttProvider::Deepgram,
                false,
                TranscriptOrigin::Unspecified,
                false,
            ),
            SttPlan::CloudBatch
        );
    }
}
