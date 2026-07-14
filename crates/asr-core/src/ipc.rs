//! Wire protocol between the desktop app's supervisor and the isolated GPU
//! worker process.
//!
//! Framing is a little-endian `u32` length prefix followed by a bincode-encoded
//! message. Payloads are small (a few hundred KB of PCM at most), so this runs
//! over the worker's stdin/stdout pipes with no shared memory.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::DictationLocalAsrConfig;
use crate::error::AsrError;
use crate::output::LocalAsrOutput;

/// Hard cap on a single frame (256 MiB) so a corrupt length can't trigger a
/// huge allocation. Real frames are ~0.5 MiB.
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

/// Which GPU family a [`DeviceInfo`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum BackendKind {
    Cpu,
    Metal,
    Vulkan,
}

/// Human- and log-friendly description of the device the worker selected.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub backend: BackendKind,
    /// ggml device index the worker will pass to whisper (`gpu_device`).
    pub index: i32,
    pub name: String,
    pub vram_mb: u64,
    /// True for a discrete GPU, false for an integrated one.
    pub discrete: bool,
}

/// App → worker.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum ToWorker {
    /// Load the model warm without transcribing.
    Prewarm { cfg: DictationLocalAsrConfig },
    /// Transcribe prepared 16 kHz mono PCM. `id` correlates the reply.
    Transcribe {
        id: u64,
        pcm: Vec<f32>,
        cfg: DictationLocalAsrConfig,
    },
    /// Ask the worker to exit cleanly.
    Shutdown,
}

/// Worker → app.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum FromWorker {
    /// Sent once at startup after a device is selected and the runtime is live.
    Ready { device: DeviceInfo },
    /// Worker found no usable GPU (or the loader is absent). The app should run
    /// CPU-only and not respawn this session.
    NoGpu { reason: String },
    /// Prewarm finished.
    Prewarmed { load_ms: u64 },
    /// A transcription completed.
    Done { id: u64, output: LocalAsrOutput },
    /// A transcription failed (recoverable — the app retries on CPU).
    Failed { id: u64, error: AsrError },
}

/// Write one length-prefixed message and flush.
pub fn write_message<W: Write, M: Serialize>(w: &mut W, msg: &M) -> io::Result<()> {
    let bytes = bincode::serialize(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("encode: {e}")))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read one length-prefixed message. Returns `UnexpectedEof` when the peer
/// closes the pipe (the supervisor treats that as "worker exited").
pub fn read_message<R: Read, M: DeserializeOwned>(r: &mut R) -> io::Result<M> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    bincode::deserialize(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode: {e}")))
}
