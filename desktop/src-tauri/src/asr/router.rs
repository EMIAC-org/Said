//! The ASR supervisor / policy brain.
//!
//! Owns the always-present in-process engine and, on Windows/Linux, an optional
//! isolated GPU worker. Policy:
//!
//! * **Primary = GPU worker** when a usable GPU exists; otherwise the in-process
//!   engine (CPU on Win/Linux, Metal on macOS) is primary.
//! * A worker **crash / hang** (`WorkerError::Dead`) fails the *current* clip
//!   over to the in-process engine instantly, and the worker is respawned on the
//!   next request. After [`MAX_GPU_DEATHS`] deaths the GPU is **quarantined** for
//!   the session (CPU only) — no thrash.
//! * A worker **transcription error** (no speech, etc.) is a real result and is
//!   returned as-is; the CPU is not burned re-running the same audio.
//!
//! Requests are serialized through the worker lock (one whisper context, one at
//! a time), which matches how dictation is actually used.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use asr_core::{DictationLocalAsrConfig, LocalAsrOutput};

use super::inproc::InProcEngine;
use super::worker_client::{WorkerClient, WorkerError};

/// Live-worker deaths tolerated (with respawn) before the GPU is quarantined.
const MAX_GPU_DEATHS: u32 = 2;

pub struct AsrRouter {
    inproc: InProcEngine,
    worker: Mutex<Option<WorkerClient>>,
    /// GPU path is off for the session (no GPU, missing worker binary, macOS, or
    /// too many crashes). When set, we never spawn/use a worker.
    gpu_disabled: AtomicBool,
    gpu_deaths: AtomicU32,
}

impl AsrRouter {
    /// Build the router and, on Win/Linux, attempt the GPU worker once.
    pub fn start() -> Self {
        if !asr_core::cpu_supports_local_asr() {
            tracing::error!(
                "[asr] CPU lacks AVX2 — on-device speech disabled (cloud STT still works)"
            );
        }
        let inproc = InProcEngine::start();

        let (worker, disabled) = if cfg!(target_os = "macos") {
            // macOS uses the reliable in-process Metal engine; no worker.
            (None, true)
        } else {
            match WorkerClient::spawn() {
                Ok(w) => {
                    tracing::info!(device = %w.device().name, "[asr] GPU worker active (primary)");
                    (Some(w), false)
                }
                Err(e) => {
                    tracing::info!(
                        reason = %e,
                        engine = inproc.kind(),
                        "[asr] no GPU worker; in-process engine is primary"
                    );
                    (None, true)
                }
            }
        };

        Self {
            inproc,
            worker: Mutex::new(worker),
            gpu_disabled: AtomicBool::new(disabled),
            gpu_deaths: AtomicU32::new(0),
        }
    }

    /// True while the isolated GPU worker is this session's primary engine
    /// (spawned OK, not quarantined). The capability signal for dictation's
    /// Auto provider routing: without it, on-device means the slow CPU path.
    pub fn gpu_active(&self) -> bool {
        !self.gpu_disabled.load(Ordering::Relaxed)
    }

    fn resolve(&self, language: &str) -> Result<DictationLocalAsrConfig, String> {
        crate::meeting_engine::resolve_dictation_local_asr_config(language)
    }

    /// Warm the primary engine so the first dictation is instant.
    pub fn prewarm(&self, language: &str) {
        if !asr_core::cpu_supports_local_asr() {
            return; // pre-AVX2 CPU: never touch whisper (see transcribe)
        }
        let cfg = match self.resolve(language) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "[asr] prewarm skipped: config unresolved");
                return;
            }
        };

        if !self.gpu_disabled.load(Ordering::Relaxed) {
            let mut guard = self.lock_worker();
            if let Some(w) = guard.as_mut() {
                w.prewarm(&cfg);
                return;
            }
        }
        self.inproc.prewarm(cfg);
    }

    /// Transcribe a WAV clip. Prepares audio once, tries the GPU worker, and
    /// falls over to the in-process engine on worker death.
    pub fn transcribe(&self, wav: &[u8], language: &str) -> Result<LocalAsrOutput, String> {
        // Hard gate BEFORE any whisper call: on a pre-AVX2 CPU ggml dies with an
        // illegal instruction (silent process death), so refuse cleanly instead.
        if !asr_core::cpu_supports_local_asr() {
            return Err(
                "This computer's CPU does not support the on-device speech model (AVX2 \
                 required). Switch dictation to a cloud provider in Settings."
                    .to_string(),
            );
        }
        let cfg = self.resolve(language)?;
        let prep_started = Instant::now();
        let pcm = asr_core::audio::prepare(wav).map_err(|e| e.to_string())?;
        // Not part of total_ms (which times the engine), but it IS user-felt
        // latency — log it so slow denoise/resample (e.g. unoptimized dev
        // builds) can't hide between the pipeline and engine timings.
        tracing::info!(
            prepare_ms = prep_started.elapsed().as_millis() as u64,
            "[asr] audio prepared (resample + denoise)"
        );
        let started = Instant::now();

        // 1. GPU worker (respawn if it died on a previous request).
        if !self.gpu_disabled.load(Ordering::Relaxed) {
            let mut guard = self.lock_worker();
            self.ensure_worker(&mut guard);
            // Own the result before touching the slot, so the &mut borrow ends.
            let attempt = guard.as_mut().map(|w| w.transcribe(&pcm, &cfg));
            match attempt {
                Some(Ok(mut out)) => {
                    out.total_ms = started.elapsed().as_millis() as u64;
                    drop(guard);
                    return self.finalize(out);
                }
                Some(Err(WorkerError::Transcription(msg))) => return Err(msg),
                Some(Err(WorkerError::Dead(msg))) => {
                    *guard = None; // Drop kills the child + frees VRAM
                    let deaths = self.gpu_deaths.fetch_add(1, Ordering::Relaxed) + 1;
                    if deaths >= MAX_GPU_DEATHS {
                        self.gpu_disabled.store(true, Ordering::Relaxed);
                        tracing::error!(error = %msg, deaths, "[asr] GPU quarantined for session; CPU only");
                    } else {
                        tracing::warn!(error = %msg, deaths, "[asr] GPU worker died; failing over to CPU, will retry");
                    }
                    // fall through to the in-process engine
                }
                None => { /* no worker (spawn failed just now) → in-process path */ }
            }
        }

        // 2. In-process safety path (CPU on Win/Linux, Metal on macOS).
        let mut out = self.inproc.transcribe(pcm, &cfg)?;
        out.total_ms = started.elapsed().as_millis() as u64;
        self.finalize(out)
    }

    /// Respawn the worker if it's absent but the GPU isn't disabled. A failed
    /// respawn disables the GPU for the session.
    fn ensure_worker(&self, guard: &mut Option<WorkerClient>) {
        if guard.is_some() {
            return;
        }
        match WorkerClient::spawn() {
            Ok(w) => {
                tracing::info!(device = %w.device().name, "[asr] GPU worker respawned");
                *guard = Some(w);
            }
            Err(e) => {
                self.gpu_disabled.store(true, Ordering::Relaxed);
                tracing::info!(reason = %e, "[asr] GPU worker respawn failed; CPU only for session");
            }
        }
    }

    fn finalize(&self, out: LocalAsrOutput) -> Result<LocalAsrOutput, String> {
        if crate::meeting_engine::is_low_quality_transcript_artifact(&out.transcript) {
            return Err("local speech returned no usable transcript".to_string());
        }
        Ok(out)
    }

    fn lock_worker(&self) -> std::sync::MutexGuard<'_, Option<WorkerClient>> {
        self.worker.lock().unwrap_or_else(|p| p.into_inner())
    }
}
