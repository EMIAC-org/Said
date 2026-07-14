//! Client for the isolated GPU worker process (`airnote-asr-gpu`).
//!
//! Owns the child process and talks the [`asr_core::ipc`] protocol over its
//! stdin/stdout via two dedicated threads (writer + reader) bridged to channels.
//! That decoupling lets every call use `recv_timeout`, so a hung driver or a
//! silently-wedged worker surfaces as [`WorkerError::Dead`] within a bound
//! instead of freezing the app. A closed pipe (crash / `GGML_ABORT`) shows up as
//! a disconnected channel — also `Dead`. The router reacts by quarantining the
//! GPU and failing the clip over to the in-process CPU engine.

use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use asr_core::ipc::{self, DeviceInfo, FromWorker, ToWorker};
use asr_core::{DictationLocalAsrConfig, LocalAsrOutput};

/// Worker binary name (target-triple suffix is stripped when staged next to the app).
const WORKER_BIN: &str = if cfg!(windows) {
    "airnote-asr-gpu.exe"
} else {
    "airnote-asr-gpu"
};

/// Max wait for the startup handshake (probe + runtime init). Generous so a
/// slow first Vulkan init isn't mistaken for a hang; short enough that a truly
/// wedged driver doesn't stall launch.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// Max wait for a transcription reply on a warm model.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Why a worker call didn't produce a transcript.
pub enum WorkerError {
    /// Worker is unusable (crashed, timed out, protocol broke). Quarantine + CPU.
    Dead(String),
    /// Worker ran but returned a transcription error (e.g. no speech). This is a
    /// real result — return it; don't burn the CPU re-running the same audio.
    Transcription(String),
}

pub struct WorkerClient {
    child: Child,
    to_worker: mpsc::Sender<ToWorker>,
    from_worker: mpsc::Receiver<FromWorker>,
    device: DeviceInfo,
    next_id: u64,
}

impl WorkerClient {
    /// Spawn the sidecar and complete the handshake. `Err` means "no usable GPU
    /// worker" — the caller runs CPU-only and does not respawn this session.
    pub fn spawn() -> Result<Self, String> {
        let bin = find_worker_binary()?;
        let mut command = Command::new(&bin);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()); // worker logs join the app's stderr (dev terminal / log file)

        // The worker is a console-subsystem binary; without CREATE_NO_WINDOW the
        // windowed (release) app pops a visible console window for it on every
        // launch. Mirrors backend.rs. macOS has no analog.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("failed to spawn ASR GPU worker {bin:?}: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "worker stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "worker stdout unavailable".to_string())?;

        // Writer thread: drains ToWorker messages to the child.
        let (to_worker, wr_rx) = mpsc::channel::<ToWorker>();
        thread::Builder::new()
            .name("asr-worker-writer".into())
            .spawn(move || {
                let mut w = BufWriter::new(stdin);
                while let Ok(msg) = wr_rx.recv() {
                    if ipc::write_message(&mut w, &msg).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("failed to start worker writer thread: {e}"))?;

        // Reader thread: forwards every FromWorker to a channel; exits (closing
        // the channel) when the worker closes stdout — the death signal.
        let (rd_tx, from_worker) = mpsc::channel::<FromWorker>();
        thread::Builder::new()
            .name("asr-worker-reader".into())
            .spawn(move || {
                let mut r = BufReader::new(stdout);
                loop {
                    match ipc::read_message::<_, FromWorker>(&mut r) {
                        Ok(msg) => {
                            if rd_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Err(_) => break, // EOF / crash / protocol error
                    }
                }
            })
            .map_err(|e| format!("failed to start worker reader thread: {e}"))?;

        // Handshake.
        match from_worker.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(FromWorker::Ready { device }) => {
                tracing::info!(
                    name = %device.name,
                    index = device.index,
                    vram_mb = device.vram_mb,
                    discrete = device.discrete,
                    "[asr:worker] GPU worker ready"
                );
                Ok(Self {
                    child,
                    to_worker,
                    from_worker,
                    device,
                    next_id: 1,
                })
            }
            Ok(FromWorker::NoGpu { reason }) => {
                let _ = child.wait();
                Err(format!("no usable GPU: {reason}"))
            }
            Ok(other) => {
                let _ = child.kill();
                Err(format!("unexpected worker handshake: {other:?}"))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                Err("worker handshake timed out (driver hang?)".to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.wait();
                Err("worker exited before handshake".to_string())
            }
        }
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.device
    }

    /// Best-effort warm load; blocks briefly for the ack so the channel stays clean.
    pub fn prewarm(&mut self, cfg: &DictationLocalAsrConfig) {
        if self
            .to_worker
            .send(ToWorker::Prewarm { cfg: cfg.clone() })
            .is_err()
        {
            return;
        }
        match self.from_worker.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(FromWorker::Prewarmed { load_ms }) => {
                tracing::info!(load_ms, "[asr:worker] GPU model warm")
            }
            Ok(other) => tracing::debug!(?other, "[asr:worker] unexpected prewarm reply"),
            Err(e) => tracing::warn!(error = %e, "[asr:worker] prewarm ack not received"),
        }
    }

    /// Transcribe on the GPU worker. Requests are serialized by the caller
    /// (router holds the worker under a lock), so exactly one reply is in flight.
    pub fn transcribe(
        &mut self,
        pcm: &[f32],
        cfg: &DictationLocalAsrConfig,
    ) -> Result<LocalAsrOutput, WorkerError> {
        let id = self.next_id;
        self.next_id += 1;

        self.to_worker
            .send(ToWorker::Transcribe {
                id,
                pcm: pcm.to_vec(),
                cfg: cfg.clone(),
            })
            .map_err(|_| WorkerError::Dead("worker writer thread gone".to_string()))?;

        loop {
            match self.from_worker.recv_timeout(REQUEST_TIMEOUT) {
                Ok(FromWorker::Done { id: rid, output }) if rid == id => return Ok(output),
                Ok(FromWorker::Failed { id: rid, error }) if rid == id => {
                    return Err(WorkerError::Transcription(error.to_string()));
                }
                // Stray late acks (e.g. a prewarm) — ignore and keep waiting.
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(WorkerError::Dead("worker transcription timed out".to_string()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(WorkerError::Dead("worker exited mid-request".to_string()));
                }
            }
        }
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        let _ = self.to_worker.send(ToWorker::Shutdown);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Locate `airnote-asr-gpu` — sibling of the app exe (staged install), else the
/// nearest `target/{debug,release}` (dev). Mirrors the backend sidecar lookup.
fn find_worker_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot get exe path: {e}"))?;
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(dir) = exe.parent() {
        candidates.push(dir.join(WORKER_BIN));
    }
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        let Some(d) = dir else { break };
        candidates.push(d.join("debug").join(WORKER_BIN));
        candidates.push(d.join("release").join(WORKER_BIN));
        candidates.push(d.join(WORKER_BIN));
        dir = d.parent().map(|p| p.to_path_buf());
    }

    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| format!("{WORKER_BIN} not found next to the app or in a target dir"))
}
