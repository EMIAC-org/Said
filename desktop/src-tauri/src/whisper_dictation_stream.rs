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
//!
//! The two branches carry **differently conditioned** audio, on purpose:
//!
//! * The live socket gets audio through [`said_core::preprocess::StreamConditioner`]
//!   (high-pass + RNNoise, state persisted across chunks). Nothing downstream
//!   conditions it — the bytes go straight to the provider — so this is the only
//!   place it can happen.
//! * The recovery tap gets the **raw** resampled audio. Its WAV is re-transcribed
//!   through the batch path on retry, and that path runs `condition_16k` itself;
//!   conditioning here too would denoise the same audio twice.

use std::sync::mpsc;
use std::thread;

use said_core::preprocess::StreamConditioner;
use said_recorder::{ChunkReceiver, resample_to_16k};

fn f32_chunk_to_i16(chunk_f32: &[f32]) -> Vec<i16> {
    chunk_f32
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
        .collect()
}

fn f32_chunk_to_pcm_bytes(chunk_f32: &[f32]) -> Vec<u8> {
    f32_chunk_to_i16(chunk_f32)
        .iter()
        .flat_map(|s| s.to_le_bytes())
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
        // Only built when there's a live socket to feed — RNNoise costs real
        // CPU and the recovery tap deliberately stores raw audio.
        let mut conditioner = live_nemotron.is_some().then(StreamConditioner::new);

        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            crate::recovery::append_pcm(&f32_chunk_to_pcm_bytes(&resampled));

            if let Some(controller) = &live_nemotron {
                let conditioned = conditioner
                    .as_mut()
                    .map(|c| c.push(&resampled))
                    .unwrap_or_else(|| resampled.clone());
                // A conditioner holds back a sub-block remainder; nothing to
                // send this round is normal, not an error.
                if !conditioned.is_empty()
                    && let Err(error) =
                        controller.append_pcm16le_blocking(f32_chunk_to_pcm_bytes(&conditioned))
                {
                    // The session task already holds the terminal provider
                    // error. Stop feeding it, but keep draining recovery so a
                    // crash during this recording still leaves usable audio.
                    tracing::warn!(
                        "[nemotron_live] audio bridge stopped id={recording_id}: {error}"
                    );
                    live_nemotron = None;
                    conditioner = None;
                }
            }
            chunks += 1;
        }

        // Flush the conditioner's tail (<10 ms) so the final word isn't clipped.
        if let (Some(controller), Some(c)) = (&live_nemotron, conditioner.as_mut()) {
            let tail = c.flush();
            if !tail.is_empty() {
                let _ = controller.append_pcm16le_blocking(f32_chunk_to_pcm_bytes(&tail));
            }
            // Once per recording — the only visible proof that the live socket
            // got conditioned audio, and mean_vad is the first thing to look at
            // when someone reports the cloud transcript sounding noisy.
            tracing::info!(
                rnnoise = c.denoise_enabled(),
                frames = c.frames(),
                mean_vad = c.mean_vad(),
                "[dictation_audio] live audio conditioned id={recording_id}"
            );
        }

        tracing::debug!("[dictation_audio] drain finished id={recording_id} chunks={chunks}");
        let _ = done_tx.send(());
    });
    done_rx
}
