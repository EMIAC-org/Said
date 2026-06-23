//! Live whisper.cpp partial transcripts during Caps Lock dictation (Turbo Q5).
//!
//! Re-transcribes the growing PCM buffer every few seconds and emits rolling
//! partial text to the status bar (`voice-status` phase `live_stt`), matching
//! the Swift local STT UX.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use said_recorder::{ChunkReceiver, resample_to_16k};
use tokio::sync::mpsc::UnboundedSender;

use crate::meeting_engine;

const MIN_SAMPLES: usize = 16_000 * 3 / 2; // 1.5 s at 16 kHz
const STEP_INTERVAL: Duration = Duration::from_secs(2);

fn f32_chunk_to_i16(chunk_f32: &[f32]) -> Vec<i16> {
    chunk_f32
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
        .collect()
}

fn maybe_emit_live_partial(
    samples: &[i16],
    language: &str,
    live_partial_tx: &UnboundedSender<String>,
    recording_id: &str,
) {
    if samples.len() < MIN_SAMPLES {
        return;
    }
    match meeting_engine::transcribe_dictation_pcm_i16(samples, language) {
        Ok(text) => {
            let text = text.trim().to_string();
            if !text.is_empty() {
                let _ = live_partial_tx.send(text);
            }
        }
        Err(e) => {
            tracing::warn!(
                "[whisper_live] partial failed id={recording_id} samples={}: {e}",
                samples.len()
            );
        }
    }
}

/// Consume recorder PCM chunks, run periodic whisper.cpp passes, and emit
/// rolling partial transcripts while Caps Lock is held.
pub fn spawn_live_whisper_bridge(
    recording_id: String,
    chunk_recv: ChunkReceiver,
    language: String,
    live_partial_tx: UnboundedSender<String>,
) {
    thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx: mpsc::Receiver<Vec<f32>> = chunk_recv.rx;
        let mut samples: Vec<i16> = Vec::new();
        let mut last_transcribe = Instant::now() - STEP_INTERVAL;

        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            let pcm_i16 = f32_chunk_to_i16(&resampled);
            let pcm_bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            crate::recovery::append_pcm(&pcm_bytes);
            samples.extend_from_slice(&pcm_i16);

            if samples.len() < MIN_SAMPLES {
                continue;
            }
            if last_transcribe.elapsed() < STEP_INTERVAL {
                continue;
            }
            last_transcribe = Instant::now();
            maybe_emit_live_partial(&samples, &language, &live_partial_tx, &recording_id);
        }

        if samples.len() >= MIN_SAMPLES {
            maybe_emit_live_partial(&samples, &language, &live_partial_tx, &recording_id);
        }

        tracing::debug!(
            "[whisper_live] bridge finished id={recording_id} samples={}",
            samples.len()
        );
    });
}
