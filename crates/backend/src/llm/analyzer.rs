//! Learning analyzer types.
//!
//! These types are used by the deterministic classifier in `routes/classify.rs`.
//! The LLM-based analysis function was removed — classification is now fully
//! deterministic (diff-based) with no LLM call.

use serde::{Deserialize, Serialize};

/// Input to the analyzer (kept for the `ExistingTerm` type used by prompt
/// rendering — may be removed if no longer needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerInput {
    pub transcript: String,
    pub polished: String,
    pub user_kept: String,
    pub output_language: String,
    pub existing_vocab: Vec<ExistingTerm>,
}

/// A vocabulary term the user already has.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingTerm {
    pub term: String,
    pub current_meaning: Option<String>,
    pub sighting_count: i64,
    pub examples: Vec<String>,
}

/// Structured output from the analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerOutput {
    pub changes: Vec<AnalyzedChange>,
    pub overall_class: String,
}

/// A single change identified by the analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzedChange {
    pub original: String,
    pub corrected: String,
    pub reason: ChangeReason,
    pub meaning: Option<String>,
    pub context_example: Option<String>,
    pub should_learn: bool,
    pub confidence: f64,
    pub skip_reason: Option<String>,
    pub format_rule: Option<String>,
}

/// Why the user made this change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason {
    SttError,
    PolishError,
    FormatPreference,
    StylePreference,
    StructuralRewrite,
}

impl ChangeReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SttError => "stt_error",
            Self::PolishError => "polish_error",
            Self::FormatPreference => "format_preference",
            Self::StylePreference => "style_preference",
            Self::StructuralRewrite => "structural_rewrite",
        }
    }

    pub fn is_learnable(&self) -> bool {
        matches!(
            self,
            Self::SttError | Self::PolishError | Self::FormatPreference
        )
    }
}
