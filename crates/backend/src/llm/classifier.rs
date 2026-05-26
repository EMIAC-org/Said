//! Edit classifier types.
//!
//! The LLM-based classify_edit function was removed — classification is now
//! fully deterministic in `routes/classify.rs`. Only the types remain, used
//! by `phonetic_triage.rs` and `learning_flow_tests.rs`.

use serde::{Deserialize, Serialize};

use super::edit_diff::Hunk;

/// Specific token-level correction extracted from inside a composite hunk.
///
/// When the user's edit bundles multiple changes (e.g. fixed a misheard name
/// AND wrapped it in a markdown link), the diff produces ONE hunk but only a
/// SUB-string of it is the actual learnable STT/polish error.  The labeler
/// emits this struct to tell us "promote *this specific term* from inside the
/// hunk", rather than the whole `kept_window`.
///
/// Example:
///   hunk.polish_window = "Anis at the rate Gmail dot com"
///   hunk.kept_window   = "[anish@gmail.com](mailto:anish@gmail.com)"
///   extracted_term     = { transcript_form: "Anis", correct_form: "anish" }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedTerm {
    /// What STT actually wrote for this token (within the polish/transcript).
    pub transcript_form: String,
    /// The correctly-spelled form the user wanted (must be a whole-word
    /// substring of `hunk.kept_window`).
    pub correct_form: String,
}

/// One labelled hunk — pairs a deterministic diff hunk with the LLM's class
/// assignment AND (optionally) a specific token-level extraction within the
/// hunk.  Promotion uses `extracted_term` when present, otherwise falls back
/// to `kept_window`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelledHunk {
    pub hunk: Hunk,
    pub class: EditClass,
    pub confidence: f64,
    /// When the LLM identifies a specific token within the hunk as the actual
    /// STT/polish error (vs. the whole hunk being the correction), it emits
    /// this field.  Stage-4 promotion prefers it over `kept_window`.
    #[serde(default)]
    pub extracted_term: Option<ExtractedTerm>,
}

/// Backwards-compatible alias for the route layer — a labelled hunk is the
/// candidate now.  The route consumes `transcript_form` / `polish_form` /
/// `correct_form` getters defined below.
pub type Candidate = LabelledHunk;

impl LabelledHunk {
    /// What STT transcribed for the candidate.  When `extracted_term` is
    /// present, returns the specific sub-token from inside the hunk; else
    /// returns the full hunk's transcript window.
    pub fn transcript_form(&self) -> &str {
        self.extracted_term
            .as_ref()
            .map(|t| t.transcript_form.as_str())
            .unwrap_or(&self.hunk.transcript_window)
    }
    /// What the polish step produced.  No extraction equivalent — polish_form
    /// is always the hunk's polish window because polish errors are the WHOLE
    /// substituted region.
    pub fn polish_form(&self) -> &str {
        &self.hunk.polish_window
    }
    /// The proposed correct form.  When `extracted_term` is present, returns
    /// the specific sub-token; else returns the full hunk's kept window.
    /// Stage-4 promotion uses THIS getter.
    pub fn correct_form(&self) -> &str {
        self.extracted_term
            .as_ref()
            .map(|t| t.correct_form.as_str())
            .unwrap_or(&self.hunk.kept_window)
    }
    /// Best guess at what was actually spoken.  For STT_ERROR this equals
    /// `correct_form()` (user restored what they actually said).
    pub fn spoke(&self) -> &str {
        self.correct_form()
    }
}

/// The four mutually-exclusive classes of edit.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EditClass {
    SttError,
    PolishError,
    UserRephrase,
    UserRewrite,
}

impl EditClass {
    pub fn as_str(self) -> &'static str {
        match self {
            EditClass::SttError => "STT_ERROR",
            EditClass::PolishError => "POLISH_ERROR",
            EditClass::UserRephrase => "USER_REPHRASE",
            EditClass::UserRewrite => "USER_REWRITE",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "STT_ERROR" | "STTERROR" => Some(Self::SttError),
            "POLISH_ERROR" | "POLISHERROR" => Some(Self::PolishError),
            "USER_REPHRASE" | "USERREPHRASE" => Some(Self::UserRephrase),
            "USER_REWRITE" | "USERREWRITE" => Some(Self::UserRewrite),
            _ => None,
        }
    }

    /// Should this class produce any learning artifacts?
    pub fn is_learnable(self) -> bool {
        matches!(self, Self::SttError | Self::PolishError)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifyResult {
    pub class: EditClass,
    pub reason: String,
    /// One entry per diff hunk (in input order).  Each carries the original
    /// hunk plus the LLM's class label and confidence.  The route uses the
    /// hunk's text — it never relies on the LLM having "invented" a term.
    pub candidates: Vec<LabelledHunk>,
    /// Mean confidence across labelled hunks ∈ [0, 1].  Soft signal — promotion
    /// logic also uses script + phonetic + jargon gates as defense in depth.
    #[serde(default)]
    pub confidence: f64,
}
