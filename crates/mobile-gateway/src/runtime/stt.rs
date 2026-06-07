//! Deepgram speech-to-text — streaming (WebSocket) for live dictation and batch
//! (prerecorded REST) for the fallback path. Mirrors the desktop/control-plane
//! Deepgram usage (nova-3, 16 kHz linear16, multi-language for Hinglish).

use std::time::Duration;

use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::client::IntoClientRequest,
};

pub type DgStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// Map an AirNote language hint to a Deepgram `language` parameter. Hinglish and
/// auto use nova-3 `multi` (code-switching); explicit en/hi pin the language.
pub fn dg_language(hint: &str) -> &'static str {
    match hint {
        "en" => "en",
        "hi" => "hi",
        _ => "multi",
    }
}

/// Open a streaming Deepgram WebSocket configured for 16 kHz PCM16 mono.
pub async fn connect_stream(api_key: &str, language_hint: &str) -> Result<DgStream, BoxErr> {
    let language = dg_language(language_hint);
    let url = format!(
        "wss://api.deepgram.com/v1/listen?model=nova-3&language={language}&smart_format=true\
         &encoding=linear16&sample_rate=16000&channels=1&interim_results=true\
         &endpointing=300&utterance_end_ms=1000"
    );
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Token {api_key}").parse()?);
    let (socket, _) = connect_async(request).await?;
    Ok(socket)
}

/// Parse a Deepgram streaming JSON message into `(transcript, is_final)` when it
/// carries a non-empty transcript; otherwise `None` (interim noise, metadata,
/// utterance-end markers, etc).
pub fn extract_transcript(text: &str) -> Option<(String, bool)> {
    let raw: serde_json::Value = serde_json::from_str(text).ok()?;
    let is_final = raw
        .get("is_final")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let transcript = raw
        .get("channel")?
        .get("alternatives")?
        .as_array()?
        .first()?
        .get("transcript")?
        .as_str()?
        .trim()
        .to_string();
    if transcript.is_empty() {
        None
    } else {
        Some((transcript, is_final))
    }
}

/// Batch (prerecorded) transcription for the fallback path. Sends the raw audio
/// bytes with the client's content type; for raw PCM16 it adds encoding hints.
pub async fn transcribe_batch(
    http: &reqwest::Client,
    api_key: &str,
    audio: Vec<u8>,
    content_type: &str,
    language_hint: &str,
) -> Result<String, String> {
    if api_key.trim().is_empty() {
        return Err("Deepgram API key not configured".into());
    }
    let language = dg_language(language_hint);
    let mut url =
        format!("https://api.deepgram.com/v1/listen?model=nova-3&smart_format=true&language={language}");
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("pcm") || ct.contains("l16") || ct.contains("raw") || ct.is_empty() {
        url.push_str("&encoding=linear16&sample_rate=16000&channels=1");
    }
    let send_ct = if content_type.trim().is_empty() {
        "application/octet-stream"
    } else {
        content_type
    };

    let resp = http
        .post(&url)
        .header("Authorization", format!("Token {api_key}"))
        .header("Content-Type", send_ct)
        .body(audio)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("deepgram request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "deepgram error {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("deepgram json parse: {e}"))?;
    let transcript = v
        .get("results")
        .and_then(|r| r.get("channels"))
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("alternatives"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|a| a.get("transcript"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_mapping() {
        assert_eq!(dg_language("hinglish"), "multi");
        assert_eq!(dg_language("auto"), "multi");
        assert_eq!(dg_language("en"), "en");
        assert_eq!(dg_language("hi"), "hi");
    }

    #[test]
    fn extracts_final_transcript() {
        let msg = r#"{"is_final":true,"channel":{"alternatives":[{"transcript":"hello world"}]}}"#;
        assert_eq!(extract_transcript(msg), Some(("hello world".to_string(), true)));
    }

    #[test]
    fn ignores_empty_transcript() {
        let msg = r#"{"is_final":false,"channel":{"alternatives":[{"transcript":""}]}}"#;
        assert_eq!(extract_transcript(msg), None);
    }
}
