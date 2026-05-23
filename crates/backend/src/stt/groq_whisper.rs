//! Groq Whisper STT — whisper-large-v3-turbo via Groq's audio API.
//!
//! Sends WAV audio to `POST https://api.groq.com/openai/v1/audio/transcriptions`
//! and returns the transcript. Uses the same Groq API key as the LLM polish step.

use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

use super::deepgram::TranscriptResult;

const GROQ_WHISPER_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const MODEL: &str = "whisper-large-v3-turbo";

#[derive(Deserialize)]
struct GroqWhisperResponse {
    text: String,
}

#[derive(Deserialize)]
struct GroqVerboseResponse {
    text: String,
    #[serde(default)]
    segments: Vec<GroqSegment>,
}

#[derive(Deserialize)]
struct GroqSegment {
    #[serde(default)]
    text: String,
    #[serde(default)]
    avg_logprob: f64,
    #[serde(default)]
    no_speech_prob: f64,
}

pub async fn transcribe(
    client: &Client,
    api_key: &str,
    wav_data: Vec<u8>,
    language: &str,
) -> Result<TranscriptResult, String> {
    if api_key.is_empty() {
        return Err("no Groq API key for whisper STT".into());
    }

    let lang = match language {
        "en" | "english" => "en",
        "hi" | "hindi" | "hinglish" => "hi",
        "auto" | "multi" | "" => "hi",
        other => other,
    };

    debug!(
        "[groq-whisper] sending {} bytes, lang={lang}, model={MODEL}",
        wav_data.len()
    );

    let file_part = reqwest::multipart::Part::bytes(wav_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("failed to build multipart: {e}"))?;

    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("model", MODEL)
        .text("language", lang.to_string())
        .text("response_format", "verbose_json")
        .text("temperature", "0");

    let resp = client
        .post(GROQ_WHISPER_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Groq whisper request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let preview = &body[..body.len().min(300)];
        return Err(format!("Groq whisper error {status}: {preview}"));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| format!("failed to read Groq whisper response: {e}"))?;

    let verbose: GroqVerboseResponse =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse Groq response: {e}"))?;

    let transcript = verbose.text.trim().to_string();
    if transcript.is_empty() {
        return Err("Groq whisper returned empty transcript".into());
    }

    let word_count = transcript.split_whitespace().count();
    let avg_confidence = if verbose.segments.is_empty() {
        0.85
    } else {
        let avg_logprob = verbose.segments.iter().map(|s| s.avg_logprob).sum::<f64>()
            / verbose.segments.len() as f64;
        logprob_to_confidence(avg_logprob)
    };

    info!(
        "[groq-whisper] {} words, conf={:.2}: {:?}",
        word_count,
        avg_confidence,
        truncate_utf8(&transcript, 120),
    );

    Ok(TranscriptResult {
        transcript: transcript.clone(),
        enriched_transcript: transcript,
        confidence: avg_confidence,
        uncertain_count: 0,
        mean_word_confidence: avg_confidence,
        word_count,
        languages: vec![lang.to_string()],
        stt_mode: format!("groq_whisper:{lang}"),
    })
}

fn logprob_to_confidence(avg_logprob: f64) -> f64 {
    avg_logprob.exp().clamp(0.0, 1.0)
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
