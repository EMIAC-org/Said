//! Lean, store-free input types for the shared polish prompt builder.
//!
//! These mirror the three fields the prompt actually reads from the local
//! backend's rusqlite-backed `Preferences` and `Correction`, so `said-core`
//! stays free of any database dependency. The local backend converts its own
//! types into these at the call site.

/// The subset of user preferences the voice-polish prompt depends on.
#[derive(Clone, Debug, Default)]
pub struct PolishPrefs {
    /// "hinglish" | "hindi" | "english".
    pub output_language: String,
    /// Tone/persona preset id ("neutral", "professional", ..., "custom").
    pub tone_preset: String,
    /// User's custom prompt body, surfaced when `tone_preset == "custom"`.
    pub custom_prompt: Option<String>,
}

/// A learned polish correction (`wrong → right`), rendered as a soft
/// "POLISH PREFERENCES" hint. Field shape matches the backend's
/// `store::corrections::Correction`.
#[derive(Clone, Debug)]
pub struct Correction {
    pub wrong: String,
    pub right: String,
    pub count: i64,
}
