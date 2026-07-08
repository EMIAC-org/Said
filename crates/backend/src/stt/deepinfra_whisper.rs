//! Cloud dictation STT via DeepInfra Whisper Large V3 (batch).
//!
//! This is the cloud dictation transcriber for the `"deepgram"` compatibility
//! provider id. Polish model routing is separate and still uses OpenRouter for
//! the Gemma polish route.

use reqwest::{Client, multipart};
use said_core::deepgram::BiasPackage;
use serde::Deserialize;
use tracing::{debug, info};

use super::deepgram::TranscriptResult;

const TRANSCRIPTIONS_URL: &str = "https://api.deepinfra.com/v1/inference/openai/whisper-large-v3";
pub const DEFAULT_MODEL: &str = "openai/whisper-large-v3";

/// Resolve the DeepInfra key: runtime env (dev/server) → build-time bundled key
/// (shipped app). End users never enter a key; cloud dictation is server-managed.
pub fn resolve_api_key() -> Option<String> {
    said_core::stt::resolve_deepinfra_api_key()
}

#[derive(Deserialize)]
struct SttResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Vec<SttSegment>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    input_length_ms: Option<f64>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    inference_status: Option<InferenceStatus>,
}

#[derive(Deserialize)]
struct SttSegment {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct InferenceStatus {
    #[serde(default)]
    runtime_ms: Option<f64>,
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
        return Err("DEEPINFRA_API_KEY is not set".into());
    }
    if wav_data.is_empty() {
        return Err("empty audio for DeepInfra STT".into());
    }

    // Force Hindi for Hinglish dictation. Direct DeepInfra tests show omitted
    // language often detects these Hindi-English clips as English and translates
    // them; `hi` preserves Hindi speech while still keeping English terms.
    // `task=transcribe` keeps it from translating to English.
    debug!(
        "[deepinfra-stt] sending {} bytes model={} lang=auto requested_mode={}",
        wav_data.len(),
        DEFAULT_MODEL,
        bias.stt_mode,
    );

    let audio = multipart::Part::bytes(wav_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("failed to build DeepInfra STT audio part: {e}"))?;
    let form = multipart::Form::new()
        .part("audio", audio)
        .text("language", "hi")
        // Force transcription rather than English translation/normalization.
        .text("task", "transcribe");

    let resp = client
        .post(TRANSCRIPTIONS_URL)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("DeepInfra STT request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let preview = said_core::text::truncate_utf8(&body, 400);
        return Err(format!("DeepInfra STT error {status}: {preview}"));
    }

    let parsed: SttResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse DeepInfra STT response: {e}"))?;

    let transcript = if parsed.text.trim().is_empty() {
        parsed
            .segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        parsed.text.trim().to_string()
    };
    if transcript.is_empty() {
        return Err("DeepInfra STT returned empty transcript".into());
    }

    let word_count = transcript.split_whitespace().count();
    let lang_label = parsed.language.as_deref().unwrap_or("auto").to_string();
    info!(
        "[deepinfra-stt] done words={} input_ms={:?} duration_s={:?} runtime_ms={:?} cost_usd={:?}: {:?}",
        word_count,
        parsed.input_length_ms,
        parsed.duration,
        parsed
            .inference_status
            .as_ref()
            .and_then(|status| status.runtime_ms),
        parsed
            .inference_status
            .as_ref()
            .and_then(|status| status.cost),
        said_core::text::truncate_utf8(&transcript, 120),
    );

    Ok(TranscriptResult {
        transcript: transcript.clone(),
        enriched_transcript: transcript,
        confidence: 0.9,
        uncertain_count: 0,
        mean_word_confidence: 0.9,
        word_count,
        languages: vec![lang_label.clone()],
        stt_mode: format!("deepinfra_whisper:{lang_label}"),
    })
}
