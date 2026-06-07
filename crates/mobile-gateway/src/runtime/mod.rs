//! Server-side dictation runtime.
//!
//! The pipeline that turns iPhone audio into polished, insertable text:
//!   STT (Deepgram) → LLM polish (Groq, streaming) → Hinglish script guard.
//! Plus the supporting prompt builder and personal-vocab snapshot.

pub mod learning;
pub mod polish;
pub mod prompt;
pub mod resolver;
pub mod script;
pub mod stt;
pub mod vocab;
