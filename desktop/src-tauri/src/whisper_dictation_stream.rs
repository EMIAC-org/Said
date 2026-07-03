//! Crash-recovery PCM tap for the on-device whisper.cpp dictation path.
//!
//! Consumes recorder PCM chunks while Caps Lock is held, resamples to 16 kHz,
//! and appends them to the crash-recovery session so a crash mid-dictation
//! doesn't lose the user's words.
//!
//! Transcription is BATCH ONLY: the on-device whisper.cpp pass runs once, on
//! release. This bridge used to also re-transcribe the growing buffer every 2 s
//! to emit rolling "live" partials to the status bar — pure redundant compute
//! (the real transcript comes from the on-release batch pass), so that was
//! removed. Only the recovery tap remains.

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
pub fn spawn_dictation_recovery_drain(recording_id: String, chunk_recv: ChunkReceiver) {
    thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx = chunk_recv.rx;
        let mut chunks = 0usize;

        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            let pcm_i16 = f32_chunk_to_i16(&resampled);
            let pcm_bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            crate::recovery::append_pcm(&pcm_bytes);
            chunks += 1;
        }

        tracing::debug!("[whisper_live] recovery drain finished id={recording_id} chunks={chunks}");
    });
}
