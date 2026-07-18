//! Crash-recovery PCM tap for dictation.
//!
//! Consumes recorder PCM chunks while Caps Lock is held, resamples to 16 kHz,
//! and appends them to the crash-recovery session so a crash mid-dictation
//! doesn't lose the user's words.
//!
//! Transcription runs once on release. The tap stores raw resampled audio; the
//! batch path owns any conditioning, avoiding a double-denoise during recovery.

use std::thread;

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
pub fn spawn_dictation_audio_drain(recording_id: String, chunk_recv: ChunkReceiver) {
    thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx = chunk_recv.rx;
        let mut chunks = 0usize;

        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            crate::recovery::append_pcm(&f32_chunk_to_pcm_bytes(&resampled));
            chunks += 1;
        }

        tracing::debug!("[dictation_audio] drain finished id={recording_id} chunks={chunks}");
    });
}
