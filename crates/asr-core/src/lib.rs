//! Backend-agnostic on-device speech-to-text core.
//!
//! This crate owns everything that both the in-process engine (CPU / macOS
//! Metal, linked into the desktop app) and the isolated GPU worker process
//! (Vulkan, Windows/Linux) need in common:
//!
//! * [`config::DictationLocalAsrConfig`] — the resolved decode/VAD settings.
//! * [`audio`] — WAV decode + 16 kHz conditioning (done once, on the app side).
//! * [`whisper::WhisperEngine`] — a warm, single-threaded whisper.cpp engine that
//!   loads a model onto a chosen [`Device`] and transcribes prepared PCM.
//! * [`ipc`] — the length-prefixed message protocol spoken between the app's
//!   supervisor and the GPU worker.
//! * [`probe`] (feature `vulkan`, worker-only) — Vulkan device enumeration and
//!   "prefer the discrete GPU" selection, index-consistent with ggml.
//!
//! The Vulkan-specific surface is entirely gated behind the `vulkan` feature so
//! the desktop app links neither ash nor the Vulkan loader.

pub mod audio;
pub mod config;
pub mod error;
pub mod ipc;
pub mod output;
pub mod vad;
pub mod whisper;

#[cfg(feature = "vulkan")]
pub mod probe;

pub use config::DictationLocalAsrConfig;
pub use error::AsrError;
pub use output::LocalAsrOutput;
pub use whisper::{Device, WhisperEngine};

/// Backends a transcription can run on. `Cpu` needs no GPU feature; `Gpu`
/// requires the crate to have been built with `metal` or `vulkan`, otherwise
/// whisper.cpp reports "no GPU found" and silently uses the CPU.
///
/// This is a plain, serializable descriptor — see [`whisper::Device`] for the
/// value the engine actually consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Backend {
    /// Pure CPU inference. Always available, never aborts the process.
    Cpu,
    /// GPU inference on the given ggml device index (0 for Metal).
    Gpu { device: i32 },
}

impl Backend {
    #[must_use]
    pub fn is_gpu(self) -> bool {
        matches!(self, Backend::Gpu { .. })
    }
}

/// Whether this machine's CPU can run the bundled whisper build.
///
/// Distributable builds pin an **AVX2 floor** (`GGML_NATIVE=OFF, GGML_AVX2=ON`
/// in the build scripts) so the binary is independent of the build host. CPUs
/// without AVX2 — pre-2013 Intel, pre-2015 AMD, and many Pentium/Celeron parts —
/// don't fail gracefully: the first ggml call dies with an illegal instruction.
/// Every entry point into whisper (app router, GPU worker) must gate on this
/// and degrade to cloud STT instead.
#[must_use]
pub fn cpu_supports_local_asr() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true // ARM/Apple Silicon builds don't carry the AVX2 floor
    }
}
