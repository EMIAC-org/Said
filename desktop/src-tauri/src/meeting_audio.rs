//! Meeting-mode audio pipeline: always-on microphone with hold-to-mute.
//!
//! Completely independent of the dictation `DeepgramSession` actor — owns its
//! own recorder, Deepgram WebSocket, and audio bridge thread.  Emits
//! `meeting-transcript` Tauri events with `{ text, timestamp_ms }` payloads
//! for every `is_final` result from Deepgram.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deepgram::{
    Deepgram,
    common::options::{Encoding, Language, Model, Options},
};
use said_recorder::{self, AudioRecorder, SAMPLE_RATE, resample_to_16k};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Keepalive interval — Deepgram closes idle sockets after ~12s of silence.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);
/// Deepgram's streaming limit.  We reconnect before hitting it.
const MAX_STREAMING_DURATION: Duration = Duration::from_secs(42);
/// Timeout for the initial WS connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout for individual send operations.
const WS_SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Payload emitted to the frontend for every final transcript segment.
#[derive(Clone, serde::Serialize)]
struct MeetingTranscriptPayload {
    text: String,
    timestamp_ms: u64,
}

/// Handle to a running meeting audio pipeline.
///
/// Created by [`start`]; the caller stores this in managed Tauri state.
/// Dropping or calling [`stop`] tears everything down.
pub struct MeetingAudio {
    cancel: CancellationToken,
    muted: Arc<AtomicBool>,
}

impl MeetingAudio {
    /// Start the always-on meeting audio pipeline.
    ///
    /// Spawns:
    ///  - A recorder thread (via `said_recorder`)
    ///  - An audio bridge std-thread that resamples and forwards PCM
    ///  - A tokio task that owns the Deepgram WS, emits events, and auto-reconnects
    pub fn start(app: AppHandle, deepgram_key: String) -> Result<Self, String> {
        let cancel = CancellationToken::new();
        let muted = Arc::new(AtomicBool::new(false));

        // Start the microphone recorder.
        let mut recorder = AudioRecorder::new();
        recorder.start()?;

        let chunk_rx = recorder
            .take_chunk_receiver()
            .ok_or("failed to take chunk receiver from recorder")?;

        // Channel from the bridge thread → the tokio task.
        let (pcm_tx, pcm_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4096);

        // ── Audio bridge thread ─────────────────────────────────────────────
        // Reads f32 chunks from cpal, resamples to 16 kHz i16-LE, respects mute.
        let bridge_cancel = cancel.clone();
        let bridge_muted = Arc::clone(&muted);
        let native_rate = chunk_rx.native_rate;
        std::thread::spawn(move || {
            while let Ok(chunk_f32) = chunk_rx.rx.recv() {
                if bridge_cancel.is_cancelled() {
                    break;
                }
                // When muted we still drain the recorder so cpal doesn't back up,
                // but we don't forward audio to Deepgram.
                if bridge_muted.load(Ordering::Relaxed) {
                    continue;
                }
                let resampled = resample_to_16k(&chunk_f32, native_rate);
                let pcm: Vec<u8> = resampled
                    .iter()
                    .flat_map(|&s| {
                        let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                        i16_val.to_le_bytes()
                    })
                    .collect();
                if pcm_tx.blocking_send(pcm).is_err() {
                    break;
                }
            }
            info!("[meeting_audio] bridge thread exiting");
            // Dropping `recorder` here is fine — its handle goes out of scope.
        });

        // ── Tokio task: Deepgram WS + event emitter ─────────────────────────
        let task_cancel = cancel.clone();
        let task_muted = Arc::clone(&muted);
        tauri::async_runtime::spawn(async move {
            run_meeting_loop(app, deepgram_key, pcm_rx, task_cancel, task_muted, recorder).await;
        });

        Ok(Self { cancel, muted })
    }

    /// Stop the pipeline — cancels all background work.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Toggle mute state.  Returns the new value (`true` = muted).
    pub fn toggle_mute(&self) -> bool {
        let was_muted = self.muted.load(Ordering::Relaxed);
        let new_muted = !was_muted;
        self.muted.store(new_muted, Ordering::Relaxed);
        new_muted
    }
}

// ── Internal: main async loop with auto-reconnect ───────────────────────────

async fn run_meeting_loop(
    app: AppHandle,
    deepgram_key: String,
    mut pcm_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
    muted: Arc<AtomicBool>,
    mut recorder: AudioRecorder,
) {
    let mut consecutive_failures: u32 = 0;
    let start_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Connect to Deepgram
        let ws = match connect_meeting_ws(&deepgram_key).await {
            Some(ws) => {
                consecutive_failures = 0;
                ws
            }
            None => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let backoff = backoff_duration(consecutive_failures);
                warn!(
                    "[meeting_audio] connect failed ({consecutive_failures}), backing off {backoff:?}"
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => continue,
                    _ = cancel.cancelled() => break,
                }
            }
        };

        info!("[meeting_audio] Deepgram WS connected, streaming");

        // Stream until the WS closes, times out, or we're cancelled.
        let ws_closed = stream_to_ws(
            &app,
            ws,
            &mut pcm_rx,
            &cancel,
            &muted,
            start_epoch,
        )
        .await;

        if cancel.is_cancelled() || ws_closed == WsCloseReason::Cancelled {
            break;
        }

        // Brief pause before reconnecting to avoid tight-loop on server errors.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {},
            _ = cancel.cancelled() => break,
        }
    }

    // Clean up the recorder on the way out.
    info!("[meeting_audio] stopping recorder");
    let _ = recorder.initiate_stop();
    info!("[meeting_audio] pipeline stopped");
}

#[derive(Debug, PartialEq)]
enum WsCloseReason {
    Cancelled,
    TimedOut,
    Error,
}

/// Streams audio from `pcm_rx` to the Deepgram WebSocket and emits transcript
/// events.  Returns the reason the loop exited.
async fn stream_to_ws(
    app: &AppHandle,
    mut ws: deepgram::listen::websocket::WebsocketHandle,
    pcm_rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    cancel: &CancellationToken,
    muted: &Arc<AtomicBool>,
    start_epoch_ms: u64,
) -> WsCloseReason {
    let session_start = tokio::time::Instant::now();
    let deadline = session_start + MAX_STREAMING_DURATION;
    let mut keepalive_interval = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive_interval.reset();

    loop {
        tokio::select! {
            // ── Cancellation ────────────────────────────────────────────
            _ = cancel.cancelled() => {
                let _ = close_ws(&mut ws).await;
                return WsCloseReason::Cancelled;
            }

            // ── Streaming time limit ────────────────────────────────────
            _ = tokio::time::sleep_until(deadline) => {
                info!("[meeting_audio] streaming time limit reached, reconnecting");
                let _ = close_ws(&mut ws).await;
                return WsCloseReason::TimedOut;
            }

            // ── Audio from bridge ───────────────────────────────────────
            Some(pcm) = pcm_rx.recv() => {
                if let Err(e) = send_audio(&mut ws, pcm).await {
                    warn!("[meeting_audio] send_audio failed: {e}");
                    return WsCloseReason::Error;
                }
                // Drain any available responses
                if !drain_responses(app, &mut ws, start_epoch_ms).await {
                    return WsCloseReason::Error;
                }
            }

            // ── Keepalive tick ──────────────────────────────────────────
            _ = keepalive_interval.tick() => {
                // When muted, send keepalives to keep the socket open
                if muted.load(Ordering::Relaxed) {
                    if let Err(e) = send_keepalive(&mut ws).await {
                        warn!("[meeting_audio] keepalive failed: {e}");
                        return WsCloseReason::Error;
                    }
                }
                // Also drain any pending responses
                if !drain_responses(app, &mut ws, start_epoch_ms).await {
                    return WsCloseReason::Error;
                }
            }
        }
    }
}

/// Drain all immediately-available responses from the WS.
/// Returns `false` if the WS closed or errored (caller should reconnect).
async fn drain_responses(
    app: &AppHandle,
    ws: &mut deepgram::listen::websocket::WebsocketHandle,
    start_epoch_ms: u64,
) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_millis(1), ws.receive()).await {
            Ok(Some(Ok(response))) => {
                let Ok(v) = serde_json::to_value(response) else {
                    continue;
                };
                // We only care about is_final Results.
                if v["type"].as_str().unwrap_or("") != "Results" {
                    continue;
                }
                if !v["is_final"].as_bool().unwrap_or(false) {
                    continue;
                }
                // Extract transcript text from channel.alternatives[0].transcript
                let transcript = v["channel"]["alternatives"]
                    .as_array()
                    .and_then(|alts| alts.first())
                    .and_then(|alt| alt["transcript"].as_str())
                    .unwrap_or("")
                    .trim();
                if transcript.is_empty() {
                    continue;
                }
                // Compute a rough timestamp from DG's start field or wall clock
                let dg_start_s = v["start"].as_f64().unwrap_or(0.0);
                let timestamp_ms = if dg_start_s > 0.0 {
                    start_epoch_ms + (dg_start_s * 1000.0) as u64
                } else {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64
                };

                let payload = MeetingTranscriptPayload {
                    text: transcript.to_string(),
                    timestamp_ms,
                };
                info!(
                    "[meeting_audio] final transcript: {:?} ({}ms)",
                    payload.text, payload.timestamp_ms
                );
                let _ = app.emit("meeting-transcript", payload);
            }
            Ok(Some(Err(e))) => {
                warn!("[meeting_audio] WS receive error: {e}");
                return false;
            }
            Ok(None) => {
                warn!("[meeting_audio] WS closed by server");
                return false;
            }
            Err(_) => {
                // No more messages available right now.
                return true;
            }
        }
    }
}

// ── Deepgram WS helpers ─────────────────────────────────────────────────────

async fn connect_meeting_ws(
    deepgram_key: &str,
) -> Option<deepgram::listen::websocket::WebsocketHandle> {
    if deepgram_key.is_empty() {
        return None;
    }
    let client = match Deepgram::new(deepgram_key) {
        Ok(c) => c,
        Err(e) => {
            warn!("[meeting_audio] Deepgram client init failed: {e}");
            return None;
        }
    };

    let options = Options::builder()
        .model(Model::Nova3)
        .language(Language::en)
        .smart_format(true)
        .build();

    let transcription = client.transcription();
    let builder = transcription
        .stream_request_with_options(options)
        .encoding(Encoding::Linear16)
        .sample_rate(SAMPLE_RATE)
        .channels(1)
        .interim_results(false)
        .utterance_end_ms(1500)
        .keep_alive();

    let start = tokio::time::Instant::now();
    match tokio::time::timeout(CONNECT_TIMEOUT, builder.handle()).await {
        Err(_) => {
            warn!(
                "[meeting_audio] WS connect timed out after {}ms",
                start.elapsed().as_millis()
            );
            None
        }
        Ok(Err(e)) => {
            warn!(
                "[meeting_audio] WS connect failed after {}ms: {e}",
                start.elapsed().as_millis()
            );
            None
        }
        Ok(Ok(ws)) => {
            info!(
                "[meeting_audio] WS connected in {}ms",
                start.elapsed().as_millis()
            );
            Some(ws)
        }
    }
}

async fn send_audio(
    ws: &mut deepgram::listen::websocket::WebsocketHandle,
    pcm: Vec<u8>,
) -> Result<(), String> {
    match tokio::time::timeout(WS_SEND_TIMEOUT, ws.send_data(pcm)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("audio send failed: {e}")),
        Err(_) => Err("audio send timed out".into()),
    }
}

async fn send_keepalive(
    ws: &mut deepgram::listen::websocket::WebsocketHandle,
) -> Result<(), String> {
    match tokio::time::timeout(WS_SEND_TIMEOUT, ws.keep_alive()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("keepalive send failed: {e}")),
        Err(_) => Err("keepalive send timed out".into()),
    }
}

async fn close_ws(
    ws: &mut deepgram::listen::websocket::WebsocketHandle,
) -> Result<(), String> {
    match tokio::time::timeout(WS_SEND_TIMEOUT, ws.close_stream()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("close_stream failed: {e}")),
        Err(_) => Err("close_stream timed out".into()),
    }
}

fn backoff_duration(consecutive_failures: u32) -> Duration {
    let exp = consecutive_failures.saturating_sub(1).min(4);
    Duration::from_millis(500 * 2_u64.saturating_pow(exp))
        .min(Duration::from_secs(8))
}
