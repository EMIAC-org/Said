//! Sarvam Saaras v3 batch STT — REST API with 29s chunking for longer clips.
//!
//! Docs: https://docs.sarvam.ai/api-reference-docs/speech-to-text/transcribe

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info};

use super::deepgram::TranscriptResult;

const SARVAM_REST_URL: &str = "https://api.sarvam.ai/speech-to-text";
const CHUNK_SECONDS: f64 = 29.0;

#[derive(Deserialize)]
struct SarvamRestResponse {
    transcript: Option<String>,
}

pub fn wav_duration_secs(wav: &[u8]) -> f64 {
    if wav.len() < 44 {
        return 0.0;
    }
    let byte_rate = u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]) as f64;
    let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as f64;
    if byte_rate > 0.0 {
        data_size / byte_rate
    } else {
        0.0
    }
}

fn pcm_chunk_to_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_size = pcm.len() as u32;
    let byte_rate = sample_rate * 2;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

fn wav_pcm_chunks(wav: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if wav.len() < 44 {
        return Err("invalid WAV: too short".into());
    }
    if &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("invalid WAV header".into());
    }
    let sample_rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
    let channels = u16::from_le_bytes([wav[22], wav[23]]);
    let bits = u16::from_le_bytes([wav[34], wav[35]]);
    if channels != 1 || bits != 16 {
        return Err(format!(
            "sarvam expects mono 16-bit PCM WAV (got {channels}ch {bits}bit)"
        ));
    }
    let data_offset = wav
        .windows(4)
        .position(|w| w == b"data")
        .map(|i| i + 8)
        .ok_or("invalid WAV: missing data chunk")?;
    if data_offset + 4 > wav.len() {
        return Err("invalid WAV: truncated data chunk".into());
    }
    let data_size = u32::from_le_bytes([
        wav[data_offset - 4],
        wav[data_offset - 3],
        wav[data_offset - 2],
        wav[data_offset - 1],
    ]) as usize;
    let pcm = &wav[data_offset..data_offset.saturating_add(data_size).min(wav.len())];
    let bytes_per_second = (sample_rate as f64 * 2.0) as usize;
    let chunk_bytes = (bytes_per_second as f64 * CHUNK_SECONDS) as usize;
    if chunk_bytes == 0 {
        return Err("invalid WAV sample rate".into());
    }
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    while offset < pcm.len() {
        let end = (offset + chunk_bytes).min(pcm.len());
        let slice = &pcm[offset..end];
        if !slice.is_empty() {
            chunks.push(pcm_chunk_to_wav(slice, sample_rate));
        }
        offset = end;
    }
    Ok(chunks)
}

async fn transcribe_chunk(
    client: &Client,
    api_key: &str,
    wav_chunk: Vec<u8>,
    mode: &str,
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(wav_chunk)
        .file_name("chunk.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new()
        .text("model", "saaras:v3")
        .text("mode", mode.to_string())
        .text("language_code", "hi-IN")
        .part("file", part);

    let resp = client
        .post(SARVAM_REST_URL)
        .header("api-subscription-key", api_key)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("sarvam request failed: {e}"))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("sarvam read body: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "sarvam HTTP {status}: {}",
            &body[..body.len().min(400)]
        ));
    }
    let parsed: SarvamRestResponse =
        serde_json::from_str(&body).map_err(|e| format!("sarvam json: {e}: {body}"))?;
    Ok(parsed.transcript.unwrap_or_default().trim().to_string())
}

/// Batch transcribe full WAV. Uses `codemix` for Hinglish-friendly Roman output.
pub async fn transcribe(
    client: &Client,
    api_key: &str,
    wav_data: Vec<u8>,
    mode: &str,
) -> Result<TranscriptResult, String> {
    let chunks = wav_pcm_chunks(&wav_data)?;
    debug!(
        "[sarvam] transcribing {} bytes in {} chunk(s), mode={mode}",
        wav_data.len(),
        chunks.len()
    );
    let mut parts = Vec::new();
    for (idx, chunk) in chunks.into_iter().enumerate() {
        let text = transcribe_chunk(client, api_key, chunk, mode).await?;
        if !text.is_empty() {
            parts.push(text);
        } else {
            debug!("[sarvam] chunk {idx} returned empty transcript");
        }
    }
    let transcript = parts.join(" ").trim().to_string();
    if transcript.is_empty() {
        return Err("sarvam returned empty transcript".into());
    }
    let word_count = transcript.split_whitespace().count();
    info!("[sarvam] {} words in {:?} mode", word_count, mode);
    Ok(TranscriptResult {
        transcript: transcript.clone(),
        enriched_transcript: transcript,
        confidence: 0.92,
        uncertain_count: 0,
        mean_word_confidence: 0.92,
        word_count,
        languages: vec!["hi-IN".into()],
        stt_mode: format!("sarvam:{mode}"),
    })
}

/// Wrap raw PCM16 mono into a mini WAV for Sarvam WS `audio/wav` frames.
pub fn pcm16_to_mini_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    pcm_chunk_to_wav(pcm, sample_rate)
}

/// Base64 JSON audio frame for Sarvam streaming WS.
pub fn ws_audio_message(pcm: &[u8], sample_rate: u32) -> String {
    let wav = pcm16_to_mini_wav(pcm, sample_rate);
    serde_json::json!({
        "audio": {
            "data": B64.encode(wav),
            "sample_rate": sample_rate,
            "encoding": "audio/wav"
        }
    })
    .to_string()
}

pub const WS_FLUSH_MESSAGE: &str = r#"{"type":"flush"}"#;

pub fn ws_connect_url(sample_rate: u32, mode: &str) -> String {
    let params = [
        ("model", "saaras:v3"),
        ("mode", mode),
        ("language-code", "hi-IN"),
        ("sample_rate", &sample_rate.to_string()),
        ("high_vad_sensitivity", "true"),
        ("flush_signal", "true"),
        ("vad_signals", "true"),
    ];
    let query: String = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("wss://api.sarvam.ai/speech-to-text/ws?{query}")
}

#[derive(Debug)]
pub struct SarvamWsEvent {
    pub transcript: String,
    pub is_final: bool,
}

/// Parse Sarvam WS JSON payload into a normalized transcript event.
pub fn parse_ws_message(text: &str) -> Option<SarvamWsEvent> {
    let raw: serde_json::Value = serde_json::from_str(text).ok()?;
    let kind = raw.get("type")?.as_str()?;
    if kind == "data" {
        let transcript = raw
            .get("data")?
            .get("transcript")?
            .as_str()?
            .trim()
            .to_string();
        if transcript.is_empty() {
            return None;
        }
        return Some(SarvamWsEvent {
            transcript,
            is_final: true,
        });
    }
    None
}
