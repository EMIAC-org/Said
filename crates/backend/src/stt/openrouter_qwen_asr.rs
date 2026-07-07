//! Cloud dictation STT via OpenRouter → OpenAI Whisper Large V3 Turbo (batch).
//!
//! This is the ONLY cloud dictation transcriber. The old Deepgram cloud path is
//! gone from dictation; cloud dictation now batch-transcribes the whole WAV on
//! release through OpenRouter. Meetings + control-plane keep their own STT.
//!
//! Docs: https://openrouter.ai/docs/guides/overview/multimodal/stt
//! Model: https://openrouter.ai/openai/whisper-large-v3-turbo (Groq-hosted)

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use said_core::deepgram::BiasPackage;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::deepgram::TranscriptResult;

const TRANSCRIPTIONS_URL: &str = "https://openrouter.ai/api/v1/audio/transcriptions";
/// OpenAI Whisper Large V3 Turbo — overridable with `OPENROUTER_STT_MODEL`.
const DEFAULT_MODEL: &str = "openai/whisper-large-v3-turbo";

/// Resolve the OpenRouter key: runtime env (dev/server) → build-time bundled key
/// (shipped app, baked from the build env like the old Deepgram key). End users
/// never enter a key; cloud dictation is server-managed.
pub fn resolve_api_key() -> Option<String> {
    non_empty(std::env::var("OPENROUTER_API_KEY").ok())
        .or_else(|| non_empty(option_env!("OPENROUTER_API_KEY").map(str::to_string)))
}

fn model_id() -> String {
    std::env::var("OPENROUTER_STT_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

#[derive(Serialize)]
struct SttRequest<'a> {
    model: &'a str,
    input_audio: InputAudio<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
}

#[derive(Serialize)]
struct InputAudio<'a> {
    data: String,
    format: &'a str,
}

#[derive(Deserialize)]
struct SttResponse {
    text: String,
    #[serde(default)]
    usage: Option<SttUsage>,
}

#[derive(Deserialize)]
struct SttUsage {
    #[serde(default)]
    seconds: Option<f64>,
    #[serde(default)]
    cost: Option<f64>,
}

pub async fn transcribe(
    client: &Client,
    api_key: &str,
    wav_data: Vec<u8>,
    bias: &BiasPackage,
) -> Result<TranscriptResult, String> {
    if api_key.trim().is_empty() {
        return Err("OPENROUTER_API_KEY is not set".into());
    }
    if wav_data.is_empty() {
        return Err("empty audio for OpenRouter STT".into());
    }

    let model = model_id();
    // Auto-detect language: let Whisper code-switch (Hindi/English) per segment.
    // Forcing "hi" biased code-mixed Hinglish toward Devanagari and mangled the
    // embedded English words.
    let lang: Option<&str> = None;
    let audio_b64 = B64.encode(&wav_data);

    debug!(
        "[openrouter-stt] sending {} bytes model={} lang=auto requested_mode={}",
        wav_data.len(),
        model,
        bias.stt_mode,
    );

    let body = SttRequest {
        model: &model,
        input_audio: InputAudio {
            data: audio_b64,
            format: "wav",
        },
        language: lang,
    };

    let resp = client
        .post(TRANSCRIPTIONS_URL)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("Content-Type", "application/json")
        .header("HTTP-Referer", "https://airnote.app")
        .header("X-Title", "AirNote")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("OpenRouter STT request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let preview = said_core::text::truncate_utf8(&body, 400);
        return Err(format!("OpenRouter STT error {status}: {preview}"));
    }

    let parsed: SttResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse OpenRouter STT response: {e}"))?;

    let transcript = parsed.text.trim().to_string();
    if transcript.is_empty() {
        return Err("OpenRouter STT returned empty transcript".into());
    }

    let word_count = transcript.split_whitespace().count();
    let lang_label = lang.unwrap_or("auto");
    info!(
        "[openrouter-stt] done words={} audio_s={:?} cost_usd={:?}: {:?}",
        word_count,
        parsed.usage.as_ref().and_then(|u| u.seconds),
        parsed.usage.as_ref().and_then(|u| u.cost),
        said_core::text::truncate_utf8(&transcript, 120),
    );

    Ok(TranscriptResult {
        transcript: transcript.clone(),
        enriched_transcript: transcript,
        confidence: 0.9,
        uncertain_count: 0,
        mean_word_confidence: 0.9,
        word_count,
        languages: vec![lang_label.to_string()],
        stt_mode: format!("openrouter_whisper:{lang_label}"),
    })
}
