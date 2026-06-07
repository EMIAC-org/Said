//! Wave 5 — server-side personal learning (explicit, gated).
//!
//! Given the AirNote output and the text the user kept after editing, extract
//! the single changed span and decide whether to learn a personal STT
//! replacement, block it as unsafe, or ignore it. Deterministic + dependency
//! free; the hot path never depends on this (insert first, learn later).

#[derive(Debug, PartialEq, Eq)]
pub enum LearnDecision {
    Learn { spoken: String, canonical: String },
    Block { spoken: String },
    Ignore,
}

pub fn extract_change(original: &str, edited: &str) -> Option<(String, String)> {
    let o: Vec<&str> = original.split_whitespace().collect();
    let e: Vec<&str> = edited.split_whitespace().collect();

    let mut p = 0;
    while p < o.len() && p < e.len() && o[p].eq_ignore_ascii_case(e[p]) {
        p += 1;
    }
    let mut s = 0;
    while s < o.len() - p
        && s < e.len() - p
        && o[o.len() - 1 - s].eq_ignore_ascii_case(e[e.len() - 1 - s])
    {
        s += 1;
    }

    let from = o[p..o.len() - s].join(" ");
    let to = e[p..e.len() - s].join(" ");
    if from.is_empty() && to.is_empty() {
        return None;
    }
    Some((from, to))
}

pub fn analyze_edit(original: &str, edited: &str) -> LearnDecision {
    let Some((from, to)) = extract_change(original, edited) else {
        return LearnDecision::Ignore;
    };
    let from_t = from.trim();
    let to_t = to.trim();
    if from_t.is_empty() || to_t.is_empty() {
        return LearnDecision::Ignore;
    }
    if from_t.split_whitespace().count() > 3 || to_t.split_whitespace().count() > 3 {
        return LearnDecision::Ignore;
    }
    let spoken = from_t.to_ascii_lowercase();
    if spoken.chars().count() < 2 || to_t.chars().count() < 2 {
        return LearnDecision::Ignore;
    }
    if spoken == to_t.to_ascii_lowercase() {
        return LearnDecision::Ignore;
    }
    if !from_t.chars().any(|c| c.is_alphabetic()) || !to_t.chars().any(|c| c.is_alphabetic()) {
        return LearnDecision::Ignore;
    }
    // Block if ANY token of the spoken span is a common word — applied token-wise
    // so multi-word spans like "go to" / "can do" cannot poison the resolver.
    if spoken.split_whitespace().any(is_common_word) {
        return LearnDecision::Block { spoken };
    }
    LearnDecision::Learn {
        spoken,
        canonical: to_t.to_string(),
    }
}

fn is_common_word(word: &str) -> bool {
    const COMMON: &[&str] = &[
        "kaisa", "kaise", "mein", "main", "time", "can", "go", "the", "is", "a", "an", "and",
        "to", "of", "or", "if", "in", "on", "for", "it", "this", "that", "you", "me", "we",
        "do", "done", "hai", "ho", "ka", "ki", "ke", "kya", "nahi", "haan", "ok", "okay",
        "yes", "no", "please", "hello", "hi", "good", "bad", "now", "here", "there",
    ];
    COMMON.contains(&word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_safe_alias() {
        match analyze_edit("send the mac ops update", "send the Macobs update") {
            LearnDecision::Learn { spoken, canonical } => {
                assert_eq!(spoken, "mac ops");
                assert_eq!(canonical, "Macobs");
            }
            other => panic!("expected Learn, got {other:?}"),
        }
    }

    #[test]
    fn blocks_common_word_alias() {
        assert_eq!(
            analyze_edit("i can do it", "i go do it"),
            LearnDecision::Block { spoken: "can".to_string() }
        );
    }

    #[test]
    fn blocks_multiword_span_with_common_token() {
        assert_eq!(
            analyze_edit("please go to office now", "please reach office now"),
            LearnDecision::Block { spoken: "go to".to_string() }
        );
    }

    #[test]
    fn ignores_case_only_change() {
        assert_eq!(analyze_edit("call rahul now", "call Rahul now"), LearnDecision::Ignore);
    }

    #[test]
    fn ignores_number_change() {
        assert_eq!(analyze_edit("meet at 5 pm", "meet at 6 pm"), LearnDecision::Ignore);
    }
}
