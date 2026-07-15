//! Crash-recovery PCM tap for dictation, with an optional live Nemotron sink.
//!
//! Consumes recorder PCM chunks while Caps Lock is held, resamples to 16 kHz,
//! and appends them to the crash-recovery session so a crash mid-dictation
//! doesn't lose the user's words.
//!
//! On-device transcription remains batch-only and runs once on release. When
//! the user explicitly selects Together Nemotron, the same PCM is also sent to
//! its already-open WebSocket session as it arrives. This is a fan-out at the
//! audio boundary, never a second recorder or a second microphone stream.

use std::sync::mpsc;
use std::thread;

use said_recorder::{ChunkReceiver, resample_to_16k};

fn f32_chunk_to_i16(chunk_f32: &[f32]) -> Vec<i16> {
    chunk_f32
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
        .collect()
}

/// Drain recorder PCM chunks and persist them (16 kHz mono i16 LE) to the
/// crash-recovery session. No transcription — the on-device whisper.cpp pass
/// runs once, in batch, on Caps Lock release.
pub fn spawn_dictation_audio_drain(
    recording_id: String,
    chunk_recv: ChunkReceiver,
    live_nemotron: Option<asr_cloud::LiveTranscriptionController>,
) -> mpsc::Receiver<()> {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx = chunk_recv.rx;
        let mut chunks = 0usize;
        let mut live_nemotron = live_nemotron;

        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            let pcm_i16 = f32_chunk_to_i16(&resampled);
            let pcm_bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            crate::recovery::append_pcm(&pcm_bytes);
            if let Some(controller) = &live_nemotron {
                if let Err(error) = controller.append_pcm16le_blocking(pcm_bytes) {
                    // The session task already holds the terminal provider
                    // error. Stop feeding it, but keep draining recovery so a
                    // crash during this recording still leaves usable audio.
                    tracing::warn!(
                        "[nemotron_live] audio bridge stopped id={recording_id}: {error}"
                    );
                    live_nemotron = None;
                }
            }
            chunks += 1;
        }

        tracing::debug!("[dictation_audio] drain finished id={recording_id} chunks={chunks}");
        let _ = done_tx.send(());
    });
    done_rx
}
