//! Server-side dictation runtime (Lark unified-runtime plan).
//!
//! The shared brain for desktop and mobile clients: STT (Deepgram) → LLM polish
//! (Groq) → Hinglish script guard → protected-term resolver, plus per-user
//! personal-memory learning. Clients stay thin (capture audio, insert result).

pub mod learning;
pub mod memory;
pub mod polish;
pub mod prompt;
pub mod resolver;
pub mod script;
pub mod stt;
