//! Persistent Deepgram Live Streaming session.
//!
//! A single actor owns the Deepgram WebSocket. Recordings send audio chunks to
//! the actor by recording id, release sends `Finalize`, and the actor returns a
//! transcript over a per-recording oneshot. Between recordings it keeps the same
//! socket warm with a tiny silent audio prime followed by Deepgram `KeepAlive`
//! messages.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::echo_gate::EchoGateShared;
use crate::server_runtime_stream::AudioMirrorCommand;

use deepgram::{
    Deepgram,
    common::{
        options::{Encoding, Endpointing, Language, Model, Options, Replace},
        stream_response::StreamResponse,
    },
    listen::websocket::WebsocketHandle,
};
use said_core::deepgram::{
    BiasPackage, MAX_KEYTERMS, MAX_REPLACEMENTS, TranscriptMeta, endpointing_for_mode,
};
use said_recorder::{ChunkReceiver, SAMPLE_RATE, resample_to_16k};
use serde_json::Value;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tracing::{debug, info, warn};

/// Confidence threshold — words below this get [word?XX%] markers for the LLM.
const LOW_CONFIDENCE_THRESHOLD: f64 = 0.85;
/// Buffered command capacity while the WebSocket is connecting or briefly slow.
///
/// The recorder's CoreAudio callback uses a small non-blocking channel so it
/// never stalls the audio thread. The bridge thread drains that channel into
/// this actor-facing queue, which absorbs slow WS connects without dropping
/// early speech.
const AUDIO_BRIDGE_BUFFER_CHUNKS: usize = 4096;
const COMMAND_BUFFER_CHUNKS: usize = AUDIO_BRIDGE_BUFFER_CHUNKS + 128;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WS_SEND_TIMEOUT: Duration = Duration::from_secs(2);
const FINALIZE_WITH_TEXT_RESULT_TIMEOUT: Duration = Duration::from_millis(1500);
const FINALIZE_NO_RESULT_TIMEOUT: Duration = Duration::from_millis(900);
const FINALIZE_QUIET_TIMEOUT: Duration = Duration::from_millis(300);
const RESPONSE_DRAIN_POLL_TIMEOUT: Duration = Duration::from_millis(1);
const LIVE_STT_HEALTH_TIMEOUT: Duration = Duration::from_millis(700);
const PCM_SIGNAL_THRESHOLD: i16 = 500;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);
const WARM_PRIME_SILENCE_MS: usize = 200;
const MAX_IDLE_WARM_DURATION: Duration = Duration::from_secs(15 * 60);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(8);
const MAX_STREAMING_DURATION: Duration = Duration::from_secs(45);
const MEETING_WORD_LIMIT: usize = 150;

#[derive(Debug, Clone)]
pub struct StreamingTranscript {
    pub transcript: String,
    pub meta: TranscriptMeta,
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
    },
    Audio {
        id: String,
        pcm: Vec<u8>,
    },
    Finalize {
        id: String,
    },
    Reconfigure {
        deepgram_key: String,
        bias: BiasPackage,
    },
    Shutdown,
    GetState(oneshot::Sender<SessionState>),
}

pub type SessionSender = tokio_mpsc::Sender<SessionCommand>;

struct ActiveRecording {
    id: String,
    result_tx: Option<oneshot::Sender<Option<StreamingTranscript>>>,
    pre_embed: Option<(String, String)>,
    parts: Vec<String>,
    total_word_count: usize,
    total_low_confidence: usize,
    total_confidence: f64,
    languages_seen: Vec<String>,
    chunks_sent: usize,
    signal_chunks_sent: usize,
    started_at: tokio::time::Instant,
    first_signal_at: Option<tokio::time::Instant>,
    last_response_at: Option<tokio::time::Instant>,
    responses_seen: usize,
    result_messages_seen: usize,
    text_result_messages_seen: usize,
    live_stt_health_miss_logged: bool,
    utterance_end_tx: Option<tokio_mpsc::UnboundedSender<String>>,
}

impl ActiveRecording {
    fn new(
        id: String,
        result_tx: oneshot::Sender<Option<StreamingTranscript>>,
        pre_embed: Option<(String, String)>,
        utterance_end_tx: Option<tokio_mpsc::UnboundedSender<String>>,
    ) -> Self {
        Self {
            id,
            result_tx: Some(result_tx),
            pre_embed,
            parts: Vec::new(),
            total_word_count: 0,
            total_low_confidence: 0,
            total_confidence: 0.0,
            languages_seen: Vec::new(),
            chunks_sent: 0,
            signal_chunks_sent: 0,
            started_at: tokio::time::Instant::now(),
            first_signal_at: None,
            last_response_at: None,
            responses_seen: 0,
            result_messages_seen: 0,
            text_result_messages_seen: 0,
            live_stt_health_miss_logged: false,
            utterance_end_tx,
        }
    }

    fn note_audio_sent(&mut self, pcm: &[u8]) {
        self.chunks_sent += 1;
        if pcm_has_signal(pcm) {
            self.signal_chunks_sent += 1;
            if self.first_signal_at.is_none() {
                self.first_signal_at = Some(tokio::time::Instant::now());
            }
        }
    }

    fn note_response(&mut self) {
        self.responses_seen += 1;
        self.last_response_at = Some(tokio::time::Instant::now());
    }

    fn note_result_message(&mut self, has_text: bool) {
        self.result_messages_seen += 1;
        if has_text {
            self.text_result_messages_seen += 1;
        }
    }

    fn should_log_live_stt_health_miss(&mut self) -> bool {
        if self.live_stt_health_miss_logged {
            return false;
        }
        let should_log = self.text_result_messages_seen == 0
            && self
                .first_signal_at
                .is_some_and(|at| at.elapsed() >= LIVE_STT_HEALTH_TIMEOUT);
        if should_log {
            self.live_stt_health_miss_logged = true;
        }
        should_log
    }

    fn should_batch_rescue_on_release(&self) -> bool {
        self.text_result_messages_seen == 0
            && self
                .first_signal_at
                .is_some_and(|at| at.elapsed() >= LIVE_STT_HEALTH_TIMEOUT)
    }

    fn speech_elapsed_ms(&self) -> u128 {
        self.first_signal_at
            .map(|at| at.elapsed().as_millis())
            .unwrap_or(0)
    }

    fn finish(&mut self, stt_mode: String) -> Option<StreamingTranscript> {
        let full = self
            .parts
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        if full.is_empty() {
            warn!(
                "[dg_session] no transcript for {} — chunks={} elapsed={}ms",
                self.id,
                self.chunks_sent,
                self.started_at.elapsed().as_millis()
            );
            return None;
        }

        if let Some((url, secret)) = self.pre_embed.take() {
            let plain = plain_for_embed(&self.parts);
            if !plain.is_empty() {
                debug!("[dg_session] firing pre-embed ({} chars)", plain.len());
                tokio::spawn(async move {
                    let client = reqwest::Client::new();
                    let body = serde_json::json!({ "text": plain });
                    let _ = client
                        .post(&url)
                        .header("Authorization", format!("Bearer {secret}"))
                        .json(&body)
                        .timeout(Duration::from_secs(5))
                        .send()
                        .await;
                });
            }
        }

        let mean_word_confidence = if self.total_word_count == 0 {
            0.0
        } else {
            self.total_confidence / self.total_word_count as f64
        };
        Some(StreamingTranscript {
            transcript: full.clone(),
            meta: TranscriptMeta {
                enriched_transcript: full,
                confidence: mean_word_confidence,
                mean_word_confidence,
                low_confidence_count: self.total_low_confidence,
                word_count: self.total_word_count,
                languages: self.languages_seen.clone(),
                stt_mode,
            },
        })
    }

    fn send_result(&mut self, transcript: Option<StreamingTranscript>) {
        if let Some(tx) = self.result_tx.take() {
            let _ = tx.send(transcript);
        }
    }
}

pub struct DeepgramSession {
    state: SessionState,
    cmd_rx: tokio_mpsc::Receiver<SessionCommand>,
    deepgram_key: String,
    bias: BiasPackage,
    consecutive_failures: u32,
}

impl DeepgramSession {
    pub fn spawn() -> SessionSender {
        let (cmd_tx, cmd_rx) = tokio_mpsc::channel(COMMAND_BUFFER_CHUNKS);
        let session = Self {
            state: SessionState::Disconnected,
            cmd_rx,
            deepgram_key: String::new(),
            bias: BiasPackage::default(),
            consecutive_failures: 0,
        };
        tauri::async_runtime::spawn(session.run());
        cmd_tx
    }

    async fn run(mut self) {
        info!("[dg_session] actor started");
        loop {
            if self.deepgram_key.is_empty() {
                self.set_state(SessionState::Disconnected);
                if !self.wait_for_config_or_shutdown().await {
                    return;
                }
                continue;
            }

            self.set_state(SessionState::Connecting);
            match connect_ws(&self.deepgram_key, &self.bias).await {
                Some(ws) => {
                    self.consecutive_failures = 0;
                    self.set_state(SessionState::Idle);
                    if !self.event_loop(ws).await {
                        return;
                    }
                }
                None => {
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    self.set_state(SessionState::Disconnected);
                    if !self.backoff_or_shutdown().await {
                        return;
                    }
                }
            }
        }
    }

    async fn event_loop(&mut self, mut ws: WebsocketHandle) -> bool {
        let mut active: Option<ActiveRecording> = None;
        let mut keepalive_interval = tokio::time::interval(KEEPALIVE_INTERVAL);
        keepalive_interval.reset();
        let mut reconnect_after_idle = false;
        let mut recording_deadline: Option<tokio::time::Instant> = None;
        let mut idle_warm_deadline = tokio::time::Instant::now() + MAX_IDLE_WARM_DURATION;

        loop {
            tokio::select! {
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        SessionCommand::StartRecording { id, result_tx, pre_embed, utterance_end_tx } => {
                            if self.state == SessionState::Idle {
                                info!(
                                    "[dg_session] start recording id={id} state={} keyterms={} replacements={}",
                                    self.state,
                                    self.bias.keyterms.len(),
                                    self.bias.replacements.len()
                                );
                                active = Some(ActiveRecording::new(
                                    id,
                                    result_tx,
                                    pre_embed,
                                    utterance_end_tx,
                                ));
                                recording_deadline = Some(
                                    tokio::time::Instant::now() + MAX_STREAMING_DURATION
                                );
                                self.set_state(SessionState::Streaming);
                            } else {
                                warn!("[dg_session] start rejected id={id} — state={}", self.state);
                                let _ = result_tx.send(None);
                            }
                        }
                        SessionCommand::Audio { id, pcm } => {
                            let Some(current) = active.as_mut() else {
                                continue;
                            };
                            if current.id != id || self.state != SessionState::Streaming {
                                continue;
                            }
                            current.note_audio_sent(&pcm);
                            if let Err(e) = send_audio_with_timeout(&mut ws, pcm, &current.id).await {
                                warn!("[dg_session] {e} — falling back to batch");
                                current.send_result(None);
                                return true;
                            }
                            if !drain_available_responses(&mut ws, &mut active).await {
                                return true;
                            }
                            if let Some(current) = active.as_mut() {
                                if current.should_log_live_stt_health_miss() {
                                    warn!(
                                        "[dg_session] no live text result after {}ms of speech id={} chunks={} signal_chunks={} ws_messages={} — will batch-rescue on release unless WS recovers",
                                        LIVE_STT_HEALTH_TIMEOUT.as_millis(),
                                        current.id,
                                        current.chunks_sent,
                                        current.signal_chunks_sent,
                                        current.responses_seen
                                    );
                                }
                            }
                            keepalive_interval.reset();
                        }
                        SessionCommand::Finalize { id } => {
                            let Some(mut current) = active.take() else {
                                continue;
                            };
                            if current.id != id {
                                active = Some(current);
                                continue;
                            }
                            recording_deadline = None;
                            if current.should_batch_rescue_on_release() {
                                warn!(
                                    "[dg_session] no live text result by release id={} speech_elapsed={}ms chunks={} signal_chunks={} ws_messages={} result_messages={} — starting batch STT immediately and reconnecting WS",
                                    current.id,
                                    current.speech_elapsed_ms(),
                                    current.chunks_sent,
                                    current.signal_chunks_sent,
                                    current.responses_seen,
                                    current.result_messages_seen
                                );
                                current.send_result(None);
                                self.set_state(SessionState::Disconnected);
                                return true;
                            }
                            self.set_state(SessionState::Finalizing);
                            if let Err(e) = send_finalize_with_timeout(&mut ws, &id).await {
                                warn!("[dg_session] {e}");
                                current.send_result(None);
                                return true;
                            }

                            let drained = self.drain_finalize(&mut ws, &mut current).await;
                            if !drained.saw_finalize_result {
                                warn!(
                                    "[dg_session] finalize did not produce a final marker id={} after {} result message(s) — falling back to batch and reconnecting to prevent transcript carry-over",
                                    current.id,
                                    current.result_messages_seen
                                );
                                current.send_result(None);
                                self.set_state(SessionState::Disconnected);
                                return true;
                            }
                            let had_transcript = drained.transcript.is_some();
                            current.send_result(drained.transcript);
                            debug!(
                                "[dg_session] finalized id={id} transcript={} chunks={} elapsed={}ms",
                                had_transcript,
                                current.chunks_sent,
                                current.started_at.elapsed().as_millis()
                            );
                            self.set_state(SessionState::Idle);
                            keepalive_interval.reset();
                            idle_warm_deadline = tokio::time::Instant::now() + MAX_IDLE_WARM_DURATION;
                            if reconnect_after_idle {
                                info!("[dg_session] reconnecting after idle to apply latest bias");
                                return true;
                            }
                        }
                        SessionCommand::Reconfigure { deepgram_key, bias } => {
                            let changed = self.deepgram_key != deepgram_key || self.bias != bias;
                            self.deepgram_key = deepgram_key;
                            self.bias = bias;
                            info!(
                                "[dg_session] reconfigured — changed={} state={} mode={} keyterms={} replacements={}",
                                changed,
                                self.state,
                                self.bias.stt_mode,
                                self.bias.keyterms.len(),
                                self.bias.replacements.len()
                            );
                            if changed {
                                if self.state == SessionState::Idle {
                                    return true;
                                }
                                reconnect_after_idle = true;
                            }
                        }
                        SessionCommand::Shutdown => {
                            let _ = send_close_with_timeout(&mut ws, "shutdown").await;
                            self.set_state(SessionState::Disconnected);
                            return false;
                        }
                        SessionCommand::GetState(reply) => {
                            let _ = reply.send(self.state);
                        }
                    }
                }
                _ = keepalive_interval.tick() => {
                    if self.state == SessionState::Idle {
                        if let Err(e) = send_keepalive_with_timeout(&mut ws, "idle").await {
                            warn!("[dg_session] {e}");
                            fail_active(&mut active);
                            return true;
                        }
                        if !drain_available_responses(&mut ws, &mut active).await {
                            return true;
                        }
                        debug!("[dg_session] KeepAlive sent");
                    }
                }
                _ = tokio::time::sleep_until(idle_warm_deadline), if self.state == SessionState::Idle => {
                    info!(
                        "[dg_session] closing warm idle WS after {}s without recording",
                        MAX_IDLE_WARM_DURATION.as_secs()
                    );
                    let _ = send_close_with_timeout(&mut ws, "warm-idle-expired").await;
                    self.set_state(SessionState::Disconnected);
                    return true;
                }
                _ = async {
                    if let Some(deadline) = recording_deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if recording_deadline.is_some() => {
                    if let Some(current) = active.as_mut() {
                        warn!(
                            "[dg_session] streaming watchdog timed out id={} after {}ms — falling back to batch and reconnecting",
                            current.id,
                            current.started_at.elapsed().as_millis()
                        );
                        current.send_result(None);
                    }
                    return true;
                }
            }
        }
    }

    async fn drain_finalize(
        &self,
        ws: &mut WebsocketHandle,
        current: &mut ActiveRecording,
    ) -> FinalizeDrain {
        let drain_start = tokio::time::Instant::now();
        let timeout = if current.text_result_messages_seen > 0 {
            FINALIZE_WITH_TEXT_RESULT_TIMEOUT
        } else {
            FINALIZE_NO_RESULT_TIMEOUT
        };
        let deadline = drain_start + timeout;
        let mut saw_finalize_result = false;
        let mut saw_any_after_finalize = false;

        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            let wait = if saw_any_after_finalize {
                FINALIZE_QUIET_TIMEOUT.min(deadline.saturating_duration_since(now))
            } else {
                deadline.saturating_duration_since(now)
            };

            match tokio::time::timeout(wait, ws.receive()).await {
                Ok(Some(Ok(response))) => {
                    current.note_response();
                    let outcome = collect_response(
                        response,
                        Some(&current.id),
                        "finalize",
                        &mut current.parts,
                        &mut current.total_word_count,
                        &mut current.total_low_confidence,
                        &mut current.total_confidence,
                        &mut current.languages_seen,
                    );
                    if outcome.is_result {
                        current.note_result_message(outcome.has_text);
                    }
                    if outcome.collected {
                        saw_any_after_finalize = true;
                    }
                    if outcome.from_finalize {
                        // Deepgram can emit more than one final result around
                        // release, especially after a user pause split the
                        // recording into utterances. Treat `from_finalize` as
                        // a signal that finalization started, not as permission
                        // to stop reading immediately.
                        saw_finalize_result = true;
                    }
                }
                Ok(None) => {
                    warn!("[dg_session] WS closed during finalize id={}", current.id);
                    break;
                }
                Ok(Some(Err(e))) => {
                    warn!(
                        "[dg_session] WS receive error during finalize id={}: {e}",
                        current.id
                    );
                    break;
                }
                Err(_) if saw_any_after_finalize => break,
                Err(_) => break,
            }
        }

        let drain_ms = drain_start.elapsed().as_millis();
        debug!(
            "[dg_session] finalize drain id={} drain={}ms timeout={}ms ws_messages={} result_messages={} text_results={} from_finalize={} parts={}",
            current.id,
            drain_ms,
            timeout.as_millis(),
            current.responses_seen,
            current.result_messages_seen,
            current.text_result_messages_seen,
            saw_finalize_result,
            current.parts.len()
        );
        FinalizeDrain {
            transcript: current.finish(self.bias.stt_mode.clone()),
            saw_finalize_result,
        }
    }

    async fn wait_for_config_or_shutdown(&mut self) -> bool {
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                SessionCommand::Reconfigure { deepgram_key, bias } => {
                    self.deepgram_key = deepgram_key;
                    self.bias = bias;
                    info!(
                        "[dg_session] configured while disconnected — key={} mode={} keyterms={} replacements={}",
                        !self.deepgram_key.is_empty(),
                        self.bias.stt_mode,
                        self.bias.keyterms.len(),
                        self.bias.replacements.len()
                    );
                    return true;
                }
                SessionCommand::StartRecording { result_tx, id, .. } => {
                    warn!("[dg_session] unavailable at start id={id} — falling back to batch");
                    let _ = result_tx.send(None);
                }
                SessionCommand::Shutdown => return false,
                SessionCommand::GetState(reply) => {
                    let _ = reply.send(self.state);
                }
                SessionCommand::Audio { .. } | SessionCommand::Finalize { .. } => {}
            }
        }
        false
    }

    async fn backoff_or_shutdown(&mut self) -> bool {
        let backoff = self.backoff_duration();
        warn!(
            "[dg_session] reconnect backoff {:?} after {} failure(s)",
            backoff, self.consecutive_failures
        );
        let deadline = tokio::time::sleep(backoff);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return true,
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        SessionCommand::Reconfigure { deepgram_key, bias } => {
                            self.deepgram_key = deepgram_key;
                            self.bias = bias;
                            return true;
                        }
                SessionCommand::StartRecording { result_tx, id, .. } => {
                            warn!("[dg_session] unavailable during backoff id={id} — falling back to batch");
                            let _ = result_tx.send(None);
                        }
                        SessionCommand::Shutdown => return false,
                        SessionCommand::GetState(reply) => {
                            let _ = reply.send(self.state);
                        }
                        SessionCommand::Audio { .. } | SessionCommand::Finalize { .. } => {}
                    }
                }
            }
        }
    }

    fn set_state(&mut self, new_state: SessionState) {
        if self.state != new_state {
            debug!("[dg_session] state {} → {}", self.state, new_state);
            self.state = new_state;
        }
    }

    fn backoff_duration(&self) -> Duration {
        let exp = self.consecutive_failures.saturating_sub(1).min(4);
        Duration::from_millis(500 * 2_u64.saturating_pow(exp)).min(RECONNECT_BACKOFF_MAX)
    }
}

fn fail_active(active: &mut Option<ActiveRecording>) {
    if let Some(mut current) = active.take() {
        current.send_result(None);
    }
}

fn pcm_has_signal(pcm: &[u8]) -> bool {
    pcm.chunks_exact(2).any(|bytes| {
        let sample = i16::from_le_bytes([bytes[0], bytes[1]]);
        sample.saturating_abs() >= PCM_SIGNAL_THRESHOLD
    })
}

async fn drain_available_responses(
    ws: &mut WebsocketHandle,
    active: &mut Option<ActiveRecording>,
) -> bool {
    loop {
        match tokio::time::timeout(RESPONSE_DRAIN_POLL_TIMEOUT, ws.receive()).await {
            Ok(Some(Ok(response))) => {
                let Ok(v) = serde_json::to_value(response) else {
                    continue;
                };
                if let Some(current) = active.as_mut() {
                    current.note_response();
                    let outcome = collect_result_value(
                        &v,
                        Some(&current.id),
                        "stream",
                        &mut current.parts,
                        &mut current.total_word_count,
                        &mut current.total_low_confidence,
                        &mut current.total_confidence,
                        &mut current.languages_seen,
                    );
                    if outcome.is_result {
                        current.note_result_message(outcome.has_text);
                    }
                    if outcome.utterance_end {
                        if let Some(tx) = &current.utterance_end_tx {
                            let _ = tx.send(current.id.clone());
                        }
                    }
                    if outcome.collected {
                        debug!("[dg_session] collected segment id={}", current.id);
                        if current.total_word_count >= MEETING_WORD_LIMIT {
                            if let Some(tx) = &current.utterance_end_tx {
                                info!(
                                    "[dg_session] word limit ({MEETING_WORD_LIMIT}) reached ({} words) — signaling chunk finalize",
                                    current.total_word_count
                                );
                                let _ = tx.send(current.id.clone());
                            }
                        }
                    }
                } else {
                    log_stream_response(&v, None, "idle");
                }
            }
            Ok(Some(Err(e))) => {
                warn!("[dg_session] WS receive error: {e}");
                fail_active(active);
                return false;
            }
            Ok(None) => {
                warn!("[dg_session] WS closed by server");
                fail_active(active);
                return false;
            }
            Err(_) => return true,
        }
    }
}

async fn sdk_send_with_timeout<F, Fut, E>(
    operation: F,
    label: &str,
    recording_id: &str,
) -> Result<(), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(WS_SEND_TIMEOUT, operation()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!(
            "Deepgram SDK {label} send failed id={recording_id} after {}ms: {e}",
            WS_SEND_TIMEOUT.as_millis()
        )),
        Err(_) => Err(format!(
            "Deepgram SDK {label} send timed out id={recording_id} after {}ms",
            WS_SEND_TIMEOUT.as_millis()
        )),
    }
}

async fn send_audio_with_timeout(
    ws: &mut WebsocketHandle,
    pcm: Vec<u8>,
    recording_id: &str,
) -> Result<(), String> {
    sdk_send_with_timeout(|| ws.send_data(pcm), "audio", recording_id).await
}

async fn send_finalize_with_timeout(
    ws: &mut WebsocketHandle,
    recording_id: &str,
) -> Result<(), String> {
    sdk_send_with_timeout(|| ws.finalize(), "Finalize", recording_id).await
}

async fn send_keepalive_with_timeout(
    ws: &mut WebsocketHandle,
    recording_id: &str,
) -> Result<(), String> {
    sdk_send_with_timeout(|| ws.keep_alive(), "KeepAlive", recording_id).await
}

async fn send_close_with_timeout(
    ws: &mut WebsocketHandle,
    recording_id: &str,
) -> Result<(), String> {
    sdk_send_with_timeout(|| ws.close_stream(), "CloseStream", recording_id).await
}

fn warm_prime_silence_pcm() -> Vec<u8> {
    let sample_count = SAMPLE_RATE as usize * WARM_PRIME_SILENCE_MS / 1000;
    vec![0_u8; sample_count * 2]
}

async fn connect_ws(deepgram_key: &str, bias: &BiasPackage) -> Option<WebsocketHandle> {
    if deepgram_key.is_empty() {
        return None;
    }

    let client = match Deepgram::new(deepgram_key) {
        Ok(client) => client,
        Err(e) => {
            warn!("[dg_session] Deepgram SDK client init failed: {e}");
            return None;
        }
    };
    let options = sdk_options(bias);
    let transcription = client.transcription();
    let builder = transcription
        .stream_request_with_options(options)
        .encoding(Encoding::Linear16)
        .sample_rate(SAMPLE_RATE)
        .channels(1)
        .interim_results(true)
        .endpointing(Endpointing::CustomDurationMs(endpointing_for_mode(
            &bias.stt_mode,
        )))
        .vad_events(true)
        .utterance_end_ms(2000)
        .keep_alive();

    let start = tokio::time::Instant::now();
    let result = tokio::time::timeout(CONNECT_TIMEOUT, builder.handle()).await;
    let ms = start.elapsed().as_millis();

    match result {
        Err(_) => {
            warn!("[dg_session] WS connect timed out after {ms}ms");
            None
        }
        Ok(Err(e)) => {
            warn!("[dg_session] WS connect failed after {ms}ms: {e}");
            None
        }
        Ok(Ok(mut ws)) => {
            info!(
                "[dg_session] ✓ SDK WS connected in {ms}ms request_id={} (lang={} keyterms={} replacements={})",
                ws.request_id(),
                bias.stt_mode,
                bias.keyterms.len(),
                bias.replacements.len(),
            );
            let prime = warm_prime_silence_pcm();
            let prime_bytes = prime.len();
            if let Err(e) = send_audio_with_timeout(&mut ws, prime, "warm-prime").await {
                warn!("[dg_session] warm prime failed after WS connect: {e}");
                return None;
            }
            info!(
                "[dg_session] warm prime sent: {}ms silence ({} bytes)",
                WARM_PRIME_SILENCE_MS, prime_bytes
            );
            Some(ws)
        }
    }
}

fn sdk_options(bias: &BiasPackage) -> Options {
    let keyterms = bias
        .keyterms
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty())
        .take(MAX_KEYTERMS);
    let replacements = bias
        .replacements
        .iter()
        .filter_map(|replacement| {
            let find = replacement.find.trim();
            if find.is_empty() {
                return None;
            }
            Some(Replace {
                find: find.to_string(),
                replace: replacement
                    .replace
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
            })
        })
        .take(MAX_REPLACEMENTS);

    Options::builder()
        .model(Model::Nova3)
        .language(Language::from(bias.stt_mode.clone()))
        .smart_format(false)
        .punctuate(false)
        .keyterms(keyterms)
        .replace(replacements)
        .build()
}

/// Discard live PCM chunks when batch-only STT (e.g. groq_whisper) is active.
/// The recorder still needs a consumer or its sync channel backs up.
pub fn spawn_chunk_drain(recording_id: String, chunk_recv: ChunkReceiver) {
    std::thread::spawn(move || {
        let sync_rx = chunk_recv.rx;
        let mut drained = 0usize;
        while sync_rx.recv().is_ok() {
            drained += 1;
        }
        debug!(
            "[dg_session] chunk drain finished id={} chunks={}",
            recording_id, drained
        );
    });
}

pub fn spawn_audio_bridge(
    recording_id: String,
    chunk_recv: ChunkReceiver,
    session_tx: SessionSender,
) {
    spawn_audio_bridge_with_echo_gate(recording_id, chunk_recv, session_tx, None, None);
}

pub fn spawn_audio_bridge_with_echo_gate(
    recording_id: String,
    chunk_recv: ChunkReceiver,
    session_tx: SessionSender,
    echo_gate: Option<Arc<EchoGateShared>>,
    mirror_tx: Option<tokio::sync::mpsc::UnboundedSender<AudioMirrorCommand>>,
) {
    std::thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx: mpsc::Receiver<Vec<f32>> = chunk_recv.rx;
        let mut bridged_chunks = 0usize;
        let mut dropped_echo_chunks = 0usize;
        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            if let Some(gate) = &echo_gate {
                let decision = gate.filter_mic_samples_16k(&resampled);
                if !decision.allow {
                    dropped_echo_chunks += 1;
                    if dropped_echo_chunks == 1 || dropped_echo_chunks % 50 == 0 {
                        debug!(
                            "[dg_session] echo gate dropped chunk id={} reason={} mic_rms={:.4} speaker_rms={:.4} corr={:.3} dropped={}",
                            recording_id,
                            decision.reason,
                            decision.mic_rms,
                            decision.speaker_rms,
                            decision.correlation,
                            dropped_echo_chunks
                        );
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
            // Crash-safe recovery tap: persist the exact PCM we stream so a crash
            // mid-dictation doesn't lose the user's words. No-op unless a capture
            // session is active (non-meeting recordings only).
            crate::recovery::append_pcm(&pcm);
            if let Some(mirror_tx) = &mirror_tx {
                let _ = mirror_tx.send(AudioMirrorCommand::Pcm(pcm.clone()));
            }
            if session_tx
                .blocking_send(SessionCommand::Audio {
                    id: recording_id.clone(),
                    pcm,
                })
                .is_err()
            {
                break;
            }
            bridged_chunks += 1;
        }
        if let Some(mirror_tx) = &mirror_tx {
            let _ = mirror_tx.send(AudioMirrorCommand::Finalize);
        }
        let _ = session_tx.blocking_send(SessionCommand::Finalize {
            id: recording_id.clone(),
        });
        debug!(
            "[dg_session] audio bridge closed id={recording_id} chunks={bridged_chunks} echo_dropped={dropped_echo_chunks}"
        );
    });
}

#[derive(Default)]
struct CollectOutcome {
    collected: bool,
    from_finalize: bool,
    is_result: bool,
    has_text: bool,
    utterance_end: bool,
}

struct FinalizeDrain {
    transcript: Option<StreamingTranscript>,
    saw_finalize_result: bool,
}

fn collect_response(
    response: StreamResponse,
    recording_id: Option<&str>,
    phase: &str,
    parts: &mut Vec<String>,
    word_count: &mut usize,
    low_conf: &mut usize,
    conf_sum: &mut f64,
    langs: &mut Vec<String>,
) -> CollectOutcome {
    let Ok(v) = serde_json::to_value(response) else {
        return CollectOutcome::default();
    };
    collect_result_value(
        &v,
        recording_id,
        phase,
        parts,
        word_count,
        low_conf,
        conf_sum,
        langs,
    )
}

fn collect_result_value(
    v: &Value,
    recording_id: Option<&str>,
    phase: &str,
    parts: &mut Vec<String>,
    word_count: &mut usize,
    low_conf: &mut usize,
    conf_sum: &mut f64,
    langs: &mut Vec<String>,
) -> CollectOutcome {
    log_stream_response(v, recording_id, phase);
    if v["type"].as_str().unwrap_or("") != "Results" {
        return CollectOutcome {
            utterance_end: stream_response_type(v) == "UtteranceEnd",
            ..CollectOutcome::default()
        };
    }
    let from_finalize = v["from_finalize"].as_bool().unwrap_or(false);
    let has_text = result_has_text(v);
    if phase == "stream" && from_finalize {
        warn!(
            "[dg_session] dropping stale finalize result during new stream id={} words={} chars={}",
            recording_id.unwrap_or("-"),
            result_word_count(v),
            result_transcript(v).chars().count()
        );
        return CollectOutcome {
            collected: false,
            from_finalize,
            is_result: true,
            has_text,
            utterance_end: false,
        };
    }
    if !v["is_final"].as_bool().unwrap_or(false) {
        return CollectOutcome {
            collected: false,
            from_finalize,
            is_result: true,
            has_text,
            utterance_end: false,
        };
    }
    if let Some(chunk) = extract_result_chunk(&v) {
        info!("[dg_session] segment: \"{}\"", chunk.enriched);
        parts.push(chunk.enriched);
        *word_count += chunk.word_count;
        *low_conf += chunk.low_confidence_count;
        *conf_sum += chunk.confidence_sum;
        for language in chunk.languages {
            if !langs.iter().any(|seen| seen == &language) {
                langs.push(language);
            }
        }
        return CollectOutcome {
            collected: true,
            from_finalize,
            is_result: true,
            has_text: true,
            utterance_end: false,
        };
    }
    CollectOutcome {
        collected: false,
        from_finalize,
        is_result: true,
        has_text,
        utterance_end: false,
    }
}

fn log_stream_response(v: &Value, recording_id: Option<&str>, phase: &str) {
    let recording_id = recording_id.unwrap_or("-");
    let message_type = stream_response_type(v);
    if message_type == "Results" {
        let is_final = v["is_final"].as_bool().unwrap_or(false);
        let speech_final = v["speech_final"].as_bool().unwrap_or(false);
        let from_finalize = v["from_finalize"].as_bool().unwrap_or(false);
        let transcript_chars = result_transcript(v).chars().count();
        let words = result_word_count(v);
        if is_final {
            info!("[dg_session] transcript ({words} words, {transcript_chars} chars)");
        } else {
            debug!(
                "[dg_session] WS Results phase={phase} id={recording_id} final={is_final} speech_final={speech_final} from_finalize={from_finalize} transcript_chars={transcript_chars} words={words}"
            );
        }
        return;
    }

    if message_type == "SpeechStarted" {
        let ts = v["timestamp"].as_f64().unwrap_or_default();
        debug!("[dg_session] WS SpeechStarted phase={phase} id={recording_id} timestamp={ts:.3}");
        return;
    }

    if message_type == "UtteranceEnd" {
        let last_word_end = v["last_word_end"].as_f64().unwrap_or_default();
        info!(
            "[dg_session] WS UtteranceEnd phase={phase} id={recording_id} last_word_end={last_word_end:.3}"
        );
        return;
    }

    if message_type == "Terminal" {
        let request_id = v["request_id"].as_str().unwrap_or("-");
        let duration = v["duration"].as_f64().unwrap_or_default();
        info!(
            "[dg_session] WS Terminal phase={phase} id={recording_id} request_id={request_id} duration={duration:.3}"
        );
        return;
    }

    info!("[dg_session] WS {message_type} phase={phase} id={recording_id}");
}

fn stream_response_type(v: &Value) -> &str {
    if let Some(message_type) = v["type"].as_str() {
        return message_type;
    }
    if v.get("request_id").is_some() && v.get("created").is_some() {
        return "Terminal";
    }
    "Unknown"
}

fn result_has_text(v: &Value) -> bool {
    !result_transcript(v).trim().is_empty() || result_word_count(v) > 0
}

fn result_transcript(v: &Value) -> &str {
    v["channel"]["alternatives"]
        .as_array()
        .and_then(|alternatives| alternatives.first())
        .and_then(|alt| alt["transcript"].as_str())
        .unwrap_or("")
}

fn result_word_count(v: &Value) -> usize {
    v["channel"]["alternatives"]
        .as_array()
        .and_then(|alternatives| alternatives.first())
        .and_then(|alt| alt["words"].as_array())
        .map_or(0, Vec::len)
}

/// Build enriched text from Deepgram's `words` array inside a Results message.
/// Words with confidence < threshold are marked `[word?XX%]` so the LLM knows
/// which words to scrutinize for context-based correction.
///
/// Falls back to joining plain `punctuated_word`/`word` fields if parsing fails.
struct ResultChunk {
    enriched: String,
    word_count: usize,
    low_confidence_count: usize,
    confidence_sum: f64,
    languages: Vec<String>,
}

fn extract_result_chunk(v: &Value) -> Option<ResultChunk> {
    let alt = v["channel"]["alternatives"].as_array()?.first()?;
    let words_val = &alt["words"];
    let Some(words) = words_val.as_array() else {
        return None;
    };
    if words.is_empty() {
        return None;
    }

    let mut parts = Vec::with_capacity(words.len());
    let mut languages = Vec::new();
    let mut confidence_sum = 0.0_f64;
    let mut low_confidence_count = 0usize;
    for w in words {
        let word = w["punctuated_word"]
            .as_str()
            .or_else(|| w["word"].as_str())
            .unwrap_or("");
        if word.is_empty() {
            continue;
        }

        let conf = w["confidence"].as_f64().unwrap_or(1.0);
        confidence_sum += conf;
        if let Some(language) = w["language"].as_str() {
            if !languages.iter().any(|seen| seen == language) {
                languages.push(language.to_string());
            }
        }
        if conf < LOW_CONFIDENCE_THRESHOLD {
            low_confidence_count += 1;
        }
        parts.push(word.to_string());
    }

    if let Some(alt_languages) = alt["languages"].as_array() {
        for language in alt_languages.iter().filter_map(|value| value.as_str()) {
            if !languages.iter().any(|seen| seen == language) {
                languages.push(language.to_string());
            }
        }
    }

    Some(ResultChunk {
        enriched: parts.join(" "),
        word_count: words.len(),
        low_confidence_count,
        confidence_sum,
        languages,
    })
}

/// Strip `[word?XX%]` confidence markers and join transcript parts into plain
/// text suitable for embedding.  The embedding cache key is SHA256 of the plain
/// transcript, so this must match what voice.rs does in `strip_confidence_markers`.
fn plain_for_embed(parts: &[String]) -> String {
    let joined = parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let mut result = String::with_capacity(joined.len());
    let mut chars = joined.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut inner = String::new();
            let mut closed = false;
            for ic in chars.by_ref() {
                if ic == ']' {
                    closed = true;
                    break;
                }
                inner.push(ic);
            }
            if closed {
                if let Some(qpos) = inner.rfind('?') {
                    let after = &inner[qpos + 1..];
                    if after.ends_with('%') && after[..after.len() - 1].parse::<f64>().is_ok() {
                        result.push_str(&inner[..qpos]);
                        continue;
                    }
                }
                result.push('[');
                result.push_str(&inner);
                result.push(']');
            } else {
                result.push('[');
                result.push_str(&inner);
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        AUDIO_BRIDGE_BUFFER_CHUNKS, ActiveRecording, DeepgramSession, FINALIZE_NO_RESULT_TIMEOUT,
        FINALIZE_QUIET_TIMEOUT, FINALIZE_WITH_TEXT_RESULT_TIMEOUT, LIVE_STT_HEALTH_TIMEOUT,
        SessionState, WARM_PRIME_SILENCE_MS, collect_result_value, extract_result_chunk,
        pcm_has_signal, warm_prime_silence_pcm,
    };
    use said_core::deepgram::{BiasPackage, ReplacementRule, build_ws_url};
    use said_recorder::SAMPLE_RATE;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn ws_url_uses_multi_mode_and_replacements() {
        let bias = BiasPackage {
            stt_mode: "multi".into(),
            keyterms: vec!["AcmeCorp".into()],
            replacements: vec![ReplacementRule {
                find: "ack me".into(),
                replace: Some("AcmeCorp".into()),
            }],
        };
        let url = build_ws_url("wss://api.deepgram.com/v1/listen", &bias, SAMPLE_RATE);
        assert!(url.contains("language=multi"));
        assert!(url.contains("endpointing=1000"));
        assert!(url.contains("utterance_end_ms=2000"));
        assert!(url.contains("replace=ack%20me:AcmeCorp"));
    }

    #[test]
    fn audio_bridge_buffer_covers_slow_ws_connects() {
        assert!(
            AUDIO_BRIDGE_BUFFER_CHUNKS >= 4096,
            "audio bridge must absorb several seconds of mic chunks while WS connects"
        );
    }

    #[test]
    fn result_chunk_tracks_languages_and_confidence() {
        let value = json!({
            "channel": {
                "alternatives": [{
                    "languages": ["hi", "en"],
                    "words": [
                        { "word": "AcmeCorp", "confidence": 0.95, "language": "en" },
                        { "word": "hai", "confidence": 0.61, "language": "hi" }
                    ]
                }]
            }
        });
        let chunk = extract_result_chunk(&value).expect("chunk");
        assert_eq!(chunk.word_count, 2);
        assert_eq!(chunk.low_confidence_count, 1);
        assert!(chunk.languages.contains(&"en".to_string()));
        assert!(chunk.enriched.contains("hai"));
    }

    #[test]
    fn pcm_signal_detector_ignores_silence_and_catches_speech() {
        assert!(!pcm_has_signal(&vec![0_u8; 64]));
        let mut pcm = Vec::new();
        pcm.extend_from_slice(&0_i16.to_le_bytes());
        pcm.extend_from_slice(&600_i16.to_le_bytes());
        assert!(pcm_has_signal(&pcm));
    }

    #[test]
    fn warm_prime_is_short_valid_silence() {
        let pcm = warm_prime_silence_pcm();
        let expected_samples = SAMPLE_RATE as usize * WARM_PRIME_SILENCE_MS / 1000;
        assert_eq!(pcm.len(), expected_samples * 2);
        assert!(!pcm.is_empty());
        assert!(!pcm_has_signal(&pcm));
    }

    #[test]
    fn live_stt_health_miss_is_diagnostic_until_release() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let mut recording = ActiveRecording::new("test".into(), tx, None, None);
        recording.first_signal_at =
            Some(tokio::time::Instant::now() - LIVE_STT_HEALTH_TIMEOUT - Duration::from_millis(1));

        assert!(recording.should_log_live_stt_health_miss());
        assert!(!recording.should_log_live_stt_health_miss());
        assert!(recording.result_tx.is_some());
    }

    #[test]
    fn streaming_phase_drops_late_finalize_results() {
        let value = json!({
            "type": "Results",
            "is_final": true,
            "from_finalize": true,
            "channel": {
                "alternatives": [{
                    "transcript": "left over words",
                    "words": [
                        { "word": "left", "punctuated_word": "left", "confidence": 0.96 },
                        { "word": "over", "punctuated_word": "over", "confidence": 0.96 },
                        { "word": "words", "punctuated_word": "words", "confidence": 0.96 }
                    ]
                }]
            }
        });
        let mut parts = Vec::new();
        let mut word_count = 0;
        let mut low_conf = 0;
        let mut conf_sum = 0.0;
        let mut languages = Vec::new();

        let outcome = collect_result_value(
            &value,
            Some("new-recording"),
            "stream",
            &mut parts,
            &mut word_count,
            &mut low_conf,
            &mut conf_sum,
            &mut languages,
        );

        assert!(outcome.from_finalize);
        assert!(outcome.is_result);
        assert!(outcome.has_text);
        assert!(!outcome.collected);
        assert!(parts.is_empty());
        assert_eq!(word_count, 0);
    }

    #[test]
    fn finalize_phase_collects_finalize_results() {
        let value = json!({
            "type": "Results",
            "is_final": true,
            "from_finalize": true,
            "channel": {
                "alternatives": [{
                    "transcript": "final words",
                    "words": [
                        { "word": "final", "punctuated_word": "final", "confidence": 0.97 },
                        { "word": "words", "punctuated_word": "words", "confidence": 0.97 }
                    ]
                }]
            }
        });
        let mut parts = Vec::new();
        let mut word_count = 0;
        let mut low_conf = 0;
        let mut conf_sum = 0.0;
        let mut languages = Vec::new();

        let outcome = collect_result_value(
            &value,
            Some("same-recording"),
            "finalize",
            &mut parts,
            &mut word_count,
            &mut low_conf,
            &mut conf_sum,
            &mut languages,
        );

        assert!(outcome.from_finalize);
        assert!(outcome.collected);
        assert_eq!(parts, vec!["final words".to_string()]);
        assert_eq!(word_count, 2);
    }

    #[test]
    fn batch_rescue_requires_speech_health_miss_without_text() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let mut recording = ActiveRecording::new("test".into(), tx, None, None);
        assert!(!recording.should_batch_rescue_on_release());

        recording.first_signal_at =
            Some(tokio::time::Instant::now() - LIVE_STT_HEALTH_TIMEOUT - Duration::from_millis(1));
        assert!(recording.should_batch_rescue_on_release());

        recording.note_result_message(true);
        assert!(!recording.should_batch_rescue_on_release());
    }

    #[test]
    fn finalize_timeouts_stay_inside_fast_path_budget() {
        assert_eq!(LIVE_STT_HEALTH_TIMEOUT, Duration::from_millis(700));
        assert_eq!(FINALIZE_NO_RESULT_TIMEOUT, Duration::from_millis(900));
        assert_eq!(FINALIZE_QUIET_TIMEOUT, Duration::from_millis(300));
        assert_eq!(
            FINALIZE_WITH_TEXT_RESULT_TIMEOUT,
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn session_backoff_caps_at_eight_seconds() {
        let session = |failures| {
            let s = DeepgramSession {
                state: SessionState::Disconnected,
                cmd_rx: tokio::sync::mpsc::channel(1).1,
                deepgram_key: String::new(),
                bias: BiasPackage::default(),
                consecutive_failures: failures,
            };
            s.backoff_duration()
        };
        assert_eq!(session(1), Duration::from_millis(500));
        assert_eq!(session(2), Duration::from_millis(1000));
        assert_eq!(session(3), Duration::from_millis(2000));
        assert_eq!(session(4), Duration::from_millis(4000));
        assert_eq!(session(5), Duration::from_millis(8000));
        assert_eq!(session(9), Duration::from_millis(8000));
    }
}
