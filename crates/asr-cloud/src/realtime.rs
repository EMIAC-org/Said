//! Together's realtime WebSocket transcription transport.
//!
//! This client is intentionally Nemotron-only. The model accepts raw 16 kHz
//! mono signed-16-bit PCM frames, not a WAV container, and emits a final
//! transcript only after `input_audio_buffer.commit` when VAD is disabled.

use std::{io::Cursor, time::Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::{
    sync::mpsc,
    time::{Duration, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::header},
};

use crate::{CloudSttError, CloudTranscription, together};

const PCM_CHUNK_BYTES: usize = 3_200; // 100 ms of mono PCM s16le at 16 kHz.
const LIVE_COMMAND_BUFFER: usize = 2_048;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(75);

type RealtimeSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Live transcript updates are replacements, not token fragments. The desktop
/// must replace its prior HUD preview whenever it receives a `Delta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveTranscriptEvent {
    Ready,
    Delta { transcript: String },
}

/// Producer held by the recorder bridge for one live transcription session.
/// It is intentionally separate from the receiver owned by the WebSocket task:
/// audio capture never knows about Together's socket lifecycle.
#[derive(Debug, Clone)]
pub struct LiveTranscriptionController {
    command_tx: mpsc::Sender<LiveCommand>,
}

/// Consumer moved into `TogetherRealtimeClient::transcribe_live`.
pub struct LiveTranscriptionInput {
    command_rx: mpsc::Receiver<LiveCommand>,
}

enum LiveCommand {
    Append(Vec<u8>),
    Commit,
}

/// Create a bounded one-recording audio bridge. At 16 kHz this holds roughly
/// twenty seconds of source callbacks while the initial TLS/WebSocket handshake
/// completes, without allowing an unbounded background queue.
pub fn live_transcription_input() -> (LiveTranscriptionController, LiveTranscriptionInput) {
    let (command_tx, command_rx) = mpsc::channel(LIVE_COMMAND_BUFFER);
    (
        LiveTranscriptionController { command_tx },
        LiveTranscriptionInput { command_rx },
    )
}

impl LiveTranscriptionController {
    /// Called only by AirNote's recorder-drain thread, never by an async task.
    /// Blocking here applies back-pressure instead of silently dropping spoken
    /// audio if a network connection is temporarily slow to establish.
    pub fn append_pcm16le_blocking(&self, pcm: Vec<u8>) -> Result<(), CloudSttError> {
        if pcm.is_empty() {
            return Ok(());
        }
        if pcm.len() % 2 != 0 {
            return Err(CloudSttError::InvalidResponse {
                detail: "live recorder supplied an odd number of PCM bytes".into(),
            });
        }
        self.command_tx
            .blocking_send(LiveCommand::Append(pcm))
            .map_err(|_| CloudSttError::InvalidResponse {
                detail: "live transcription session ended before audio capture finished".into(),
            })
    }

    /// Signals the authoritative Caps/Option release. Together VAD is disabled,
    /// so this explicit commit is the only way a final transcript is produced.
    pub async fn commit(&self) -> Result<(), CloudSttError> {
        self.command_tx
            .send(LiveCommand::Commit)
            .await
            .map_err(|_| CloudSttError::InvalidResponse {
                detail: "live transcription session ended before recording was committed".into(),
            })
    }
}

/// A stateless Together realtime client. A WebSocket is one utterance: this
/// avoids state leaking across Caps Lock holds and makes a failed run easy to
/// classify without replaying audio into an uncertain session.
#[derive(Debug, Clone)]
pub struct TogetherRealtimeClient {
    api_key: String,
}

impl TogetherRealtimeClient {
    pub fn nemotron(api_key: String) -> Result<Self, CloudSttError> {
        if api_key.trim().is_empty() {
            return Err(CloudSttError::MissingApiKey {
                provider: "Together AI realtime transcription".into(),
                env_var: together::API_KEY_ENV.into(),
            });
        }
        Ok(Self { api_key })
    }

    pub fn model(&self) -> &'static str {
        together::NEMOTRON_3_5_ASR_STREAMING_0_6B
    }

    /// Keep a Together realtime session open while a recording is active.
    ///
    /// `input` receives raw mono PCM as it is captured. `event_tx` receives
    /// replacement-style interim hypotheses for a local HUD only; callers must
    /// never type these hypotheses into the focused application. `Commit` is
    /// sent only on the user's key release and yields the authoritative final
    /// transcript returned from this future.
    pub async fn transcribe_live(
        &self,
        mut input: LiveTranscriptionInput,
        event_tx: mpsc::UnboundedSender<LiveTranscriptEvent>,
    ) -> Result<CloudTranscription, CloudSttError> {
        let started = Instant::now();
        let (transcript, audio_secs) = timeout(
            REQUEST_TIMEOUT,
            self.transcribe_live_pcm(&mut input, &event_tx),
        )
        .await
        .map_err(|_| CloudSttError::Timeout {
            budget_secs: REQUEST_TIMEOUT.as_secs(),
        })??;

        Ok(CloudTranscription {
            text: transcript.trim().to_string(),
            language: Some(together::NEMOTRON_LANGUAGE.to_string()),
            audio_secs: Some(audio_secs),
            latency_ms: started.elapsed().as_millis() as u64,
            model: format!("{}/realtime", self.model()),
        })
    }

    /// Replay an already-saved WAV through Together's realtime protocol.
    ///
    /// This exists for retrying historical recordings, which obviously cannot
    /// be streamed at capture time. New live dictations use `transcribe_live`;
    /// do not route normal key-down/key-up recording through this method.
    pub async fn transcribe_wav(&self, wav: &[u8]) -> Result<CloudTranscription, CloudSttError> {
        let pcm = wav_to_pcm16le(wav)?;
        let audio_secs = pcm.len() as f32 / 32_000.0;
        let started = Instant::now();

        let transcript = timeout(REQUEST_TIMEOUT, self.transcribe_pcm16le(&pcm))
            .await
            .map_err(|_| CloudSttError::Timeout {
                budget_secs: REQUEST_TIMEOUT.as_secs(),
            })??;

        Ok(CloudTranscription {
            text: transcript.trim().to_string(),
            language: Some(together::NEMOTRON_LANGUAGE.to_string()),
            audio_secs: Some(audio_secs),
            latency_ms: started.elapsed().as_millis() as u64,
            model: format!("{}/realtime", self.model()),
        })
    }

    async fn transcribe_pcm16le(&self, pcm: &[u8]) -> Result<String, CloudSttError> {
        if pcm.is_empty() {
            return Err(CloudSttError::Rejected {
                status: 422,
                detail: "the recording contained no PCM audio".into(),
            });
        }

        let mut socket = self.open_socket().await?;

        for frame in pcm.chunks(PCM_CHUNK_BYTES) {
            let event = serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": STANDARD.encode(frame),
            });
            socket
                .send(Message::Text(event.to_string()))
                .await
                .map_err(websocket_error)?;
        }
        socket
            .send(Message::Text(
                serde_json::json!({ "type": "input_audio_buffer.commit" }).to_string(),
            ))
            .await
            .map_err(websocket_error)?;

        loop {
            let Some(message) = socket.next().await else {
                return Err(CloudSttError::InvalidResponse {
                    detail: "Together realtime connection closed before a final transcript".into(),
                });
            };
            if let Some(transcript) = handle_socket_message(&mut socket, message, None).await? {
                return Ok(transcript);
            }
        }
    }

    async fn open_socket(&self) -> Result<RealtimeSocket, CloudSttError> {
        let mut request = realtime_url().into_client_request().map_err(|error| {
            CloudSttError::InvalidResponse {
                detail: format!("invalid realtime request: {error}"),
            }
        })?;
        let auth = format!("Bearer {}", self.api_key);
        request.headers_mut().insert(
            header::AUTHORIZATION,
            auth.parse()
                .map_err(|error| CloudSttError::InvalidResponse {
                    detail: format!("invalid Together authorization header: {error}"),
                })?,
        );
        request.headers_mut().insert(
            "OpenAI-Beta",
            "realtime=v1".parse().expect("static header value is valid"),
        );

        let (mut socket, _) = timeout(CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| CloudSttError::Timeout {
                budget_secs: CONNECT_TIMEOUT.as_secs(),
            })?
            .map_err(websocket_error)?;
        wait_for_session(&mut socket).await?;
        Ok(socket)
    }

    async fn transcribe_live_pcm(
        &self,
        input: &mut LiveTranscriptionInput,
        event_tx: &mpsc::UnboundedSender<LiveTranscriptEvent>,
    ) -> Result<(String, f32), CloudSttError> {
        let mut socket = self.open_socket().await?;
        let _ = event_tx.send(LiveTranscriptEvent::Ready);
        let mut pending_pcm = Vec::with_capacity(PCM_CHUNK_BYTES * 2);
        let mut audio_bytes = 0usize;

        loop {
            tokio::select! {
                command = input.command_rx.recv() => match command {
                    Some(LiveCommand::Append(pcm)) => {
                        audio_bytes += pcm.len();
                        pending_pcm.extend_from_slice(&pcm);
                        send_full_pcm_frames(&mut socket, &mut pending_pcm).await?;
                    }
                    Some(LiveCommand::Commit) => break,
                    None => {
                        return Err(CloudSttError::InvalidResponse {
                            detail: "live transcription was cancelled before recording was committed".into(),
                        });
                    }
                },
                message = socket.next() => {
                    let Some(message) = message else {
                        return Err(connection_closed_before_final());
                    };
                    if let Some(transcript) = handle_socket_message(&mut socket, message, Some(event_tx)).await? {
                        return Ok((transcript, audio_bytes as f32 / 32_000.0));
                    }
                },
            }
        }

        if audio_bytes == 0 {
            return Err(CloudSttError::Rejected {
                status: 422,
                detail: "the recording contained no PCM audio".into(),
            });
        }
        if !pending_pcm.is_empty() {
            send_pcm_frame(&mut socket, &pending_pcm).await?;
        }
        socket
            .send(Message::Text(
                serde_json::json!({ "type": "input_audio_buffer.commit" }).to_string(),
            ))
            .await
            .map_err(websocket_error)?;

        loop {
            let Some(message) = socket.next().await else {
                return Err(connection_closed_before_final());
            };
            if let Some(transcript) =
                handle_socket_message(&mut socket, message, Some(event_tx)).await?
            {
                return Ok((transcript, audio_bytes as f32 / 32_000.0));
            }
        }
    }
}

fn realtime_url() -> String {
    format!(
        "{}?intent=transcription&model={}&input_audio_format=pcm_s16le_16000&language={}&turn_detection=none",
        together::REALTIME_ENDPOINT,
        together::NEMOTRON_3_5_ASR_STREAMING_0_6B,
        together::NEMOTRON_LANGUAGE,
    )
}

async fn wait_for_session<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<(), CloudSttError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let Some(message) = socket.next().await else {
            return Err(CloudSttError::InvalidResponse {
                detail: "Together realtime connection closed before session creation".into(),
            });
        };
        match message.map_err(websocket_error)? {
            Message::Text(text) => match parse_event(&text)? {
                RealtimeEvent::SessionCreated => return Ok(()),
                RealtimeEvent::Failed { message } | RealtimeEvent::Error { message } => {
                    return Err(CloudSttError::Rejected {
                        status: 422,
                        detail: message,
                    });
                }
                RealtimeEvent::Delta { .. }
                | RealtimeEvent::Completed { .. }
                | RealtimeEvent::Other => {}
            },
            Message::Ping(payload) => socket
                .send(Message::Pong(payload))
                .await
                .map_err(websocket_error)?,
            Message::Close(_) => {
                return Err(CloudSttError::InvalidResponse {
                    detail: "Together realtime connection closed before session creation".into(),
                });
            }
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
}

async fn send_full_pcm_frames(
    socket: &mut RealtimeSocket,
    pending_pcm: &mut Vec<u8>,
) -> Result<(), CloudSttError> {
    while pending_pcm.len() >= PCM_CHUNK_BYTES {
        let frame = pending_pcm[..PCM_CHUNK_BYTES].to_vec();
        pending_pcm.drain(..PCM_CHUNK_BYTES);
        send_pcm_frame(socket, &frame).await?;
    }
    Ok(())
}

async fn send_pcm_frame(socket: &mut RealtimeSocket, frame: &[u8]) -> Result<(), CloudSttError> {
    let event = serde_json::json!({
        "type": "input_audio_buffer.append",
        "audio": STANDARD.encode(frame),
    });
    socket
        .send(Message::Text(event.to_string()))
        .await
        .map_err(websocket_error)
}

async fn handle_socket_message(
    socket: &mut RealtimeSocket,
    message: Result<Message, tokio_tungstenite::tungstenite::Error>,
    event_tx: Option<&mpsc::UnboundedSender<LiveTranscriptEvent>>,
) -> Result<Option<String>, CloudSttError> {
    match message.map_err(websocket_error)? {
        Message::Text(text) => match parse_event(&text)? {
            RealtimeEvent::Completed { transcript } => Ok(Some(transcript)),
            RealtimeEvent::Delta { transcript } => {
                if !transcript.trim().is_empty() {
                    if let Some(event_tx) = event_tx {
                        let _ = event_tx.send(LiveTranscriptEvent::Delta { transcript });
                    }
                }
                Ok(None)
            }
            RealtimeEvent::Failed { message } => Err(CloudSttError::Rejected {
                status: 422,
                detail: message,
            }),
            RealtimeEvent::Error { message } => Err(CloudSttError::InvalidResponse {
                detail: format!("Together realtime service error: {message}"),
            }),
            RealtimeEvent::SessionCreated | RealtimeEvent::Other => Ok(None),
        },
        Message::Ping(payload) => {
            socket
                .send(Message::Pong(payload))
                .await
                .map_err(websocket_error)?;
            Ok(None)
        }
        Message::Close(_) => Err(connection_closed_before_final()),
        Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
    }
}

fn connection_closed_before_final() -> CloudSttError {
    CloudSttError::InvalidResponse {
        detail: "Together realtime connection closed before a final transcript".into(),
    }
}

#[derive(Debug, Deserialize)]
struct WireEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    transcript: String,
    #[serde(default)]
    delta: String,
    #[serde(default)]
    error: Option<WireError>,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
}

enum RealtimeEvent {
    SessionCreated,
    Delta { transcript: String },
    Completed { transcript: String },
    Failed { message: String },
    Error { message: String },
    Other,
}

fn parse_event(text: &str) -> Result<RealtimeEvent, CloudSttError> {
    let event: WireEvent =
        serde_json::from_str(text).map_err(|error| CloudSttError::InvalidResponse {
            detail: format!("invalid Together realtime event: {error}"),
        })?;
    let message = event
        .error
        .map(|error| error.message)
        .filter(|message| !message.is_empty())
        .unwrap_or(event.message);
    Ok(match event.kind.as_str() {
        "session.created" => RealtimeEvent::SessionCreated,
        "conversation.item.input_audio_transcription.delta" => RealtimeEvent::Delta {
            transcript: event.delta,
        },
        "conversation.item.input_audio_transcription.completed" => RealtimeEvent::Completed {
            transcript: event.transcript,
        },
        "conversation.item.input_audio_transcription.failed" => RealtimeEvent::Failed { message },
        "error" => RealtimeEvent::Error { message },
        _ => RealtimeEvent::Other,
    })
}

fn websocket_error(error: tokio_tungstenite::tungstenite::Error) -> CloudSttError {
    use tokio_tungstenite::tungstenite::Error;

    match error {
        Error::Http(response) => crate::classify_http_status(response.status().as_u16(), ""),
        Error::Io(_) => CloudSttError::Offline,
        other => CloudSttError::InvalidResponse {
            detail: format!("Together realtime WebSocket error: {other}"),
        },
    }
}

/// Decode only the WAV shape AirNote records: mono, 16 kHz, signed 16-bit PCM.
/// The websocket protocol expects raw PCM and must never receive a RIFF header.
fn wav_to_pcm16le(wav: &[u8]) -> Result<Vec<u8>, CloudSttError> {
    let mut reader =
        hound::WavReader::new(Cursor::new(wav)).map_err(|error| CloudSttError::Rejected {
            status: 422,
            detail: format!("expected a WAV recording: {error}"),
        })?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != 16_000
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(CloudSttError::Rejected {
            status: 422,
            detail: format!(
                "realtime Nemotron needs mono 16 kHz signed-16-bit WAV (received {} channel(s), {} Hz, {}-bit)",
                spec.channels, spec.sample_rate, spec.bits_per_sample
            ),
        });
    }

    let mut pcm = Vec::with_capacity(reader.duration() as usize * 2);
    for sample in reader.samples::<i16>() {
        let sample = sample.map_err(|error| CloudSttError::Rejected {
            status: 422,
            detail: format!("invalid WAV sample: {error}"),
        })?;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16, sample_rate: u32, bits_per_sample: u16) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format: hound::SampleFormat::Int,
        };
        let mut bytes = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
        writer.write_sample(123i16).unwrap();
        writer.write_sample(-456i16).unwrap();
        writer.finalize().unwrap();
        bytes.into_inner()
    }

    #[test]
    fn realtime_url_pins_nemotron_hindi_and_manual_commit_mode() {
        let url = realtime_url();
        assert!(url.contains(together::NEMOTRON_3_5_ASR_STREAMING_0_6B));
        assert!(url.contains("input_audio_format=pcm_s16le_16000"));
        assert!(url.contains("language=hi"));
        assert!(url.contains("turn_detection=none"));
    }

    #[test]
    fn wav_conversion_strips_riff_and_preserves_little_endian_pcm() {
        let pcm = wav_to_pcm16le(&wav(1, 16_000, 16)).unwrap();
        assert_eq!(
            pcm,
            [123i16.to_le_bytes(), (-456i16).to_le_bytes()].concat()
        );
        assert!(!pcm.starts_with(b"RIFF"));
    }

    #[test]
    fn wav_conversion_rejects_an_unsupported_audio_shape() {
        let error = wav_to_pcm16le(&wav(2, 16_000, 16)).unwrap_err();
        assert!(error.to_string().contains("mono 16 kHz"));
    }

    #[test]
    fn parser_distinguishes_final_transcripts_from_interim_deltas() {
        assert!(matches!(
            parse_event(
                r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"interim"}"#
            )
            .unwrap(),
            RealtimeEvent::Delta { transcript } if transcript == "interim"
        ));
        assert!(matches!(
            parse_event(r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"final text"}"#).unwrap(),
            RealtimeEvent::Completed { transcript } if transcript == "final text"
        ));
    }

    #[tokio::test]
    async fn live_audio_bridge_keeps_audio_before_the_release_commit() {
        let (controller, mut input) = live_transcription_input();
        let audio_writer = controller.clone();
        std::thread::spawn(move || {
            audio_writer
                .append_pcm16le_blocking(vec![1, 0, 2, 0])
                .unwrap();
        })
        .join()
        .unwrap();

        assert!(matches!(
            input.command_rx.recv().await,
            Some(LiveCommand::Append(pcm)) if pcm == vec![1, 0, 2, 0]
        ));
        controller.commit().await.unwrap();
        assert!(matches!(
            input.command_rx.recv().await,
            Some(LiveCommand::Commit)
        ));
    }
}
