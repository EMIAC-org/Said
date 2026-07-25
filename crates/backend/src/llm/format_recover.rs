//! Email extraction from polished output.
//!
//! This module used to host a full deterministic post-LLM formatting guard
//! (protocol mishears, spoken emails/URLs/paths/identifiers/env vars). Every
//! one of those passes was reachable only through `recover()`, which was
//! disabled at its single call site and never re-enabled, so they were removed
//! along with the entry point.
//!
//! What remains is the one function the product actually calls:
//! [`extract_emails`], used by `store::email_memory` to harvest addresses out
//! of a finished polish for the email-memory table.

use once_cell::sync::Lazy;
use regex::Regex;

static EMAIL_ADDR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}").unwrap());

pub fn extract_emails(text: &str) -> Vec<String> {
    email_spans(text)
        .into_iter()
        .map(|(_, _, email)| email)
        .collect()
}

/// Byte spans of every email address in `text`, with trailing sentence
/// punctuation trimmed off the match (`a@b.com.` → `a@b.com`).
fn email_spans(text: &str) -> Vec<(usize, usize, String)> {
    EMAIL_ADDR
        .find_iter(text)
        .map(|m| {
            let mut end = m.end();
            while end > m.start()
                && text[..end]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | ')' | ']' | '}'))
            {
                end -= text[..end].chars().next_back().unwrap().len_utf8();
            }
            (m.start(), end, text[m.start()..end].to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::extract_emails;

    #[test]
    fn extracts_plain_addresses() {
        assert_eq!(
            extract_emails("mail anish@gmail.com and v.abhi@example.co.in today"),
            vec!["anish@gmail.com", "v.abhi@example.co.in"]
        );
    }

    #[test]
    fn trims_trailing_sentence_punctuation() {
        assert_eq!(
            extract_emails("send it to anish@gmail.com."),
            vec!["anish@gmail.com"]
        );
        assert_eq!(
            extract_emails("(anish@gmail.com), then reply"),
            vec!["anish@gmail.com"]
        );
    }

    #[test]
    fn returns_nothing_for_prose() {
        assert!(extract_emails("mail anish at gmail dot com").is_empty());
    }
}
