//! Dictation PCM router for crash recovery and live local transcription.
//!
//! Consumes recorder PCM chunks while Caps Lock is held, resamples to 16 kHz,
//! and appends them to the crash-recovery session so a crash mid-dictation
//! doesn't lose the user's words.
//!
//! Catalog models may consume the same 16 kHz frames for live HUD text. The
//! complete recovery audio remains authoritative for batch fallback.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use said_recorder::{ChunkReceiver, resample_to_16k};
use tauri::AppHandle;

static ACTIVE_DRAINS: OnceLock<(Mutex<HashSet<String>>, Condvar)> = OnceLock::new();

fn active_drains() -> &'static (Mutex<HashSet<String>>, Condvar) {
    ACTIVE_DRAINS.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

pub fn wait_for_drain(recording_id: &str, timeout: Duration) {
    let (active, changed) = active_drains();
    let Ok(guard) = active.lock() else {
        return;
    };
    if guard.contains(recording_id) {
        let _ = changed.wait_timeout_while(guard, timeout, |ids| ids.contains(recording_id));
    }
}

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

/// Drain recorder PCM chunks, persist recovery audio, and route frames to a
/// streaming-capable selected catalog model. Finalization and batch fallback
/// remain owned by the normal dictation STT call after key release.
pub fn spawn_dictation_audio_drain(
    app: AppHandle,
    recording_id: String,
    chunk_recv: ChunkReceiver,
) {
    if let Ok(mut active) = active_drains().0.lock() {
        active.insert(recording_id.clone());
    }
    let stream_local = said_core::prefs::load().dictation_stt == crate::stt_policy::LOCAL_PREF
        && crate::local_transcribe::selected_supports_streaming();
    if stream_local {
        crate::local_transcribe::start_stream(app, recording_id.clone());
    }
    thread::spawn(move || {
        let native_rate = chunk_recv.native_rate;
        let sync_rx = chunk_recv.rx;
        let mut chunks = 0usize;

        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            crate::recovery::append_pcm(&f32_chunk_to_pcm_bytes(&resampled));
            if stream_local {
                crate::local_transcribe::feed_stream(&recording_id, resampled);
            }
            chunks += 1;
        }

        tracing::debug!("[dictation_audio] drain finished id={recording_id} chunks={chunks}");
        if let Ok(mut active) = active_drains().0.lock() {
            active.remove(&recording_id);
            active_drains().1.notify_all();
        }
    });
}
