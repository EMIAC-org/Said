//! Experimental batch STT via OpenRouter → Qwen3-ASR-Flash.
//!
//! Opt-in only (`AIRNOTE_BATCH_STT_BACKEND=openrouter_qwen`). Delete this file
//! and the two call sites in `routes/voice.rs` + `mod.rs` to remove entirely.
//!
//! Docs: https://openrouter.ai/docs/guides/overview/multimodal/stt
//! Model: https://openrouter.ai/qwen/qwen3-asr-flash-2026-02-10

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use said_core::deepgram::BiasPackage;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::deepgram::TranscriptResult;

const TRANSCRIPTIONS_URL: &str = "https://openrouter.ai/api/v1/audio/transcriptions";
const DEFAULT_MODEL: &str = "qwen/qwen3-asr-flash-2026-02-10";

/// `openrouter_qwen` → route batch STT through OpenRouter. Anything else → Deepgram.
pub fn is_enabled() -> bool {
    std::env::var("AIRNOTE_BATCH_STT_BACKEND")
        .map(|v| v.trim().eq_ignore_ascii_case("openrouter_qwen"))
        .unwrap_or(false)
}

/// When true, ignore inbound local pre-transcripts so batch OpenRouter runs on the WAV.
pub fn force_cloud_batch() -> bool {
    is_enabled()
}

pub fn resolve_api_key() -> Option<String> {
    non_empty(std::env::var("OPENROUTER_API_KEY").ok())
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

fn language_hint(stt_mode: &str) -> Option<&'static str> {
    match stt_mode.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => Some("en"),
        "hi" | "hindi" | "hinglish" | "multi" => Some("hi"),
        // Omit for auto-detect on code-mixed audio.
        _ => None,
    }
}

fn context_from_bias(bias: &BiasPackage) -> Option<String> {
    if bias.keyterms.is_empty() && bias.replacements.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !bias.keyterms.is_empty() {
        parts.push(format!(
            "Prefer these terms when heard: {}.",
            bias.keyterms.join(", ")
        ));
    }
    if !bias.replacements.is_empty() {
        let rules: Vec<String> = bias
            .replacements
            .iter()
            .map(|r| {
                if let Some(rep) = r.replace.as_deref().filter(|s| !s.is_empty()) {
                    format!("{} → {}", r.find, rep)
                } else {
                    r.find.clone()
                }
            })
            .collect();
        parts.push(format!("Vocabulary hints: {}.", rules.join("; ")));
    }
    Some(parts.join(" "))
}

#[derive(Serialize)]
struct SttRequest<'a> {
    model: &'a str,
    input_audio: InputAudio<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<ProviderPassthrough<'a>>,
}

#[derive(Serialize)]
struct InputAudio<'a> {
    data: String,
    format: &'a str,
}

#[derive(Serialize)]
struct ProviderPassthrough<'a> {
    options: ProviderOptions<'a>,
}

#[derive(Serialize)]
struct ProviderOptions<'a> {
    qwen: QwenOptions<'a>,
}

#[derive(Serialize)]
struct QwenOptions<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,
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
    let lang = language_hint(&bias.stt_mode);
    let context = context_from_bias(bias);
    let audio_b64 = B64.encode(&wav_data);

    debug!(
        "[openrouter-stt] sending {} bytes model={} lang={:?} keyterms={}",
        wav_data.len(),
        model,
        lang,
        bias.keyterms.len(),
    );

    let body = SttRequest {
        model: &model,
        input_audio: InputAudio {
            data: audio_b64,
            format: "wav",
        },
        language: lang,
        provider: context.as_deref().map(|ctx| ProviderPassthrough {
            options: ProviderOptions {
                qwen: QwenOptions { context: Some(ctx) },
            },
        }),
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
        stt_mode: format!("openrouter_qwen:{lang_label}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_when_env_set() {
        let enabled = std::env::var("AIRNOTE_BATCH_STT_BACKEND")
            .map(|v| v.trim().eq_ignore_ascii_case("openrouter_qwen"))
            .unwrap_or(false);
        assert_eq!(is_enabled(), enabled);
    }

    #[test]
    fn language_hint_maps_hinglish_to_hi() {
        assert_eq!(language_hint("hinglish"), Some("hi"));
        assert_eq!(language_hint("multi"), Some("hi"));
        assert_eq!(language_hint("en"), Some("en"));
    }
}
