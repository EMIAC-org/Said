//! Wave 6 — protected-term resolver.
//!
//! Deterministic, post-polish authority over the final text. Applies the user's
//! learned personal STT replacements (spoken → canonical) with the plan's
//! priority chain:
//!
//!   personal blocked > personal approved replacement > (vocab hint, in prompt)
//!   > LLM polish
//!
//! Whole-word, ASCII-case-insensitive matching (learned aliases are
//! overwhelmingly ASCII; Hinglish output is romanized to Latin). A SINGLE
//! left-to-right pass over the original text with longest-match-wins, so rules
//! never cascade into one another.

use std::collections::HashSet;

pub fn apply_resolver(text: &str, replacements: &[(String, String)], blocked: &HashSet<String>) -> String {
    let mut rules: Vec<(Vec<char>, &str)> = replacements
        .iter()
        .filter_map(|(spoken, canonical)| {
            let key = spoken.trim().to_ascii_lowercase();
            if key.is_empty() || blocked.contains(&key) {
                None
            } else {
                Some((key.chars().collect::<Vec<char>>(), canonical.as_str()))
            }
        })
        .collect();
    rules.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    if rules.is_empty() {
        return text.to_string();
    }

    let hay: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < hay.len() {
        let mut matched = false;
        for (needle, canonical) in &rules {
            if window_matches_ci(&hay, i, needle) && boundary_ok(&hay, i, needle.len()) {
                out.push_str(canonical);
                i += needle.len();
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn window_matches_ci(hay: &[char], at: usize, needle: &[char]) -> bool {
    if needle.is_empty() || at + needle.len() > hay.len() {
        return false;
    }
    for (k, nc) in needle.iter().enumerate() {
        if !hay[at + k].eq_ignore_ascii_case(nc) {
            return false;
        }
    }
    true
}

fn boundary_ok(hay: &[char], at: usize, len: usize) -> bool {
    let before_ok = at == 0 || !is_word_char(hay[at - 1]);
    let after = at + len;
    let after_ok = after >= hay.len() || !is_word_char(hay[after]);
    before_ok && after_ok
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reps(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn applies_multiword_case_insensitive() {
        let out = apply_resolver("Send to Mac Ops today.", &reps(&[("mac ops", "Macobs")]), &HashSet::new());
        assert_eq!(out, "Send to Macobs today.");
    }

    #[test]
    fn respects_word_boundaries() {
        assert_eq!(apply_resolver("macating now", &reps(&[("mac", "Mac")]), &HashSet::new()), "macating now");
        assert_eq!(apply_resolver("the mac is here", &reps(&[("mac", "Mac")]), &HashSet::new()), "the Mac is here");
    }

    #[test]
    fn blocked_alias_is_skipped() {
        let mut blocked = HashSet::new();
        blocked.insert("mac ops".to_string());
        assert_eq!(apply_resolver("send to mac ops", &reps(&[("mac ops", "Macobs")]), &blocked), "send to mac ops");
    }

    #[test]
    fn no_cascade_between_rules() {
        let out = apply_resolver("say hello world", &reps(&[("hello world", "hi"), ("hi", "BYE")]), &HashSet::new());
        assert_eq!(out, "say hi");
    }

    #[test]
    fn longest_match_wins() {
        let out = apply_resolver("the mac ops team", &reps(&[("mac", "M"), ("mac ops", "Macobs")]), &HashSet::new());
        assert_eq!(out, "the Macobs team");
    }
}
