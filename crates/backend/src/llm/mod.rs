pub mod alias_review;
pub mod alias_safety;
pub mod analyzer;
pub mod cerebras;
pub mod classifier;
pub mod deepinfra;
pub mod deepseek;
pub mod devanagari_recovery;
pub mod edit_diff;
pub mod format_pass;
pub mod format_recover;
pub mod gateway;
pub mod gemini_direct;
pub mod groq;
pub mod meaning;
pub mod openai_codex;
pub mod phonetic_triage;
pub mod phonetics;
pub mod polish_dispatch;
pub mod pre_filter;
pub mod promotion_gate;
pub mod prompt;
pub mod script;
pub mod stream_safety;
pub mod vocab_retrieval;

/// Shared result type returned by all LLM streaming clients.
pub struct PolishResult {
    pub polished: String,
    pub polish_ms: u64,
}
