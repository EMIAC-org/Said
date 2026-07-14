//! DeepInfra provider preset.
//!
//! DeepInfra exposes the OpenAI-compatible transcription endpoint at
//! `https://api.deepinfra.com/v1/audio/transcriptions` with standard Bearer
//! auth. We run `openai/whisper-large-v3` — the full model, picked over turbo
//! after live A/B on Hinglish dictation (better accuracy; ~1.5–2× turbo's
//! latency, still seconds-per-clip).
//!
//! Docs:
//! * API reference: <https://docs.deepinfra.com/api-reference/audio/openai-audio-transcriptions>
//! * Model card:    <https://deepinfra.com/openai/whisper-large-v3>

use std::time::Duration;

use crate::HostedSttConfig;

pub const BASE_URL: &str = "https://api.deepinfra.com/v1";
/// Production dictation model (see module docs).
pub const WHISPER_LARGE_V3: &str = "openai/whisper-large-v3";
/// large-v3 distilled to an 8×-faster decoder at a small accuracy cost.
pub const WHISPER_LARGE_V3_TURBO: &str = "openai/whisper-large-v3-turbo";
/// NVIDIA's 0.6B streaming multilingual ASR. Faster/cheaper tier than whisper.
/// Behavior notes (verified live): with a `language=hi` hint it transliterates
/// EVERYTHING into Devanagari, including English words — and it returned empty
/// transcripts for most real Hinglish clips. Not production-fit for us.
pub const NEMOTRON_STREAMING_MULTILINGUAL: &str =
    "nvidia/Nemotron-3.5-ASR-Streaming-Multilingual-0.6b";

/// The env var that carries the DeepInfra key — baked into release builds via
/// `option_env!` (same scheme as `DEEPSEEK_API_KEY` for meeting summaries).
pub const API_KEY_ENV: &str = "DEEPINFRA_API_KEY";

/// Dictation-tuned config: fail fast when offline (6s connect budget), but
/// leave room for a multi-minute clip to upload on a slow uplink (75s total).
pub fn config(api_key: String) -> HostedSttConfig {
    HostedSttConfig {
        base_url: BASE_URL.to_string(),
        model: WHISPER_LARGE_V3.to_string(),
        api_key,
        connect_timeout: Duration::from_secs(6),
        request_timeout: Duration::from_secs(75),
    }
}
