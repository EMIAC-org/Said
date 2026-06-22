//! Local Swift STT live streaming session (mirrors `dg_stream` actor contract).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use crate::echo_gate::EchoGateShared;
use crate::server_runtime_stream::AudioMirrorCommand;
use crate::swift_stt_engine;

use futures::{SinkExt, StreamExt};
use said_core::deepgram::TranscriptMeta;
use said_recorder::{ChunkReceiver, resample_to_16k};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

pub use crate::dg_stream::StreamingTranscript;

const AUDIO_BRIDGE_BUFFER_CHUNKS: usize = 64;
const COMMAND_BUFFER_CHUNKS: usize = AUDIO_BRIDGE_BUFFER_CHUNKS + 16;
const WS_SEND_TIMEOUT: Duration = Duration::from_secs(2);
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STOP_DRAIN_QUIET: Duration = Duration::from_millis(350);
const STOP_DRAIN_MAX: Duration = Duration::from_millis(1_500);

#[derive(Clone)]
pub struct AudioBridgeStopHandle {
    recording_id: String,
    stop_requested: Arc<AtomicBool>,
}

impl AudioBridgeStopHandle {
    fn new(recording_id: String) -> Self {
        Self {
            recording_id,
            stop_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn recording_id(&self) -> &str {
        &self.recording_id
    }

    pub fn request_stop(&self, reason: &str) {
        let was_set = self.stop_requested.swap(true, Ordering::AcqRel);
        info!(
            "[swift_session] audio bridge stop requested id={} reason={} already_requested={}",
            self.recording_id, reason, was_set
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    Connecting,
    Idle,
    Streaming,
    Finalizing,
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connecting => write!(f, "connecting"),
            Self::Idle => write!(f, "idle"),
            Self::Streaming => write!(f, "streaming"),
            Self::Finalizing => write!(f, "finalizing"),
        }
    }
}

pub enum SessionCommand {
    StartRecording {
        id: String,
        result_tx: oneshot::Sender<Option<StreamingTranscript>>,
        pre_embed: Option<(String, String)>,
        utterance_end_tx: Option<tokio_mpsc::UnboundedSender<String>>,
        /// Emits rolling partial text to the status bar while Caps Lock is held.
        live_partial_tx: Option<tokio_mpsc::UnboundedSender<String>>,
        /// Personal vocabulary (starred keyterms) sent to the sidecar on connect
        /// to bias the on-device Whisper model toward proper nouns it would
        /// otherwise mangle. Empty = no biasing (unchanged behavior).
        vocab: Vec<String>,
    },
    Audio {
        id: String,
        pcm: Vec<u8>,
    },
    Finalize {
        id: String,
    },
    Shutdown,
    GetState(oneshot::Sender<SessionState>),
}

pub type SessionSender = tokio_mpsc::Sender<SessionCommand>;

struct ActiveRecording {
    id: String,
    result_tx: Option<oneshot::Sender<Option<StreamingTranscript>>>,
    live_partial_tx: Option<tokio_mpsc::UnboundedSender<String>>,
    latest_text: String,
    word_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishSource {
    SwiftFinal,
    SwiftLivePartialFallback,
    SwiftNone,
}

impl FinishSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::SwiftFinal => "swift_final",
            Self::SwiftLivePartialFallback => "swift_live_partial_fallback",
            Self::SwiftNone => "swift_none",
        }
    }
}

enum FinishDecision {
    Final(FinalTranscript),
    LatestPartial,
    NoTranscript,
}

struct FinishOutcome {
    source: FinishSource,
    sidecar_source: Option<String>,
    transcript_chars: usize,
    word_count: usize,
}

struct FinalTranscript {
    text: String,
    source: Option<String>,
}

impl ActiveRecording {
    fn new(
        id: String,
        result_tx: oneshot::Sender<Option<StreamingTranscript>>,
        live_partial_tx: Option<tokio_mpsc::UnboundedSender<String>>,
    ) -> Self {
        Self {
            id,
            result_tx: Some(result_tx),
            live_partial_tx,
            latest_text: String::new(),
            word_count: 0,
        }
    }

    fn has_latest_partial(&self) -> bool {
        !self.latest_text.trim().is_empty()
    }

    fn accept_text(&mut self, text: &str, emit_live: bool) -> bool {
        if let Some(text) = clean_swift_transcript(text) {
            self.latest_text = text;
            self.word_count = self.latest_text.split_whitespace().count();
            if emit_live {
                if let Some(tx) = &self.live_partial_tx {
                    let _ = tx.send(self.latest_text.clone());
                }
            }
            true
        } else {
            false
        }
    }

    fn note_partial(&mut self, text: &str) -> bool {
        self.accept_text(text, true)
    }

    fn finish(mut self, decision: FinishDecision) -> FinishOutcome {
        let mut sidecar_source = None;
        let source = match decision {
            FinishDecision::Final(final_transcript) => {
                sidecar_source = final_transcript.source;
                if self.accept_text(&final_transcript.text, false) {
                    FinishSource::SwiftFinal
                } else if self.has_latest_partial() {
                    FinishSource::SwiftLivePartialFallback
                } else {
                    self.latest_text.clear();
                    self.word_count = 0;
                    FinishSource::SwiftNone
                }
            }
            FinishDecision::LatestPartial if self.has_latest_partial() => {
                FinishSource::SwiftLivePartialFallback
            }
            FinishDecision::LatestPartial | FinishDecision::NoTranscript => {
                self.latest_text.clear();
                self.word_count = 0;
                FinishSource::SwiftNone
            }
        };
        let transcript = self.latest_text.trim().to_string();
        let word_count = if self.word_count > 0 {
            self.word_count
        } else {
            transcript.split_whitespace().count()
        };
        let transcript_chars = transcript.len();
        let meta = TranscriptMeta {
            enriched_transcript: transcript.clone(),
            confidence: 1.0,
            mean_word_confidence: 1.0,
            low_confidence_count: 0,
            word_count,
            languages: vec!["hi".to_string()],
            stt_mode: "swift_local".to_string(),
        };
        let payload = if transcript.is_empty() {
            None
        } else {
            Some(StreamingTranscript { transcript, meta })
        };
        if let Some(tx) = self.result_tx.take() {
            let _ = tx.send(payload);
        }
        FinishOutcome {
            source,
            sidecar_source,
            transcript_chars,
            word_count,
        }
    }
}

type WsWrite = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

struct WsConnection {
    write: WsWrite,
    partial_rx: tokio_mpsc::UnboundedReceiver<TranscriptEvent>,
}

enum TranscriptEvent {
    Partial(String),
    Final(FinalTranscript),
}

impl TranscriptEvent {
    fn text(&self) -> &str {
        match self {
            Self::Partial(text) => text,
            Self::Final(final_transcript) => &final_transcript.text,
        }
    }
}

pub struct SwiftSession;

impl SwiftSession {
    pub fn spawn() -> SessionSender {
        let (tx, mut rx) = tokio_mpsc::channel(COMMAND_BUFFER_CHUNKS);
        tauri::async_runtime::spawn(async move {
            let mut state = SessionState::Disconnected;
            let mut active: Option<ActiveRecording> = None;
            let mut ws: Option<WsConnection> = None;

            let mut overflow: Option<SessionCommand> = None;

            loop {
                if let Some(conn) = ws.as_mut() {
                    tokio::select! {
                        cmd = recv_coalesced(&mut rx, &mut overflow) => {
                            let Some(cmd) = cmd else { break };
                            if !handle_session_command(
                                cmd,
                                &mut state,
                                &mut active,
                                &mut ws,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        event = conn.partial_rx.recv() => {
                            if let Some(event) = event {
                                if let Some(rec) = active.as_mut() {
                                    rec.note_partial(event.text());
                                }
                            }
                        }
                    }
                } else {
                    let Some(cmd) = recv_coalesced(&mut rx, &mut overflow).await else {
                        break;
                    };
                    if !handle_session_command(cmd, &mut state, &mut active, &mut ws).await {
                        break;
                    }
                }
            }
        });
        tx
    }
}

/// Returns `false` when the session actor should exit.
async fn handle_session_command(
    cmd: SessionCommand,
    state: &mut SessionState,
    active: &mut Option<ActiveRecording>,
    ws: &mut Option<WsConnection>,
) -> bool {
    match cmd {
        SessionCommand::StartRecording {
            id,
            result_tx,
            pre_embed: _,
            utterance_end_tx: _,
            live_partial_tx,
            vocab,
        } => match connect_ws(&vocab).await {
            Ok(conn) => {
                *ws = Some(conn);
                *state = SessionState::Streaming;
                *active = Some(ActiveRecording::new(id, result_tx, live_partial_tx));
                info!("[swift_session] recording started");
            }
            Err(e) => {
                warn!("[swift_session] connect failed: {e}");
                *state = SessionState::Disconnected;
                let _ = result_tx.send(None);
                *ws = None;
                *active = None;
            }
        },
        SessionCommand::Audio { id, pcm } => {
            if active.as_ref().is_some_and(|a| a.id != id) {
                return true;
            }
            if let Some(conn) = ws.as_mut() {
                if conn.write.send(Message::Binary(pcm)).await.is_err() {
                    warn!("[swift_session] audio send failed id={id}");
                }
            }
        }
        SessionCommand::Finalize { id } => {
            if !active.as_ref().is_some_and(|a| a.id == id) {
                return true;
            }
            *state = SessionState::Finalizing;
            let had_partial_before_finalize = active
                .as_ref()
                .is_some_and(ActiveRecording::has_latest_partial);
            if let Some(conn) = ws.as_mut() {
                let _ = conn
                    .write
                    .send(Message::Text(r#"{"type":"finalize"}"#.to_string().into()))
                    .await;
            }
            let wait_start = tokio::time::Instant::now();
            let final_transcript = {
                let mut active_slot = active.as_mut();
                wait_final_or_partial_drain(ws.as_mut(), &mut active_slot).await
            };
            let finalize_wait_ms = wait_start.elapsed().as_millis();
            let decision = if let Some(final_transcript) = final_transcript {
                FinishDecision::Final(final_transcript)
            } else if active
                .as_ref()
                .is_some_and(ActiveRecording::has_latest_partial)
            {
                FinishDecision::LatestPartial
            } else {
                FinishDecision::NoTranscript
            };
            if let Some(rec) = active.take() {
                let outcome = rec.finish(decision);
                info!(
                    "[swift_session] finalized id={id} source={} sidecar_source={} had_partial_before={} chars={} words={} wait_ms={finalize_wait_ms}",
                    outcome.source.as_str(),
                    outcome.sidecar_source.as_deref().unwrap_or("none"),
                    had_partial_before_finalize,
                    outcome.transcript_chars,
                    outcome.word_count
                );
            }
            if let Some(conn) = ws.as_mut() {
                let _ = conn.write.close().await;
            }
            *ws = None;
            *state = SessionState::Idle;
        }
        SessionCommand::Shutdown => {
            if let Some(rec) = active.take() {
                let outcome = if rec.has_latest_partial() {
                    rec.finish(FinishDecision::LatestPartial)
                } else {
                    rec.finish(FinishDecision::NoTranscript)
                };
                info!(
                    "[swift_session] shutdown finalized source={} chars={} words={}",
                    outcome.source.as_str(),
                    outcome.transcript_chars,
                    outcome.word_count
                );
            }
            if let Some(mut conn) = ws.take() {
                let _ = conn.write.close().await;
            }
            swift_stt_engine::shutdown();
            return false;
        }
        SessionCommand::GetState(reply) => {
            let _ = reply.send(*state);
        }
    }
    true
}

/// Merge back-to-back `Audio` commands so finalize is not delayed by per-chunk dispatch.
async fn recv_coalesced(
    rx: &mut tokio_mpsc::Receiver<SessionCommand>,
    overflow: &mut Option<SessionCommand>,
) -> Option<SessionCommand> {
    if let Some(cmd) = overflow.take() {
        return Some(cmd);
    }
    let first = rx.recv().await?;
    match first {
        SessionCommand::Audio { id, mut pcm } => {
            while let Ok(next) = rx.try_recv() {
                match next {
                    SessionCommand::Audio {
                        id: next_id,
                        pcm: next_pcm,
                    } if next_id == id => pcm.extend(next_pcm),
                    other => {
                        *overflow = Some(other);
                        break;
                    }
                }
            }
            Some(SessionCommand::Audio { id, pcm })
        }
        other => Some(other),
    }
}

async fn connect_ws(vocab: &[String]) -> Result<WsConnection, String> {
    let port = tokio::task::spawn_blocking(swift_stt_engine::ensure_running)
        .await
        .map_err(|e| format!("spawn failed: {e}"))??;
    let url = format!("ws://127.0.0.1:{port}/stream");
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(&url));
    let (stream, _) = connect
        .await
        .map_err(|_| "Swift WS connect timed out".to_string())?
        .map_err(|e| format!("Swift WS connect failed: {e}"))?;
    let (mut write, read) = stream.split();
    // Bias the on-device Whisper model toward the user's personal vocabulary so
    // proper nouns (Kubernetes, n8n, EMIAC, names) aren't mangled. Best-effort:
    // a failed config send must never abort the recording.
    if !vocab.is_empty() {
        let frame = serde_json::json!({ "type": "config", "vocab": vocab }).to_string();
        if let Err(e) = write.send(Message::Text(frame.into())).await {
            warn!("[swift_session] vocab config send failed: {e}");
        }
    }
    let (partial_tx, partial_rx) = tokio_mpsc::unbounded_channel();
    tauri::async_runtime::spawn(async move {
        let mut read = read;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(m) => {
                    if let Some(err) = parse_ws_error(&m) {
                        warn!("[swift_session] sidecar error: {err}");
                    } else if let Some(event) = parse_ws_event(&m) {
                        let _ = partial_tx.send(event);
                    }
                }
                Err(e) => {
                    debug!("[swift_session] reader ended: {e}");
                    break;
                }
            }
        }
    });
    Ok(WsConnection { write, partial_rx })
}

async fn drain_partial_rx(
    conn: Option<&mut WsConnection>,
    active: &mut Option<&mut ActiveRecording>,
) -> Option<FinalTranscript> {
    let Some(conn) = conn else { return None };
    while let Ok(event) = conn.partial_rx.try_recv() {
        match event {
            TranscriptEvent::Partial(text) => {
                if let Some(rec) = active.as_mut() {
                    rec.note_partial(&text);
                }
            }
            TranscriptEvent::Final(final_transcript) => return Some(final_transcript),
        }
    }
    None
}

async fn wait_final_or_partial_drain(
    conn: Option<&mut WsConnection>,
    active: &mut Option<&mut ActiveRecording>,
) -> Option<FinalTranscript> {
    let conn = conn?;
    let hard_deadline = tokio::time::Instant::now() + FINALIZE_TIMEOUT;
    loop {
        if let Some(final_text) = drain_partial_rx(Some(conn), active).await {
            return Some(final_text);
        }
        let now = tokio::time::Instant::now();
        let remaining = hard_deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, conn.partial_rx.recv()).await {
            Ok(Some(event)) => match event {
                TranscriptEvent::Partial(text) => {
                    if let Some(rec) = active.as_mut() {
                        rec.note_partial(&text);
                    }
                }
                TranscriptEvent::Final(final_transcript) => return Some(final_transcript),
            },
            Ok(None) | Err(_) => break,
        }
    }
    None
}

fn parse_ws_event(msg: &Message) -> Option<TranscriptEvent> {
    let text = match msg {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => String::from_utf8(b.clone()).ok()?,
        _ => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let kind = v.get("type")?.as_str()?;
    let text = v.get("text")?.as_str()?.to_string();
    match kind {
        "partial" => Some(TranscriptEvent::Partial(text)),
        "final" => Some(TranscriptEvent::Final(FinalTranscript {
            text,
            source: v.get("source").and_then(|s| s.as_str()).map(str::to_string),
        })),
        _ => None,
    }
}

fn parse_ws_error(msg: &Message) -> Option<String> {
    let text = match msg {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => String::from_utf8(b.clone()).ok()?,
        _ => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    if v.get("type")?.as_str()? == "error" {
        v.get("message")?.as_str().map(str::to_string)
    } else {
        None
    }
}

fn clean_swift_transcript(text: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' && chars.peek() == Some(&'|') {
            let mut previous = ch;
            for next in chars.by_ref() {
                if previous == '|' && next == '>' {
                    break;
                }
                previous = next;
            }
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if is_suspicious_swift_transcript(&cleaned) {
        warn!(
            "[swift_session] dropping suspicious transcript partial: {:?}",
            cleaned.chars().take(160).collect::<String>()
        );
        None
    } else {
        Some(cleaned)
    }
}

fn is_suspicious_swift_transcript(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() || !compact.chars().any(|c| c.is_alphanumeric()) {
        return true;
    }
    let punct = compact.chars().filter(|c| !c.is_alphanumeric()).count();
    if punct as f64 / compact.chars().count().max(1) as f64 > 0.65 {
        return true;
    }
    let tokens: Vec<String> = trimmed
        .split_whitespace()
        .map(|token| normalize_swift_token(token))
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.len() >= 8 {
        let mut counts = std::collections::HashMap::<&str, usize>::new();
        for token in &tokens {
            *counts.entry(token.as_str()).or_insert(0) += 1;
        }
        let top = counts.values().copied().max().unwrap_or(0);
        let unique_ratio = counts.len() as f64 / tokens.len() as f64;
        let top_ratio = top as f64 / tokens.len() as f64;
        let avg_len =
            tokens.iter().map(|t| t.chars().count()).sum::<usize>() as f64 / tokens.len() as f64;
        if top_ratio >= 0.55 && unique_ratio <= 0.30 {
            return true;
        }
        if avg_len <= 1.25 && tokens.len() >= 12 {
            return true;
        }
    }
    false
}

fn normalize_swift_token(token: &str) -> String {
    token
        .trim_matches(|c: char| !c.is_alphanumeric() && !('\u{0900}'..='\u{097F}').contains(&c))
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finish_with_final_uses_clean_final_text() {
        let (tx, rx) = oneshot::channel();
        let mut rec = ActiveRecording::new("test".to_string(), tx, None);
        assert!(rec.note_partial("older live partial"));

        let outcome = rec.finish(FinishDecision::Final(FinalTranscript {
            text: "fresh final text".to_string(),
            source: Some("final_decode".to_string()),
        }));

        let payload = rx.await.expect("result sent").expect("transcript");
        assert_eq!(outcome.source, FinishSource::SwiftFinal);
        assert_eq!(outcome.sidecar_source.as_deref(), Some("final_decode"));
        assert_eq!(payload.transcript, "fresh final text");
        assert_eq!(payload.meta.word_count, 3);
    }

    #[tokio::test]
    async fn finish_with_empty_final_falls_back_to_latest_active_partial() {
        let (tx, rx) = oneshot::channel();
        let mut rec = ActiveRecording::new("test".to_string(), tx, None);
        assert!(rec.note_partial("complete live partial"));

        let outcome = rec.finish(FinishDecision::Final(FinalTranscript {
            text: String::new(),
            source: Some("empty_final".to_string()),
        }));

        let payload = rx.await.expect("result sent").expect("transcript");
        assert_eq!(outcome.source, FinishSource::SwiftLivePartialFallback);
        assert_eq!(outcome.sidecar_source.as_deref(), Some("empty_final"));
        assert_eq!(payload.transcript, "complete live partial");
        assert_eq!(payload.meta.word_count, 3);
    }

    #[tokio::test]
    async fn finish_without_final_uses_latest_active_partial() {
        let (tx, rx) = oneshot::channel();
        let mut rec = ActiveRecording::new("test".to_string(), tx, None);
        assert!(rec.note_partial("latest live partial"));

        let outcome = rec.finish(FinishDecision::LatestPartial);

        let payload = rx.await.expect("result sent").expect("transcript");
        assert_eq!(outcome.source, FinishSource::SwiftLivePartialFallback);
        assert_eq!(payload.transcript, "latest live partial");
        assert_eq!(payload.meta.word_count, 3);
    }

    #[tokio::test]
    async fn finish_without_final_or_partial_returns_none() {
        let (tx, rx) = oneshot::channel();
        let rec = ActiveRecording::new("test".to_string(), tx, None);

        let outcome = rec.finish(FinishDecision::LatestPartial);

        assert_eq!(outcome.source, FinishSource::SwiftNone);
        assert!(rx.await.expect("result sent").is_none());
    }

    #[tokio::test]
    async fn finish_with_suspicious_partial_returns_none() {
        let (tx, rx) = oneshot::channel();
        let mut rec = ActiveRecording::new("test".to_string(), tx, None);
        assert!(!rec.note_partial(""));

        let outcome = rec.finish(FinishDecision::LatestPartial);

        assert_eq!(outcome.source, FinishSource::SwiftNone);
        assert!(rx.await.expect("result sent").is_none());
    }
}

pub fn spawn_audio_bridge_with_echo_gate(
    recording_id: String,
    chunk_recv: ChunkReceiver,
    session_tx: SessionSender,
    echo_gate: Option<Arc<EchoGateShared>>,
    mirror_tx: Option<tokio_mpsc::UnboundedSender<AudioMirrorCommand>>,
) -> AudioBridgeStopHandle {
    let stop_handle = AudioBridgeStopHandle::new(recording_id.clone());
    let stop_requested = Arc::clone(&stop_handle.stop_requested);
    std::thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx: mpsc::Receiver<Vec<f32>> = chunk_recv.rx;
        let bridge_started = std::time::Instant::now();
        let mut stop_seen_at: Option<std::time::Instant> = None;
        let mut drain_reason = "channel_closed";
        let mut chunks_sent = 0usize;
        let mut bytes_sent = 0usize;
        let mut chunks_dropped = 0usize;
        loop {
            let stop_requested_now = stop_requested.load(Ordering::Acquire);
            if stop_requested_now && stop_seen_at.is_none() {
                stop_seen_at = Some(std::time::Instant::now());
                info!("[swift_session] audio bridge draining after stop id={recording_id}");
            }
            let timeout = if stop_requested_now {
                STOP_DRAIN_QUIET
            } else {
                STOP_POLL_INTERVAL
            };
            let chunk_f32 = match sync_rx.recv_timeout(timeout) {
                Ok(chunk) => chunk,
                Err(mpsc::RecvTimeoutError::Timeout) if stop_requested_now => {
                    drain_reason = "stop_quiet";
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    drain_reason = "channel_closed";
                    break;
                }
            };
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            if let Some(gate) = &echo_gate {
                let decision = gate.filter_mic_samples_16k(&resampled);
                if !decision.allow {
                    chunks_dropped += 1;
                    if stop_seen_at.is_some_and(|started| started.elapsed() >= STOP_DRAIN_MAX) {
                        drain_reason = "stop_max_after_drops";
                        break;
                    }
                    continue;
                }
            }
            let pcm: Vec<u8> = resampled
                .iter()
                .flat_map(|&s| {
                    let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                    i16_val.to_le_bytes()
                })
                .collect();
            crate::recovery::append_pcm(&pcm);
            if let Some(mirror_tx) = &mirror_tx {
                let _ = mirror_tx.send(AudioMirrorCommand::Pcm(pcm.clone()));
            }
            let pcm_len = pcm.len();
            match session_tx.try_send(SessionCommand::Audio {
                id: recording_id.clone(),
                pcm,
            }) {
                Ok(()) => {
                    chunks_sent += 1;
                    bytes_sent += pcm_len;
                }
                Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                    chunks_dropped += 1;
                    continue;
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => break,
            }
            if stop_seen_at.is_some_and(|started| started.elapsed() >= STOP_DRAIN_MAX) {
                drain_reason = "stop_max";
                break;
            }
        }
        info!(
            "[swift_session] audio bridge drained id={} reason={} stop_requested={} chunks={} bytes={} dropped={} native_rate={} elapsed_ms={}",
            recording_id,
            drain_reason,
            stop_requested.load(Ordering::Acquire),
            chunks_sent,
            bytes_sent,
            chunks_dropped,
            native_rate,
            bridge_started.elapsed().as_millis()
        );
        if let Some(mirror_tx) = &mirror_tx {
            let _ = mirror_tx.send(AudioMirrorCommand::Finalize);
        }
        let mut finalize = Some(SessionCommand::Finalize {
            id: recording_id.clone(),
        });
        let finalize_started = std::time::Instant::now();
        let deadline = std::time::Instant::now() + WS_SEND_TIMEOUT;
        while let Some(cmd) = finalize.take() {
            match session_tx.try_send(cmd) {
                Ok(()) => {
                    info!(
                        "[swift_session] finalize enqueued id={} after_ms={}",
                        recording_id,
                        finalize_started.elapsed().as_millis()
                    );
                    break;
                }
                Err(tokio_mpsc::error::TrySendError::Full(cmd)) => {
                    if std::time::Instant::now() >= deadline {
                        warn!(
                            "[swift_session] finalize send timed out id={} after_ms={} chunks={} bytes={} dropped={}",
                            recording_id,
                            finalize_started.elapsed().as_millis(),
                            chunks_sent,
                            bytes_sent,
                            chunks_dropped
                        );
                        break;
                    }
                    finalize = Some(cmd);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
        debug!("[swift_session] audio bridge done id={recording_id}");
    });
    stop_handle
}

#[cfg(not(target_os = "macos"))]
pub mod stub {
    //! Non-macOS builds do not ship the Swift live STT path.
}
