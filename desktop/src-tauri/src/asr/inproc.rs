//! In-process ASR engine — the always-available safety path.
//!
//! Runs a warm [`asr_core::WhisperEngine`] on a dedicated thread (whisper.cpp
//! contexts are single-threaded), fed over an mpsc channel. On Windows/Linux
//! this is the **CPU** engine — it never touches Vulkan and never aborts the
//! process, so it is the failover target when the GPU worker dies. On macOS it
//! is the **Metal** engine (reliable in-process GPU; no worker needed there).
//!
//! It idle-unloads the model after a timeout to release memory, exactly like the
//! previous local ASR worker.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use asr_core::{Device, DictationLocalAsrConfig, LocalAsrOutput, WhisperEngine};

use super::model_label;

const DEFAULT_IDLE_UNLOAD_SECS: u64 = 600;

/// Handle to the in-process engine thread.
pub struct InProcEngine {
    tx: mpsc::Sender<Job>,
}

enum Job {
    Prewarm {
        cfg: DictationLocalAsrConfig,
    },
    Transcribe {
        pcm: Vec<f32>,
        cfg: DictationLocalAsrConfig,
        reply: mpsc::Sender<Result<LocalAsrOutput, String>>,
    },
}

impl InProcEngine {
    /// Start the engine thread. CPU on Windows/Linux, Metal (`Gpu(0)`) on macOS.
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        let device = Self::device();
        if let Err(e) = thread::Builder::new()
            .name("airnote-inproc-asr".to_string())
            .spawn(move || run(rx, device))
        {
            tracing::error!(error = %e, "[asr:inproc] failed to start engine thread");
        }
        Self { tx }
    }

    fn device() -> Device {
        if cfg!(target_os = "macos") {
            Device::Gpu(0) // Metal
        } else {
            Device::Cpu
        }
    }

    /// Human label for logs/telemetry.
    pub fn kind(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "metal"
        } else {
            "cpu"
        }
    }

    /// Load the model warm without transcribing (best effort).
    pub fn prewarm(&self, cfg: DictationLocalAsrConfig) {
        let _ = self.tx.send(Job::Prewarm { cfg });
    }

    /// Transcribe prepared 16 kHz PCM. Blocks until the engine thread replies.
    pub fn transcribe(
        &self,
        pcm: Vec<f32>,
        cfg: &DictationLocalAsrConfig,
    ) -> Result<LocalAsrOutput, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Job::Transcribe {
                pcm,
                cfg: cfg.clone(),
                reply,
            })
            .map_err(|e| format!("in-process ASR engine is unavailable: {e}"))?;
        rx.recv()
            .map_err(|e| format!("in-process ASR engine stopped: {e}"))?
    }
}

fn run(rx: mpsc::Receiver<Job>, device: Device) {
    let mut engine = WhisperEngine::new(device);
    let idle = idle_timeout();

    loop {
        let job = match idle {
            Some(timeout) => match rx.recv_timeout(timeout) {
                Ok(job) => job,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if engine.is_loaded() {
                        engine.unload();
                        tracing::info!(
                            idle_secs = timeout.as_secs(),
                            "[asr:inproc] unloaded warm model after idle timeout"
                        );
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            },
        };

        match job {
            Job::Prewarm { cfg } => {
                if let Err(e) = engine.ensure_loaded(&cfg) {
                    tracing::warn!(error = %e, "[asr:inproc] prewarm failed");
                }
            }
            Job::Transcribe { pcm, cfg, reply } => {
                let started = Instant::now();
                let result = engine.transcribe(&pcm, &cfg).map(|t| LocalAsrOutput {
                    transcript: t.text,
                    model: model_label(&cfg.model),
                    language: cfg.language.clone(),
                    total_ms: started.elapsed().as_millis() as u64,
                    load_ms: t.load_ms,
                    inference_ms: t.inference_ms,
                    queue_wait_ms: 0,
                });
                let _ = reply.send(result.map_err(|e| e.to_string()));
            }
        }
    }
}

fn idle_timeout() -> Option<Duration> {
    let secs = std::env::var("AIRNOTE_DICTATION_ASR_IDLE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_UNLOAD_SECS);
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs.max(30)))
    }
}
