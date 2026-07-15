//! Dictation speech-to-text — the platform routing seam.
//!
//! | platform | provider |
//! |----------|----------|
//! | Windows / Intel Mac | live Together Nemotron (fixed) |
//! | Apple Silicon Mac | mandated local model, with an optional Cloud Nemotron switch in Settings |
//!
//! The platform policy in `stt_policy` is authoritative. A stale preference
//! cannot route Windows or Intel Macs to local ASR, and Apple Silicon has only
//! a deliberate local ↔ Cloud Nemotron choice.
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
/// logs): `"on-device/whisper"`, `"on-device/nemotron"`, or
/// `"together/nemotron-realtime"`.
pub fn provider_name() -> &'static str {
    provider::name()
}

/// True when dictation can transcribe right now (model present / key baked).
/// Network reachability is not probed here — an offline cloud call fails
/// fast with its own actionable error.
pub fn dictation_ready() -> bool {
    provider::ready()
}

/// The policy-default provider for diagnostic/status UI.
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
pub async fn transcribe_wav_bytes(wav: &[u8], language: &str) -> Result<PreTranscript, String> {
    provider::transcribe(wav, language).await
}

/// Transcribe a meeting-mode chunk with the shared 148 MB local Oriserve
/// model. This deliberately bypasses the platform dictation policy: Windows
/// dictation can use Together, but meeting audio must never go to hosted STT.
pub async fn transcribe_meeting_wav_bytes(
    wav: &[u8],
    language: &str,
) -> Result<PreTranscript, String> {
    crate::meeting_engine::require_meeting_local_model()?;

    let started = std::time::Instant::now();
    let wav = wav.to_vec();
    let language = language.to_string();
    let local =
        tokio::task::spawn_blocking(move || crate::asr::transcribe_wav_bytes(wav, language))
            .await
            .map_err(|error| format!("meeting local speech worker failed: {error}"))??;
    let duration_ms = local.total_ms.max(started.elapsed().as_millis() as u64);
    let word_count = local.transcript.split_whitespace().count();

    tracing::info!(
        total_ms = local.total_ms,
        model = %local.model,
        "[dictation_stt] local meeting ASR complete"
    );

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
            path: "meeting_local_batch".to_string(),
            duration_ms,
            origin: said_core::transcript::TranscriptOrigin::MeetingLocal,
            ..TranscriptMeta::default()
        },
    })
}

/// Warm this platform's provider at startup so the first utterance doesn't
/// pay setup costs: on-device pre-loads its model; live Nemotron resolves the
/// API key and logs loudly if the build shipped without one, so a broken build
/// is visible before the first dictation.
pub fn prewarm() {
    provider::prewarm();
}

/// True only for the cloud provider that has a live WebSocket transcription
/// contract. This is captured at recording start so changing Settings halfway
/// through a hold cannot switch transport beneath an active session.
pub fn uses_live_nemotron() -> bool {
    provider::uses_live_nemotron()
}

/// Run an already-open live Nemotron recording. The recorder owns audio
/// capture; this module owns provider credentials and converts the completed
/// Together response into the app's provider-neutral transcript contract.
pub async fn transcribe_live_nemotron(
    input: asr_cloud::LiveTranscriptionInput,
    event_tx: tokio::sync::mpsc::UnboundedSender<asr_cloud::LiveTranscriptEvent>,
) -> Result<PreTranscript, String> {
    provider::transcribe_live_nemotron(input, event_tx).await
}

// ── On-device engine (asr-core) — dictation on macOS; meetings everywhere ──
// Windows can select it too via the diagnostics escape hatch (module docs).
mod on_device {
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::PreTranscript;

    const WHISPER_NAME: &str = "on-device/whisper";
    const NEMOTRON_NAME: &str = "on-device/nemotron";

    fn uses_nemotron() -> bool {
        crate::nemotron::is_selected()
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
            crate::nemotron::selected_installed()
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
            if !crate::nemotron::selected_installed() {
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
                model = crate::nemotron::selected_model_name(),
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
                    model: format!("local:{}", crate::nemotron::selected_model_file()),
                    provider: "local_nemotron".to_string(),
                    path: "local_batch".to_string(),
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

// ── Enforced local / live Together Nemotron STT ─────────────────────────────
mod provider {
    use std::sync::OnceLock;

    use asr_cloud::{TogetherRealtimeClient, together};
    use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

    use super::PreTranscript;

    const TOGETHER_NEMOTRON_NAME: &str = "together/nemotron-realtime";

    #[derive(Clone, Copy, PartialEq)]
    enum Selected {
        OnDevice,
        TogetherNemotron,
    }

    /// Resolve the next clip from the central device policy. Only Apple
    /// Silicon permits a user-selected Cloud Nemotron route.
    fn selection() -> Selected {
        let policy = crate::stt_policy::current();
        if policy.is_cloud_locked() {
            return Selected::TogetherNemotron;
        }
        if said_core::prefs::load().dictation_stt == crate::stt_policy::CLOUD_NEMOTRON_PREF {
            Selected::TogetherNemotron
        } else {
            Selected::OnDevice
        }
    }

    fn name_of(s: Selected) -> &'static str {
        match s {
            Selected::OnDevice => super::on_device::name(),
            Selected::TogetherNemotron => TOGETHER_NEMOTRON_NAME,
        }
    }

    pub(super) fn name() -> &'static str {
        name_of(selection())
    }

    pub(super) fn uses_live_nemotron() -> bool {
        selection() == Selected::TogetherNemotron
    }

    pub(super) fn auto_name() -> &'static str {
        if crate::stt_policy::current().is_cloud_locked() {
            TOGETHER_NEMOTRON_NAME
        } else {
            super::on_device::name()
        }
    }

    pub(super) fn ready() -> bool {
        match selection() {
            Selected::OnDevice => super::on_device::ready(),
            Selected::TogetherNemotron => api_key().is_some(),
        }
    }

    pub(super) fn prewarm() {
        match selection() {
            Selected::OnDevice => super::on_device::prewarm(),
            Selected::TogetherNemotron => match nemotron_realtime_client() {
                Ok(client) => tracing::info!(
                    model = client.model(),
                    transport = "websocket",
                    "[dictation_stt] Together Nemotron provider ready"
                ),
                Err(e) => {
                    tracing::error!("[dictation_stt] Together Nemotron provider NOT ready: {e}")
                }
            },
        }
    }

    /// Together AI key baked into the build at compile time — same scheme as
    /// `DEEPSEEK_API_KEY` for meeting summaries: set `TOGETHER_API_KEY` in
    /// the build environment (scripts/build-windows.ps1 loads it from the
    /// repo-root .env) and it ships inside the binary; users never enter it.
    /// Dev builds without the bake fall back to a runtime env var.
    fn bundled_api_key() -> Option<String> {
        option_env!("TOGETHER_API_KEY")
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
    }

    fn api_key() -> Option<String> {
        bundled_api_key().or_else(|| {
            std::env::var(together::API_KEY_ENV)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
    }

    /// Nemotron's selected model is realtime-only in AirNote: it never shares
    /// the multipart Whisper client, so an accidental HTTP fallback is impossible.
    fn nemotron_realtime_client() -> Result<&'static TogetherRealtimeClient, String> {
        static CLIENT: OnceLock<TogetherRealtimeClient> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let key = api_key().ok_or_else(|| {
            format!(
                "Speech service unavailable — no Together AI key in this build (bake {} at build time, or set it as an env var).",
                together::API_KEY_ENV
            )
        })?;
        let client = TogetherRealtimeClient::nemotron(key).map_err(|e| e.to_string())?;
        Ok(CLIENT.get_or_init(|| client))
    }

    pub(super) async fn transcribe(wav: &[u8], language: &str) -> Result<PreTranscript, String> {
        let selected = selection();
        if selected == Selected::OnDevice {
            return super::on_device::transcribe(wav, language).await;
        }
        let transcription = match selected {
            Selected::TogetherNemotron => {
                // Only historical WAV retry/reprocess paths reach here. A
                // newly recorded dictation opens `transcribe_live_nemotron`
                // at key-down and never replays its completed WAV.
                let transcription = nemotron_realtime_client()?
                    .transcribe_wav(wav)
                    .await
                    .map_err(|e| e.to_string())?;
                transcription
            }
            Selected::OnDevice => unreachable!("on-device returned before cloud dispatch"),
        };

        tracing::info!(
            latency_ms = transcription.latency_ms,
            audio_secs = transcription.audio_secs.unwrap_or(0.0),
            detected_language = transcription.language.as_deref().unwrap_or("unreported"),
            requested_language = language,
            transport = "websocket",
            model = %transcription.model,
            "[dictation_stt] Together transcription complete"
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
                model: format!("together:{}", transcription.model),
                provider: "together".to_string(),
                path: "websocket_batch".to_string(),
                duration_ms: transcription.latency_ms,
                origin: TranscriptOrigin::DictationHosted,
                ..TranscriptMeta::default()
            },
        })
    }

    /// Finish a session that was opened at key-down and received PCM while the
    /// user spoke. This deliberately does not consult `selection()` again:
    /// the selection was captured when the session started, and a settings
    /// change during a recording must not reroute its final audio.
    pub(super) async fn transcribe_live_nemotron(
        input: asr_cloud::LiveTranscriptionInput,
        event_tx: tokio::sync::mpsc::UnboundedSender<asr_cloud::LiveTranscriptEvent>,
    ) -> Result<PreTranscript, String> {
        let transcription = nemotron_realtime_client()?
            .transcribe_live(input, event_tx)
            .await
            .map_err(|e| e.to_string())?;

        tracing::info!(
            latency_ms = transcription.latency_ms,
            audio_secs = transcription.audio_secs.unwrap_or(0.0),
            model = %transcription.model,
            transport = "websocket-live",
            "[dictation_stt] Together Nemotron live transcription complete"
        );

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
                model: format!("together:{}", transcription.model),
                provider: "together".to_string(),
                path: "websocket_live".to_string(),
                duration_ms: transcription.latency_ms,
                origin: TranscriptOrigin::DictationHosted,
                ..TranscriptMeta::default()
            },
        })
    }
}
