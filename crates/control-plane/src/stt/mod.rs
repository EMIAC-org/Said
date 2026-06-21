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
    call_deepgram_batch(api_key, wav_data, tag).await
}

async fn call_deepgram_batch(
    api_key: &str,
    wav_data: Vec<u8>,
    tag: &str,
) -> Result<String, String> {
    let bias = said_core::deepgram::BiasPackage {
        stt_mode: "hi".to_string(),
        ..Default::default()
    };
    let url = said_core::deepgram::build_batch_url("https://api.deepgram.com/v1/listen", &bias);
    let client = &*crate::HTTP_CLIENT;
    let resp = client
        .post(url)
        .header("Authorization", format!("Token {api_key}"))
        .header("Content-Type", "audio/wav")
        .body(wav_data)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("{tag}: Deepgram request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "{tag}: Deepgram returned {status}: {}",
            said_core::text::truncate_utf8(&body, 300)
        ));
    }

    let raw = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("{tag}: failed to parse Deepgram response: {e}"))?;
    let transcript = raw
        .get("results")
        .and_then(|v| v.get("channels"))
        .and_then(serde_json::Value::as_array)
        .and_then(|channels| channels.first())
        .and_then(|channel| channel.get("alternatives"))
        .and_then(serde_json::Value::as_array)
        .and_then(|alternatives| alternatives.first())
        .and_then(|alternative| alternative.get("transcript"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if transcript.is_empty() {
        Err(format!("{tag}: Deepgram returned empty transcript"))
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
    let url = said_core::deepgram::build_ws_url(
        "wss://api.deepgram.com/v1/listen",
        &bias,
        sample_rate,
    );
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
