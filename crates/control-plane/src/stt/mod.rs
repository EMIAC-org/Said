use said_core::stt::resolve_server_default_provider;

pub use said_core::stt::SttProvider;

pub fn runtime_stt_provider() -> String {
    resolve_server_default_provider()
}

pub fn runtime_stt_credential_provider(_provider: &str) -> &'static str {
    "deepgram"
}

pub async fn call_batch_stt(
    _provider: &str,
    api_key: &str,
    wav_data: Vec<u8>,
    tag: &str,
) -> Result<String, String> {
    call_deepinfra_batch(api_key, wav_data, tag).await
}

async fn call_deepinfra_batch(
    api_key: &str,
    wav_data: Vec<u8>,
    tag: &str,
) -> Result<String, String> {
    let client = &*crate::HTTP_CLIENT;
    let audio = reqwest::multipart::Part::bytes(wav_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("{tag}: failed to build DeepInfra audio part: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("audio", audio)
        .text("language", "hi")
        // Force transcription rather than English translation/normalization.
        .text("task", "transcribe");
    let resp = client
        .post("https://api.deepinfra.com/v1/inference/openai/whisper-large-v3")
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .multipart(form)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("{tag}: DeepInfra request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "{tag}: DeepInfra returned {status}: {}",
            said_core::text::truncate_utf8(&body, 300)
        ));
    }

    let raw = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("{tag}: failed to parse DeepInfra response: {e}"))?;
    let direct = raw
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let transcript = if direct.is_empty() {
        raw.get("segments")
            .and_then(serde_json::Value::as_array)
            .map(|segments| {
                segments
                    .iter()
                    .filter_map(|segment| segment.get("text").and_then(serde_json::Value::as_str))
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    } else {
        direct
    };

    if transcript.is_empty() {
        Err(format!("{tag}: DeepInfra returned empty transcript"))
    } else {
        Ok(transcript)
    }
}

pub async fn connect_runtime_ws(
    _provider: &str,
    api_key: &str,
    sample_rate: u32,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    connect_deepgram_ws(api_key, sample_rate).await
}

async fn connect_deepgram_ws(
    api_key: &str,
    sample_rate: u32,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
    let bias = said_core::deepgram::BiasPackage {
        stt_mode: "hi".to_string(),
        ..Default::default()
    };
    let url =
        said_core::deepgram::build_ws_url("wss://api.deepgram.com/v1/listen", &bias, sample_rate);
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Token {api_key}").parse()?);
    let (socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(6), connect_async(request))
            .await
            .map_err(|_| "Deepgram runtime websocket connect timed out")??;
    Ok(socket)
}
