pub mod alias_review;
pub mod classifier;
pub mod edit_diff;
pub mod gateway;
pub mod gemini_direct;
pub mod groq;
pub mod meaning;
pub mod openai_codex;
pub mod phonetic_triage;
pub mod phonetics;
pub mod pre_filter;
pub mod promotion_gate;
pub mod prompt;
pub mod script;
pub mod stream_safety;
pub mod vocab_resolver;

/// Shared result type returned by all LLM streaming clients.
pub struct PolishResult {
    pub polished: String,
    pub polish_ms: u64,
}
