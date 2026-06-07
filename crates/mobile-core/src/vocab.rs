use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabScope {
    Personal,
    Org,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabTerm {
    pub term: String,
    pub spoken_aliases: Vec<String>,
    pub term_type: String,
    pub scope: VocabScope,
    pub priority: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleDefaults {
    pub default_style: String,
    pub language_hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabSnapshot {
    pub hash: String,
    pub terms: Vec<VocabTerm>,
    pub style_defaults: StyleDefaults,
    pub stt_replacements_version: u64,
    pub prompt_hints_version: u64,
}

#[must_use]
pub fn has_term(snapshot: &VocabSnapshot, term: &str) -> bool {
    snapshot
        .terms
        .iter()
        .any(|candidate| candidate.term.eq_ignore_ascii_case(term))
}
