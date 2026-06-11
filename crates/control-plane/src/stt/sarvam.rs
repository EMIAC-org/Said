//! Sarvam Saaras v3 — batch REST + streaming WS for server runtime.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

const SARVAM_REST_URL: &str = "https://api.sarvam.ai/speech-to-text";
const CHUNK_SECONDS: f64 = 29.0;

#[derive(Deserialize)]
struct SarvamRestResponse {
    transcript: Option<String>,
}

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

pub fn ws_connect_url(sample_rate: u32) -> String {
    format!(
        "wss://api.sarvam.ai/speech-to-text/ws?model=saaras:v3&mode=codemix&language-code=hi-IN&sample_rate={sample_rate}&high_vad_sensitivity=true&flush_signal=true&vad_signals=true"
    )
}

pub async fn connect_ws(
    api_key: &str,
    sample_rate: u32,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let url = ws_connect_url(sample_rate);
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Api-Subscription-Key",
        api_key
            .parse()
            .map_err(|e| format!("invalid sarvam api key header: {e}"))?,
    );
    let (socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(6), connect_async(request))
            .await
            .map_err(|_| "Sarvam runtime websocket connect timed out")??;
    Ok(socket)
}

#[derive(Debug)]
pub struct SarvamWsEvent {
    pub transcript: String,
}

pub fn parse_ws_message(text: &str) -> Option<SarvamWsEvent> {
    let raw: serde_json::Value = serde_json::from_str(text).ok()?;
    if raw.get("type")?.as_str()? != "data" {
        return None;
    }
    let transcript = raw
        .get("data")?
        .get("transcript")?
        .as_str()?
        .trim()
        .to_string();
    if transcript.is_empty() {
        return None;
    }
    Some(SarvamWsEvent { transcript })
}

pub async fn transcribe_batch(
    api_key: &str,
    wav_data: Vec<u8>,
    tag: &str,
) -> Result<String, String> {
    let chunks = wav_pcm_chunks(&wav_data)?;
    let client = reqwest::Client::new();
    let mut parts = Vec::new();
    for chunk in chunks {
        let text = transcribe_chunk(&client, api_key, chunk).await?;
        if !text.is_empty() {
            parts.push(text);
        }
    }
    let transcript = parts.join(" ").trim().to_string();
    if transcript.is_empty() {
        return Err(format!("{tag}: Sarvam returned empty transcript"));
    }
    Ok(transcript)
}

async fn transcribe_chunk(
    client: &reqwest::Client,
    api_key: &str,
    wav_chunk: Vec<u8>,
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(wav_chunk)
        .file_name("chunk.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new()
        .text("model", "saaras:v3")
        .text("mode", "codemix")
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

fn pcm16_to_mini_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
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
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("invalid WAV".into());
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
            chunks.push(pcm16_to_mini_wav(slice, sample_rate));
        }
        offset = end;
    }
    Ok(chunks)
}
