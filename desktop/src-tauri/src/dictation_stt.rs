//! Dictation speech-to-text — the platform routing seam.
//!
//! | platform | provider |
//! |----------|----------|
//! | macOS    | on-device whisper (`asr-core`, Metal in-process) — always |
//! | Windows  | user-selectable (Settings → Speech recognition): **Auto** (default), On-device, or Hosted |
//!
//! Windows selection, resolved per clip in a strict order:
//!   1. `AIRNOTE_DICTATION_STT_PROVIDER=local|hosted` — diagnostics escape
//!      hatch, pins the provider for the whole session (dev A/B, offline debug).
//!   2. The `dictation_stt` desktop pref ("local" / "hosted") — the Settings
//!      toggle; read per clip so a change applies to the very next dictation.
//!   3. **Auto**: on-device iff this machine runs it *well* — usable GPU
//!      (isolated `airnote-asr-gpu` Vulkan worker came up) + local model
//!      installed. Otherwise hosted (DeepInfra whisper-large-v3). The
//!      capability probe is hardware, so it's cached for the session.
//!
//! There is deliberately no mid-clip provider fallback: a clip runs on exactly
//! one provider and fails loudly with an actionable message. (Within the
//! on-device provider, the asr router keeps its own GPU→CPU crash failover —
//! that's engine survival, not provider switching.) Everything downstream is
//! provider-agnostic — the transcript feeds the backend's `pre_transcript`
//! contract unchanged.
//!
//! Meetings are a separate pipeline (`meeting_engine`) and keep the on-device
//! model on all platforms — the model-status helpers below still report on it.

use said_core::transcript::TranscriptMeta;

/// The finished dictation transcript, handed to the backend as
/// `pre_transcript` for LLM polishing.
#[derive(Debug, Clone)]
pub struct PreTranscript {
    pub transcript: String,
    pub meta: TranscriptMeta,
}

/// Stable identifier of the provider the next dictation will use (status UI,
/// logs): `"on-device/whisper"`, `"on-device/nemotron"`, or `"deepinfra"`.
pub fn provider_name() -> &'static str {
    provider::name()
}

/// True when dictation can transcribe right now (model present / key baked).
/// Network reachability is not probed here — an offline hosted call fails
/// fast with its own actionable error.
pub fn dictation_ready() -> bool {
    provider::ready()
}

/// What Auto resolves to on this machine: `"on-device/whisper"` or
/// `"deepinfra"`. Lets Settings say "Auto — on this device: On-device (GPU)".
pub fn auto_provider_name() -> &'static str {
    provider::auto_name()
}

// ── On-device model status ─────────────────────────────────────────────────
// The whisper model still powers meetings on every platform (and dictation on
// macOS); Settings surfaces these regardless of the dictation provider.

pub fn model_installed() -> bool {
    crate::meeting_engine::dictation_whisper_model_installed()
}

pub fn runtime_ready() -> bool {
    crate::meeting_engine::dictation_whisper_runtime_ready()
}

pub fn vad_installed() -> bool {
    crate::meeting_engine::silero_vad_model_installed()
}

/// Transcribe one dictation WAV with this platform's provider.
///
/// `language` is the user's configured dictation language; providers may use
/// it as a hint or run their own detection (see each provider module).
/// Errors are complete user-facing sentences, shown verbatim in the UI.
pub async fn transcribe_wav_bytes(wav: &[u8], language: &str) -> Result<PreTranscript, String> {
    provider::transcribe(wav, language).await
}

/// Warm this platform's provider at startup so the first utterance doesn't
/// pay setup costs: on-device pre-loads the whisper model; hosted resolves the
/// API key and builds the HTTP client — and logs loudly if the build shipped
/// without a key, so a broken build is visible before the first dictation.
pub fn prewarm() {
    provider::prewarm();
}

// ── On-device engine (asr-core) — dictation on macOS; meetings everywhere ──
// Windows can select it too via the diagnostics escape hatch (module docs).
mod on_device {
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::PreTranscript;

    const WHISPER_NAME: &str = "on-device/whisper";
    const NEMOTRON_NAME: &str = "on-device/nemotron";

    fn uses_nemotron() -> bool {
        said_core::prefs::load().local_stt_model == "nemotron"
    }

    pub(super) fn name() -> &'static str {
        if uses_nemotron() {
            NEMOTRON_NAME
        } else {
            WHISPER_NAME
        }
    }

    pub(super) fn ready() -> bool {
        if uses_nemotron() {
            crate::nemotron::installed()
        } else {
            super::model_installed() && super::runtime_ready()
        }
    }

    /// Pre-load the whisper model so the first utterance skips the model load.
    pub(super) fn prewarm() {
        if uses_nemotron() {
            crate::nemotron::prewarm();
        } else {
            crate::asr::prewarm_default_language();
        }
    }

    pub(super) async fn transcribe(wav: &[u8], language: &str) -> Result<PreTranscript, String> {
        if uses_nemotron() {
            if !crate::nemotron::installed() {
                return Err(
                    "Nemotron is selected but not installed. Download it in Settings → Speech recognition."
                        .into(),
                );
            }

            let wav = wav.to_vec();
            let language = language.to_string();
            let output = tokio::task::spawn_blocking(move || {
                crate::nemotron::transcribe_wav_bytes(&wav, &language)
            })
            .await
            .map_err(|error| format!("Nemotron speech worker failed: {error}"))??;
            let word_count = output.transcript.split_whitespace().count();
            tracing::info!(
                duration_ms = output.duration_ms,
                language = output.language.as_deref().unwrap_or("unreported"),
                model = crate::nemotron::MODEL_NAME,
                "[dictation_stt] Nemotron local ASR complete"
            );
            return Ok(PreTranscript {
                transcript: output.transcript.clone(),
                meta: TranscriptMeta {
                    enriched_transcript: output.transcript,
                    confidence: 1.0,
                    mean_word_confidence: 1.0,
                    low_confidence_count: 0,
                    word_count,
                    languages: output.language.into_iter().collect(),
                    model: format!("local:{}", crate::nemotron::MODEL_FILE),
                    duration_ms: output.duration_ms,
                    origin: TranscriptOrigin::DictationLocal,
                    ..TranscriptMeta::default()
                },
            });
        }

        if !super::model_installed() {
            return Err(
                "Local speech model is required. Download the on-device model in Settings.".into(),
            );
        }

        let started = std::time::Instant::now();
        let wav = wav.to_vec();
        let language = language.to_string();
        let local =
            tokio::task::spawn_blocking(move || crate::asr::transcribe_wav_bytes(wav, language))
                .await
                .map_err(|e| format!("local speech worker failed: {e}"))??;
        let duration_ms = local.total_ms.max(started.elapsed().as_millis() as u64);

        tracing::info!(
            total_ms = local.total_ms,
            queue_wait_ms = local.queue_wait_ms,
            load_ms = local.load_ms,
            inference_ms = local.inference_ms,
            model = %local.model,
            "[dictation_stt] local ASR complete"
        );

        let word_count = local.transcript.split_whitespace().count();
        Ok(PreTranscript {
            transcript: local.transcript.clone(),
            meta: TranscriptMeta {
                enriched_transcript: local.transcript,
                confidence: 1.0,
                mean_word_confidence: 1.0,
                low_confidence_count: 0,
                word_count,
                languages: vec![local.language],
                model: local.model,
                duration_ms,
                origin: TranscriptOrigin::DictationLocal,
                ..TranscriptMeta::default()
            },
        })
    }
}

// ── Windows: Auto / On-device / Hosted (see module docs) ───────────────────
#[cfg(target_os = "windows")]
mod provider {
    use std::sync::OnceLock;

    use asr_cloud::{HostedSttClient, deepinfra};
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::PreTranscript;

    /// Provider family only — the exact hosted model can be overridden
    /// per-process (DEEPINFRA_STT_MODEL) and is logged per clip.
    const HOSTED_NAME: &str = "deepinfra";

    #[derive(Clone, Copy, PartialEq)]
    enum Selected {
        OnDevice,
        Hosted,
    }

    /// Resolve which provider the next clip uses (module-doc order: env pin →
    /// Settings pref → Auto). The pref is re-read per call so the Settings
    /// toggle applies immediately; the pieces that are per-session (env pin,
    /// hardware capability) are cached.
    fn selection() -> Selected {
        if let Some(pinned) = env_pin() {
            return pinned;
        }
        match said_core::prefs::load().dictation_stt.as_str() {
            "local" => Selected::OnDevice,
            "hosted" => Selected::Hosted,
            _ => auto_selection(),
        }
    }

    /// Diagnostics escape hatch: pins the provider for the whole session.
    fn env_pin() -> Option<Selected> {
        static PIN: OnceLock<Option<Selected>> = OnceLock::new();
        *PIN.get_or_init(|| {
            let value = std::env::var("AIRNOTE_DICTATION_STT_PROVIDER").ok()?;
            let pinned = match value.trim().to_ascii_lowercase().as_str() {
                "local" => Some(Selected::OnDevice),
                "hosted" => Some(Selected::Hosted),
                _ => None,
            };
            if let Some(p) = pinned {
                tracing::warn!(
                    provider = if p == Selected::OnDevice { "on-device" } else { "hosted" },
                    "[dictation_stt] AIRNOTE_DICTATION_STT_PROVIDER pins the provider this session (diagnostics mode)"
                );
            }
            pinned
        })
    }

    /// Auto: on-device iff this machine runs it well — the Vulkan GPU worker
    /// came up AND the local model is installed. Hardware doesn't change
    /// mid-session, so the answer is cached; the first call pays the worker
    /// spawn + probe (~1s, done at startup prewarm in practice).
    fn auto_selection() -> Selected {
        static AUTO: OnceLock<Selected> = OnceLock::new();
        *AUTO.get_or_init(|| {
            let gpu = crate::asr::gpu_active();
            let model = super::model_installed();
            let selected = if gpu && model {
                Selected::OnDevice
            } else {
                Selected::Hosted
            };
            tracing::info!(
                gpu_active = gpu,
                model_installed = model,
                resolved = if selected == Selected::OnDevice {
                    "on-device"
                } else {
                    "hosted"
                },
                "[dictation_stt] Auto provider resolved for this session"
            );
            selected
        })
    }

    fn name_of(s: Selected) -> &'static str {
        match s {
            Selected::OnDevice => super::on_device::name(),
            Selected::Hosted => HOSTED_NAME,
        }
    }

    pub(super) fn name() -> &'static str {
        name_of(selection())
    }

    pub(super) fn auto_name() -> &'static str {
        name_of(auto_selection())
    }

    pub(super) fn ready() -> bool {
        match selection() {
            Selected::OnDevice => super::on_device::ready(),
            Selected::Hosted => api_key().is_some(),
        }
    }

    pub(super) fn prewarm() {
        match selection() {
            Selected::OnDevice => super::on_device::prewarm(),
            Selected::Hosted => match client() {
                Ok(client) => tracing::info!(
                    model = client.model(),
                    "[dictation_stt] hosted provider ready"
                ),
                Err(e) => tracing::error!("[dictation_stt] hosted provider NOT ready: {e}"),
            },
        }
    }

    /// DeepInfra key baked into the build at compile time — same scheme as
    /// `DEEPSEEK_API_KEY` for meeting summaries: set `DEEPINFRA_API_KEY` in
    /// the build environment (scripts/build-windows.ps1 loads it from the
    /// repo-root .env) and it ships inside the binary; users never enter it.
    /// Dev builds without the bake fall back to a runtime env var.
    fn bundled_api_key() -> Option<String> {
        option_env!("DEEPINFRA_API_KEY")
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
    }

    fn api_key() -> Option<String> {
        bundled_api_key().or_else(|| {
            std::env::var(deepinfra::API_KEY_ENV)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
    }

    /// One process-wide client: reqwest pools the TLS connection, saving a
    /// handshake on every dictation after the first. Construction is deferred
    /// until the key resolves so a dev who exports the env var after launch
    /// isn't stuck with a cached failure.
    ///
    /// Ops knob: `DEEPINFRA_STT_MODEL` overrides the model id at process start
    /// (A/B-testing alternative hosted models without a rebuild). Unset =
    /// whisper-large-v3-turbo, the production default.
    fn client() -> Result<&'static HostedSttClient, String> {
        static CLIENT: OnceLock<HostedSttClient> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let key = api_key().ok_or_else(|| {
            format!(
                "Speech service unavailable — no DeepInfra key in this build (bake {} at build time, or set it as an env var).",
                deepinfra::API_KEY_ENV
            )
        })?;
        let mut cfg = deepinfra::config(key);
        if let Ok(model) = std::env::var("DEEPINFRA_STT_MODEL") {
            let model = model.trim();
            if !model.is_empty() {
                cfg.model = model.to_string();
            }
        }
        let client = HostedSttClient::new(cfg).map_err(|e| e.to_string())?;
        Ok(CLIENT.get_or_init(|| client))
    }

    pub(super) async fn transcribe(wav: &[u8], language: &str) -> Result<PreTranscript, String> {
        if selection() == Selected::OnDevice {
            return super::on_device::transcribe(wav, language).await;
        }
        let client = client()?;

        // Always send a language hint. Auto-detect labels Hinglish clips
        // "en"/"hi" inconsistently clip-to-clip, which flips the output script
        // mid-session; pinning Hindi keeps Hinglish stable (Devanagari Hindi,
        // English words preserved in Latin). A concrete user preference passes
        // through; the app's "auto" resolves to Hindi — our dictation default.
        let hint = match language {
            "" | "auto" => "hi",
            lang => lang,
        };
        let hosted = client
            .transcribe_wav(wav.to_vec(), Some(hint))
            .await
            .map_err(|e| e.to_string())?;

        tracing::info!(
            latency_ms = hosted.latency_ms,
            audio_secs = hosted.audio_secs.unwrap_or(0.0),
            detected_language = hosted.language.as_deref().unwrap_or("unreported"),
            requested_language = language,
            language_hint = hint,
            model = %hosted.model,
            "[dictation_stt] hosted transcription complete"
        );

        // An empty transcript is a terminal "nothing to type" outcome — fail
        // here with the honest message instead of letting an empty
        // pre_transcript die downstream as a backend 400.
        if hosted.text.is_empty() {
            return Err("No speech detected — try speaking again.".to_string());
        }

        let word_count = hosted.text.split_whitespace().count();
        Ok(PreTranscript {
            transcript: hosted.text.clone(),
            meta: TranscriptMeta {
                enriched_transcript: hosted.text,
                confidence: 1.0,
                mean_word_confidence: 1.0,
                low_confidence_count: 0,
                word_count,
                languages: hosted.language.into_iter().collect(),
                model: format!("deepinfra:{}", hosted.model),
                duration_ms: hosted.latency_ms,
                origin: TranscriptOrigin::DictationHosted,
                ..TranscriptMeta::default()
            },
        })
    }
}

// ── macOS (and other Unix): on-device whisper via asr-core ─────────────────
#[cfg(not(target_os = "windows"))]
mod provider {
    use super::PreTranscript;

    pub(super) fn name() -> &'static str {
        super::on_device::name()
    }

    pub(super) fn auto_name() -> &'static str {
        super::on_device::name()
    }

    pub(super) fn ready() -> bool {
        super::on_device::ready()
    }

    pub(super) fn prewarm() {
        super::on_device::prewarm();
    }

    pub(super) async fn transcribe(wav: &[u8], language: &str) -> Result<PreTranscript, String> {
        super::on_device::transcribe(wav, language).await
    }
}
