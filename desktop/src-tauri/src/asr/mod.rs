//! On-device dictation ASR: adaptive, process-isolated, universal.
//!
//! Architecture (see the design notes in memory `said-windows-whisper-vulkan`):
//!   * [`inproc::InProcEngine`] — always-present in-process engine. CPU on
//!     Windows/Linux (never aborts the process), Metal on macOS.
//!   * [`worker_client::WorkerClient`] — isolated GPU worker (`airnote-asr-gpu`)
//!     on Windows/Linux, so a ggml `GGML_ABORT`/`exit(1)` on a bad GPU can't take
//!     the app down.
//!   * [`router::AsrRouter`] — the supervisor: prefers the GPU worker, fails over
//!     to the in-process engine on crash, quarantines a repeatedly-crashing GPU.
//!
//! Public surface mirrors the previous `local_asr` module so call sites are a
//! one-line change.

mod inproc;
mod router;
mod worker_client;

use std::path::Path;
use std::sync::OnceLock;

use asr_core::LocalAsrOutput;

pub use router::AsrRouter;

/// Prewarm language used at startup (see dictation STT routing notes).
const DEFAULT_PREWARM_LANGUAGE: &str = "hinglish";

static ROUTER: OnceLock<AsrRouter> = OnceLock::new();

fn router() -> &'static AsrRouter {
    ROUTER.get_or_init(AsrRouter::start)
}

/// Transcribe a WAV recording on the best available local engine.
///
/// Drop-in replacement for the old `local_asr::transcribe_wav_bytes`.
pub fn transcribe_wav_bytes(wav: Vec<u8>, language: String) -> Result<LocalAsrOutput, String> {
    router().transcribe(&wav, &language)
}

/// Warm the primary engine at startup so the first dictation has no load latency.
pub fn prewarm_default_language() {
    if !env_bool("AIRNOTE_DICTATION_ASR_PREWARM", true) {
        return;
    }
    router().prewarm(DEFAULT_PREWARM_LANGUAGE);
}

/// True while the isolated GPU worker is this session's primary local engine.
/// First call spawns the router (probe + handshake) if it isn't running yet.
pub fn gpu_active() -> bool {
    router().gpu_active()
}

/// The model file's name, e.g. `ggml-oriserve-hinglish-fp16.bin`.
pub(crate) fn model_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("local-whisper")
        .to_string()
}

/// Parse a boolean env var with a default.
pub(crate) fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .and_then(|v| match v.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}
