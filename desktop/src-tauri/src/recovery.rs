//! Crash-safe dictation recovery.
//!
//! While a (non-meeting) recording is in flight we stream the same 16 kHz mono
//! i16 PCM that goes to Deepgram into an on-disk file. If the app dies before the
//! recording has been delivered (panic → SIGABRT, OS kill, power loss), the audio
//! survives on disk. On the next launch [`take_orphan`] returns it so the normal
//! pipeline can re-transcribe and hand the user their words back.
//!
//! A clean finish (success, error, cancel, too-short) always calls [`clear`], so a
//! leftover file unambiguously means "the app died mid-dictation". The capture is a
//! single append-only write per audio chunk — no busy loops, no growth beyond the
//! current utterance, and a no-op atomic check when no recording is active.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sample rate of the PCM the audio bridge feeds us (post-resample).
const SAMPLE_RATE: u32 = 16_000;
/// Bytes per second of 16 kHz mono i16 audio.
const BYTES_PER_SEC: u64 = SAMPLE_RATE as u64 * 2;
/// Ignore orphans shorter than ~1 s — not worth interrupting the user to recover.
const MIN_RECOVERABLE_BYTES: u64 = BYTES_PER_SEC;

/// Cheap gate so `append_pcm` is a single atomic load when nothing is recording.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

struct Session {
    file: File,
    bytes: u64,
}

fn dir() -> PathBuf {
    said_core::paths::data_dir().join("recovery")
}

fn pcm_path() -> PathBuf {
    dir().join("in_progress.pcm")
}

fn meta_path() -> PathBuf {
    dir().join("in_progress.json")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Begin capturing a new recording. Truncates any previous in-progress file so a
/// stale (but uncrashed) capture can never masquerade as an orphan.
pub fn begin() {
    let dir = dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        tracing::warn!("[recovery] cannot create recovery dir: {e}");
        return;
    }
    let file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(pcm_path())
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[recovery] cannot open recovery file: {e}");
            return;
        }
    };
    let meta = serde_json::json!({ "started_at": now_ms(), "rate": SAMPLE_RATE });
    let _ = fs::write(meta_path(), meta.to_string());
    if let Ok(mut guard) = SESSION.lock() {
        *guard = Some(Session { file, bytes: 0 });
        ACTIVE.store(true, Ordering::SeqCst);
        tracing::info!("[recovery] dictation capture started");
    }
}

/// Append a chunk of 16 kHz mono i16 LE PCM. Cheap no-op when inactive.
pub fn append_pcm(pcm: &[u8]) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(mut guard) = SESSION.lock() {
        if let Some(session) = guard.as_mut() {
            if session.file.write_all(pcm).is_ok() {
                session.bytes += pcm.len() as u64;
            }
        }
    }
}

/// Stop capturing and delete the in-progress files. Idempotent — safe to call on
/// every terminal path even if no capture was active.
pub fn clear() {
    ACTIVE.store(false, Ordering::SeqCst);
    if let Ok(mut guard) = SESSION.lock() {
        *guard = None;
    }
    let _ = fs::remove_file(pcm_path());
    let _ = fs::remove_file(meta_path());
}

/// On launch: if a recoverable orphan exists, return it as a ready-to-send WAV and
/// delete the on-disk files. Returns `None` when there is nothing worth recovering.
pub fn take_orphan() -> Option<Vec<u8>> {
    // An actively-capturing session is never an orphan.
    if ACTIVE.load(Ordering::SeqCst) {
        return None;
    }
    let pcm = fs::read(pcm_path()).ok()?;
    // Consume the files regardless of outcome so we never re-offer the same audio.
    let _ = fs::remove_file(pcm_path());
    let _ = fs::remove_file(meta_path());
    if (pcm.len() as u64) < MIN_RECOVERABLE_BYTES {
        tracing::info!(
            "[recovery] orphan too short ({} bytes) — discarding",
            pcm.len()
        );
        return None;
    }
    let seconds = pcm.len() as u64 / BYTES_PER_SEC;
    tracing::warn!(
        "[recovery] orphan dictation found ({} bytes, ~{seconds}s) — re-transcribing",
        pcm.len()
    );
    Some(pcm16k_to_wav(&pcm))
}

/// Wrap raw 16 kHz mono i16 LE PCM in a minimal 44-byte WAV header.
fn pcm16k_to_wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = BYTES_PER_SEC as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// Test helper (chaos harness): plant a synthetic orphan of `seconds` of 16 kHz
/// silence so the next-launch recovery path can be exercised without a real
/// crash. The transcript will be empty (silence), so this verifies the recovery
/// *plumbing* (detection → re-transcribe attempt), not word fidelity.
pub fn plant_synthetic_orphan(seconds: u32) {
    let dir = dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let bytes = BYTES_PER_SEC as usize * seconds as usize;
    if fs::write(pcm_path(), vec![0u8; bytes]).is_err() {
        return;
    }
    let meta =
        serde_json::json!({ "started_at": now_ms(), "rate": SAMPLE_RATE, "synthetic": true });
    let _ = fs::write(meta_path(), meta.to_string());
    tracing::warn!("[recovery] planted synthetic {seconds}s orphan for chaos testing");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let pcm = vec![0u8; 3200]; // 0.1 s of 16 kHz mono i16
        let wav = pcm16k_to_wav(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        // RIFF chunk size = 36 + data_len
        assert_eq!(
            u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]),
            36 + 3200
        );
        // sample rate field
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            SAMPLE_RATE
        );
        // data chunk size = pcm length, and total length = 44 + pcm
        assert_eq!(
            u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]),
            3200
        );
        assert_eq!(wav.len(), 44 + 3200);
    }

    #[test]
    fn min_recoverable_is_one_second() {
        assert_eq!(MIN_RECOVERABLE_BYTES, 32_000);
    }
}
