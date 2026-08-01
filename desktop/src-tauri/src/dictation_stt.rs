//! Dictation speech-to-text — the platform routing seam.
//!
//! | platform | provider |
//! |----------|----------|
//! | Windows / Intel Mac | selected hosted STT route |
//! | Apple Silicon Mac | local model or a selected hosted STT route |
//!
//! The platform policy in `stt_policy` is authoritative. A stale preference
//! cannot route Windows or Intel Macs to local ASR. Everything downstream is
//! provider-agnostic: the transcript feeds the existing polish route unchanged.
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

/// Stable identifier of the provider the next dictation will use (status UI
/// and logs).
pub fn provider_name() -> &'static str {
    provider::name()
}

/// True when dictation can transcribe right now (model present / key baked).
/// Network reachability is not probed here — an offline cloud call fails
/// fast with its own actionable error.
pub fn dictation_ready() -> bool {
    provider::ready()
}

/// The policy-resolved provider for diagnostic/status UI.
pub fn auto_provider_name() -> &'static str {
    provider::auto_name()
}

// ── On-device model status ─────────────────────────────────────────────────
// Oriserve/whisper.cpp remains the meetings engine on every platform. For
// dictation it is used only when the Apple-Silicon device policy selects it.

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
pub async fn transcribe_wav_bytes(
    wav: &[u8],
    language: &str,
    recording_id: Option<&str>,
) -> Result<PreTranscript, String> {
    provider::transcribe(wav, language, recording_id).await
}

/// Transcribe a legacy meeting-mode chunk with the native meeting provider.
/// Capable Apple Silicon uses the same Q4 model recommended at onboarding;
/// Windows and Intel Macs stay on local Oriserve and never use hosted STT.
pub async fn transcribe_meeting_wav_bytes(
    wav: &[u8],
    language: &str,
) -> Result<PreTranscript, String> {
    crate::meeting_engine::require_meeting_local_model()?;

    let started = std::time::Instant::now();
    let wav = wav.to_vec();
    let language = language.to_string();
    let use_nemotron_q4 = crate::meeting_engine::meetings_use_nemotron_q4();
    let local = tokio::task::spawn_blocking(move || {
        if use_nemotron_q4 {
            let output = crate::nemotron::transcribe_wav_bytes_for(
                crate::nemotron::Variant::Q4,
                &wav,
                &language,
            )?;
            Ok::<_, String>((
                output.transcript,
                output.language.unwrap_or(language),
                crate::nemotron::Variant::Q4.display_name().to_string(),
                output.duration_ms,
                "local_nemotron".to_string(),
            ))
        } else {
            let output = crate::asr::transcribe_wav_bytes(wav, language)?;
            Ok((
                output.transcript,
                output.language,
                output.model,
                output.total_ms,
                "local_whisper".to_string(),
            ))
        }
    })
    .await
    .map_err(|error| format!("meeting local speech worker failed: {error}"))??;
    let (transcript, language, model, local_duration_ms, provider) = local;
    let duration_ms = local_duration_ms.max(started.elapsed().as_millis() as u64);
    let word_count = transcript.split_whitespace().count();

    tracing::info!(
        total_ms = local_duration_ms,
        model = %model,
        "[dictation_stt] local meeting ASR complete"
    );

    Ok(PreTranscript {
        transcript: transcript.clone(),
        meta: TranscriptMeta {
            enriched_transcript: transcript,
            confidence: 1.0,
            mean_word_confidence: 1.0,
            low_confidence_count: 0,
            word_count,
            languages: vec![language],
            model,
            provider,
            path: "meeting_local_batch".to_string(),
            duration_ms,
            origin: said_core::transcript::TranscriptOrigin::MeetingLocal,
            ..TranscriptMeta::default()
        },
    })
}

/// Warm this platform's provider at startup so the first utterance doesn't pay
/// setup costs. Cloud routes resolve their key and log loudly if the build
/// shipped without it, so a broken configuration is visible before dictation.
pub fn prewarm() {
    provider::prewarm();
}

// ── On-device engine (asr-core) — dictation on macOS; meetings everywhere ──
// Windows can select it too via the diagnostics escape hatch (module docs).
mod on_device {
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::PreTranscript;

    const WHISPER_NAME: &str = "on-device/whisper";
    const TRANSCRIBE_CPP_NAME: &str = "on-device/transcribe-cpp";

    fn uses_catalog_model() -> bool {
        crate::local_transcribe::is_selected()
    }

    pub(super) fn name() -> &'static str {
        if uses_catalog_model() {
            TRANSCRIBE_CPP_NAME
        } else {
            WHISPER_NAME
        }
    }

    pub(super) fn ready() -> bool {
        if uses_catalog_model() {
            crate::local_transcribe::selected_installed()
        } else {
            super::model_installed() && super::runtime_ready()
        }
    }

    /// Pre-load the whisper model so the first utterance skips the model load.
    pub(super) fn prewarm() {
        if uses_catalog_model() {
            crate::local_transcribe::prewarm();
        } else {
            crate::asr::prewarm_default_language();
        }
    }

    pub(super) async fn transcribe(
        wav: &[u8],
        language: &str,
        recording_id: Option<&str>,
    ) -> Result<PreTranscript, String> {
        if uses_catalog_model() {
            if !crate::local_transcribe::selected_installed() {
                return Err(format!(
                    "{} is selected but not installed. Download it in Settings → Speech recognition.",
                    crate::local_transcribe::selected_model_name()
                ));
            }

            let wav = wav.to_vec();
            let language = language.to_string();
            let recording_id = recording_id.map(str::to_string);
            let output = tokio::task::spawn_blocking(move || {
                crate::local_transcribe::transcribe_wav_bytes(
                    &wav,
                    &language,
                    recording_id.as_deref(),
                )
            })
            .await
            .map_err(|error| format!("Local speech worker failed: {error}"))??;
            let word_count = output.transcript.split_whitespace().count();
            tracing::info!(
                duration_ms = output.duration_ms,
                language = output.language.as_deref().unwrap_or("unreported"),
                model = crate::local_transcribe::selected_model_name(),
                streamed = output.streamed,
                "[dictation_stt] catalog local ASR complete"
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
                    model: format!("local:{}", crate::local_transcribe::selected_model_file()),
                    provider: "local_transcribe_cpp".to_string(),
                    path: if output.streamed {
                        "local_stream".to_string()
                    } else {
                        "local_batch".to_string()
                    },
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
                provider: "local_whisper".to_string(),
                path: "local_batch".to_string(),
                duration_ms,
                origin: TranscriptOrigin::DictationLocal,
                ..TranscriptMeta::default()
            },
        })
    }
}

// ── Selected local / cloud completed-recording STT ──────────────────────────
mod provider {
    use std::sync::OnceLock;

    use asr_cloud::{
        DEEPINFRA_API_KEY_ENV, DeepInfraClient, ELEVEN_LABS_API_KEY_ENV, ElevenLabsClient,
    };
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::PreTranscript;

    const DEEPINFRA_WHISPER_NAME: &str = "deepinfra/whisper-large-v3-turbo";
    const ELEVENLABS_SCRIBE_V2_NAME: &str = "elevenlabs/scribe-v2";

    #[derive(Clone, Copy, PartialEq)]
    enum Selected {
        OnDevice,
        DeepInfraWhisper,
        ElevenLabsScribeV2,
    }

    /// Resolve the next clip from the central device policy. Cloud-only
    /// devices may choose between hosted routes; Apple Silicon may also use
    /// its local model.
    fn selection() -> Selected {
        let policy = crate::stt_policy::current();
        if policy.is_cloud_locked() {
            return cloud_selection();
        }
        match said_core::prefs::load().dictation_stt.as_str() {
            crate::stt_policy::CLOUD_DEEPINFRA_PREF => Selected::DeepInfraWhisper,
            crate::stt_policy::CLOUD_ELEVENLABS_PREF => Selected::ElevenLabsScribeV2,
            _ => Selected::OnDevice,
        }
    }

    fn cloud_selection() -> Selected {
        match said_core::prefs::load().dictation_stt.as_str() {
            crate::stt_policy::CLOUD_ELEVENLABS_PREF => Selected::ElevenLabsScribeV2,
            _ => Selected::DeepInfraWhisper,
        }
    }

    fn name_of(s: Selected) -> &'static str {
        match s {
            Selected::OnDevice => super::on_device::name(),
            Selected::DeepInfraWhisper => DEEPINFRA_WHISPER_NAME,
            Selected::ElevenLabsScribeV2 => ELEVENLABS_SCRIBE_V2_NAME,
        }
    }

    pub(super) fn name() -> &'static str {
        name_of(selection())
    }

    pub(super) fn auto_name() -> &'static str {
        name_of(selection())
    }

    pub(super) fn ready() -> bool {
        match selection() {
            Selected::OnDevice => super::on_device::ready(),
            Selected::DeepInfraWhisper => deepinfra_api_key().is_some(),
            Selected::ElevenLabsScribeV2 => elevenlabs_api_key().is_some(),
        }
    }

    pub(super) fn prewarm() {
        match selection() {
            Selected::OnDevice => super::on_device::prewarm(),
            Selected::DeepInfraWhisper => match deepinfra_client() {
                Ok(client) => tracing::info!(
                    model = client.model(),
                    transport = "http-batch",
                    "[dictation_stt] DeepInfra Whisper provider ready"
                ),
                Err(e) => {
                    tracing::error!("[dictation_stt] DeepInfra Whisper provider NOT ready: {e}")
                }
            },
            Selected::ElevenLabsScribeV2 => match elevenlabs_client() {
                Ok(client) => tracing::info!(
                    model = client.model(),
                    transport = "http-completed-recording",
                    "[dictation_stt] ElevenLabs Scribe v2 provider ready"
                ),
                Err(e) => {
                    tracing::error!(
                        "[dictation_stt] ElevenLabs transcription provider NOT ready: {e}"
                    )
                }
            },
        }
    }

    /// Cloud keys are baked into distributable builds. Dev builds read the
    /// runtime environment after `said_core::load_env()` loads the repo `.env`.
    fn api_key(bundled: Option<&str>, env_var: &str) -> Option<String> {
        bundled
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .or_else(|| {
                std::env::var(env_var)
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            })
    }

    fn deepinfra_api_key() -> Option<String> {
        api_key(option_env!("DEEPINFRA_API_KEY"), DEEPINFRA_API_KEY_ENV)
    }

    fn deepinfra_client() -> Result<&'static DeepInfraClient, String> {
        static CLIENT: OnceLock<DeepInfraClient> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let key = deepinfra_api_key().ok_or_else(|| {
            format!(
                "Speech service unavailable — no DeepInfra key in this build (bake {DEEPINFRA_API_KEY_ENV} at build time, or set it as an env var)."
            )
        })?;
        let client = DeepInfraClient::new(key).map_err(|e| e.to_string())?;
        Ok(CLIENT.get_or_init(|| client))
    }

    fn elevenlabs_api_key() -> Option<String> {
        api_key(option_env!("ELEVEN_LABS_API_KEY"), ELEVEN_LABS_API_KEY_ENV)
    }

    fn elevenlabs_client() -> Result<&'static ElevenLabsClient, String> {
        static CLIENT: OnceLock<ElevenLabsClient> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let key = elevenlabs_api_key().ok_or_else(|| {
            format!(
                "Speech service unavailable — no ElevenLabs key in this build (bake {ELEVEN_LABS_API_KEY_ENV} at build time, or set it as an env var)."
            )
        })?;
        let client = ElevenLabsClient::new(key).map_err(|e| e.to_string())?;
        Ok(CLIENT.get_or_init(|| client))
    }

    pub(super) async fn transcribe(
        wav: &[u8],
        language: &str,
        recording_id: Option<&str>,
    ) -> Result<PreTranscript, String> {
        let selected = selection();
        if selected == Selected::OnDevice {
            return super::on_device::transcribe(wav, language, recording_id).await;
        }
        let transcription = match selected {
            Selected::DeepInfraWhisper => {
                let transcription = deepinfra_client()?
                    .transcribe_wav(wav)
                    .await
                    .map_err(|e| e.to_string())?;
                transcription
            }
            Selected::ElevenLabsScribeV2 => elevenlabs_client()?
                .transcribe_wav(wav)
                .await
                .map_err(|e| e.to_string())?,
            Selected::OnDevice => unreachable!("on-device returned before cloud dispatch"),
        };

        tracing::info!(
            latency_ms = transcription.latency_ms,
            audio_secs = transcription.audio_secs.unwrap_or(0.0),
            detected_language = transcription.language.as_deref().unwrap_or("unreported"),
            requested_language = language,
            transport = "http-completed-recording",
            model = %transcription.model,
            provider = %name_of(selected),
            "[dictation_stt] cloud transcription complete"
        );

        // An empty transcript is a terminal "nothing to type" outcome — fail
        // here with the honest message instead of letting an empty
        // pre_transcript die downstream as a backend 400.
        if transcription.text.is_empty() {
            return Err("No speech detected — try speaking again.".to_string());
        }

        let word_count = transcription.text.split_whitespace().count();
        Ok(PreTranscript {
            transcript: transcription.text.clone(),
            meta: TranscriptMeta {
                enriched_transcript: transcription.text,
                confidence: 1.0,
                mean_word_confidence: 1.0,
                low_confidence_count: 0,
                word_count,
                languages: transcription.language.into_iter().collect(),
                model: format!("{}:{}", provider_id(selected), transcription.model),
                provider: provider_id(selected).to_string(),
                path: "http_batch".to_string(),
                duration_ms: transcription.latency_ms,
                origin: TranscriptOrigin::DictationHosted,
                ..TranscriptMeta::default()
            },
        })
    }

    fn provider_id(selected: Selected) -> &'static str {
        match selected {
            Selected::OnDevice => "local",
            Selected::DeepInfraWhisper => "deepinfra",
            Selected::ElevenLabsScribeV2 => "elevenlabs",
        }
    }
}
