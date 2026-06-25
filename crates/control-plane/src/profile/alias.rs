//! Profile-owned deterministic alias placeholders (Wave 1 — types + validation only).
//!
//! Examples of valid multi-word recoveries (generated later by DeepSeek):
//! - `n 10` -> `n8n`
//! - `deep gram` -> `Deepgram`
//!
//! Hard rule: never alias common Hinglish/Hindi/English words.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::alias_safety::{is_common_alias_source, normalize_alias_phrase};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAliasStatus {
    Candidate,
    Active,
    Blocked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileAlias {
    pub source_phrase: String,
    pub canonical_phrase: String,
    pub status: ProfileAliasStatus,
    pub confidence: f64,
    pub evidence_count: i32,
    #[serde(default)]
    pub reason: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub profile_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AliasRejectReason {
    EmptyPhrase,
    IdenticalSourceAndCanonical,
    CommonSourcePhrase,
    InstructionLikeCanonical,
}

impl std::fmt::Display for AliasRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPhrase => write!(f, "empty phrase"),
            Self::IdenticalSourceAndCanonical => write!(f, "source and canonical are identical"),
            Self::CommonSourcePhrase => write!(f, "common Hinglish/Hindi/English source phrase"),
            Self::InstructionLikeCanonical => write!(f, "canonical looks like an instruction"),
        }
    }
}

fn looks_instruction_like(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    [
        "you must",
        "ignore previous",
        "ignore all",
        "system:",
        "assistant:",
        "disregard",
        "override",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Validate a profile-owned alias candidate. Does not persist or apply replacements.
pub fn validate_alias_candidate(alias: &ProfileAlias) -> Result<(), AliasRejectReason> {
    let source = alias.source_phrase.trim();
    let canonical = alias.canonical_phrase.trim();
    if source.is_empty() || canonical.is_empty() {
        return Err(AliasRejectReason::EmptyPhrase);
    }
    if source.eq_ignore_ascii_case(canonical) {
        return Err(AliasRejectReason::IdenticalSourceAndCanonical);
    }
    let source_norm = normalize_alias_phrase(source);
    if is_common_alias_source(&source_norm) {
        return Err(AliasRejectReason::CommonSourcePhrase);
    }
    if looks_instruction_like(canonical) {
        return Err(AliasRejectReason::InstructionLikeCanonical);
    }
    Ok(())
}

/// Parse and validate every alias entry inside `profile_json.aliases`.
pub fn validate_aliases_in_json(
    profile_json: &serde_json::Value,
) -> Vec<(usize, AliasRejectReason)> {
    let Some(aliases) = profile_json.get("aliases").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut rejects = Vec::new();
    for (idx, entry) in aliases.iter().enumerate() {
        let parsed: Result<ProfileAlias, _> = serde_json::from_value(entry.clone());
        match parsed {
            Ok(alias) => {
                if let Err(reason) = validate_alias_candidate(&alias) {
                    rejects.push((idx, reason));
                }
            }
            Err(_) => rejects.push((idx, AliasRejectReason::EmptyPhrase)),
        }
    }
    rejects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(source: &str, canonical: &str) -> ProfileAlias {
        ProfileAlias {
            source_phrase: source.into(),
            canonical_phrase: canonical.into(),
            status: ProfileAliasStatus::Candidate,
            confidence: 0.9,
            evidence_count: 2,
            reason: String::new(),
            last_seen_at: None,
            profile_version: 1,
        }
    }

    #[test]
    fn accepts_multi_word_product_alias() {
        assert!(validate_alias_candidate(&alias("n 10", "n8n")).is_ok());
        assert!(validate_alias_candidate(&alias("deep gram", "Deepgram")).is_ok());
    }

    #[test]
    fn rejects_common_word_alias() {
        assert_eq!(
            validate_alias_candidate(&alias("kaam", "Kafka")),
            Err(AliasRejectReason::CommonSourcePhrase)
        );
    }
}
