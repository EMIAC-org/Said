//! Common-word guards for profile-owned alias candidates.
//!
//! Aliases must never map common Hinglish/Hindi/English words to protected terms.
//! Multi-word clubbing like `n 10 -> n8n` is allowed when the full phrase is non-common.

/// Account-global profile scope when no org is active.
pub const GLOBAL_ORG_SCOPE: &str = "00000000-0000-0000-0000-000000000000";

pub fn global_org_scope() -> uuid::Uuid {
    uuid::Uuid::parse_str(GLOBAL_ORG_SCOPE).expect("valid sentinel UUID")
}

pub fn normalize_alias_phrase(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns true when `norm` is a common Hinglish/Hindi/English token or phrase
/// composed only of common tokens. Mirrors runtime learning memory guards.
pub fn is_common_alias_source(norm: &str) -> bool {
    const COMMON: &[&str] = &[
        "kaisa",
        "kaisi",
        "kaise",
        "aisa",
        "aisi",
        "aise",
        "laga",
        "lagi",
        "lage",
        "main",
        "mein",
        "hai",
        "hain",
        "tha",
        "thi",
        "the",
        "time",
        "can",
        "go",
        "do",
        "this",
        "for",
        "me",
        "one thing",
        "ek baar",
        "char log",
        "kaam",
        "kya",
        "kyun",
        "aur",
        "batao",
        "bolo",
        "karo",
        "karna",
        "kar",
        "bhejo",
        "dikhao",
        "kholo",
        "a",
        "an",
        "is",
        "are",
        "was",
        "were",
        "be",
        "been",
        "being",
        "have",
        "has",
        "had",
        "does",
        "did",
        "will",
        "would",
        "could",
        "should",
        "may",
        "might",
        "must",
        "shall",
        "and",
        "or",
        "but",
        "if",
        "in",
        "on",
        "at",
        "to",
        "of",
        "it",
        "its",
        "that",
        "which",
        "who",
        "not",
        "no",
        "yes",
        "ok",
        "okay",
        "yeah",
        "yep",
        "nope",
        "open",
        "close",
        "send",
        "return",
        "source",
        "schema",
        "resolver",
        "chart",
        "bank",
        "smallcap",
        "small",
        "cap",
        "one",
        "two",
        "too",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "ek",
        "do",
        "teen",
        "char",
        "panch",
        "paanch",
        "ka",
        "ke",
        "ki",
        "ko",
        "se",
        "par",
        "pe",
    ];
    if COMMON.contains(&norm) {
        return true;
    }
    let tokens: Vec<_> = norm.split_whitespace().collect();
    !tokens.is_empty() && tokens.iter().all(|t| COMMON.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_word_non_common_alias_is_allowed() {
        assert!(!is_common_alias_source("n 10"));
        assert!(!is_common_alias_source("deep gram"));
    }

    #[test]
    fn common_hinglish_words_are_blocked() {
        assert!(is_common_alias_source("kaam"));
        assert!(is_common_alias_source("main mein"));
    }
}
