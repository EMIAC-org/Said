//! Together AI live Nemotron transport constants.

pub const API_KEY_ENV: &str = "TOGETHER_API_KEY";
pub const NEMOTRON_3_5_ASR_STREAMING_0_6B: &str = "nvidia/nemotron-3.5-asr-streaming-0.6b";
/// Hindi is intentional for AirNote's Hinglish dictation path. Constraining
/// decoding prevents short Roman-Hindi utterances from being classified as
/// unrelated languages before the server polish step can romanize them.
pub const NEMOTRON_LANGUAGE: &str = "hi";
pub const REALTIME_ENDPOINT: &str = "wss://api.together.ai/v1/realtime";
