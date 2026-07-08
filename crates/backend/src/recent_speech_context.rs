use std::collections::{HashMap, HashSet};

use said_core::polish::prompt::RECENT_SPEECH_HINTS_MAX_CHARS;

pub const RECENT_SPEECH_TTL_MS: i64 = 10 * 60 * 1000;
pub const RECENT_SPEECH_RUN_LIMIT: usize = 5;

#[derive(Clone, Debug)]
struct Token {
    text: String,
    lower: String,
}

#[derive(Clone, Debug)]
struct Candidate {
    text: String,
    score: i32,
    order: usize,
}

pub fn extract_recent_speech_hints(transcripts: &[String]) -> Vec<String> {
    let mut candidates: HashMap<String, Candidate> = HashMap::new();
    let mut order = 0usize;

    for (recency_index, transcript) in transcripts.iter().enumerate() {
        for sentence in split_sentences(transcript) {
            if contains_sensitive_marker(sentence) {
                continue;
            }
            let tokens = tokenize(sentence);
            if tokens.is_empty() {
                continue;
            }

            for idx in 0..tokens.len() {
                let score = token_signal_score(&tokens[idx]);
                if score >= 8 {
                    add_candidate(
                        &mut candidates,
                        &tokens[idx].text,
                        score + recency_bonus(recency_index),
                        order,
                    );
                    order += 1;
                }
            }

            for start in 0..tokens.len() {
                for len in 2..=4 {
                    let end = start + len;
                    if end > tokens.len() {
                        continue;
                    }
                    if let Some((phrase, score)) = phrase_candidate(&tokens[start..end]) {
                        add_candidate(
                            &mut candidates,
                            &phrase,
                            score + recency_bonus(recency_index),
                            order,
                        );
                        order += 1;
                    }
                }
            }
        }
    }

    let mut ranked = candidates.into_values().collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.order.cmp(&b.order))
            .then_with(|| a.text.chars().count().cmp(&b.text.chars().count()))
    });

    compact_to_prompt_budget(ranked)
}

fn split_sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| matches!(c, '\n' | '.' | '!' | '?' | ';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn tokenize(sentence: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in sentence.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '/' | '#' | '+') {
            current.push(ch);
        } else if !current.is_empty() {
            push_token(&mut tokens, &mut current);
        }
    }
    if !current.is_empty() {
        push_token(&mut tokens, &mut current);
    }

    tokens
}

fn push_token(tokens: &mut Vec<Token>, current: &mut String) {
    let text = current
        .trim_matches(|c: char| {
            c.is_ascii_punctuation() && !matches!(c, '-' | '_' | '/' | '#' | '+')
        })
        .to_string();
    current.clear();
    if text.is_empty() {
        return;
    }
    tokens.push(Token {
        lower: text.to_lowercase(),
        text,
    });
}

fn phrase_candidate(tokens: &[Token]) -> Option<(String, i32)> {
    let mut start = 0usize;
    let mut end = tokens.len();
    while start < end && is_stopword(&tokens[start].lower) {
        start += 1;
    }
    while end > start && is_stopword(&tokens[end - 1].lower) {
        end -= 1;
    }
    if end <= start {
        return None;
    }

    let slice = &tokens[start..end];
    let meaningful = slice
        .iter()
        .filter(|t| !is_stopword(&t.lower) || is_domain_keyword(&t.lower))
        .count();
    if meaningful < 2 {
        return None;
    }

    let signal_score: i32 = slice.iter().map(token_signal_score).sum();
    if signal_score <= 0 {
        return None;
    }

    let phrase = slice
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if phrase.chars().count() > 60 {
        return None;
    }

    let length_bonus = match slice.len() {
        2 => 4,
        3 => 3,
        _ => 1,
    };
    Some((phrase, signal_score + length_bonus + meaningful as i32))
}

fn add_candidate(
    candidates: &mut HashMap<String, Candidate>,
    raw_text: &str,
    score: i32,
    order: usize,
) {
    let text = raw_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if text.is_empty() {
        return;
    }
    let key = text.to_lowercase();
    match candidates.get_mut(&key) {
        Some(existing) if score > existing.score => {
            existing.score = score;
            existing.order = existing.order.min(order);
        }
        Some(_) => {}
        None => {
            candidates.insert(key, Candidate { text, score, order });
        }
    }
}

fn compact_to_prompt_budget(ranked: Vec<Candidate>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut used_chars = 0usize;

    for candidate in ranked {
        let text = candidate.text.trim();
        if text.is_empty() || !seen.insert(text.to_lowercase()) {
            continue;
        }
        let text_chars = text.chars().count();
        let separator_chars = if out.is_empty() { 0 } else { 2 };
        if used_chars + separator_chars + text_chars > RECENT_SPEECH_HINTS_MAX_CHARS {
            continue;
        }
        used_chars += separator_chars + text_chars;
        out.push(text.to_string());
    }

    out
}

fn token_signal_score(token: &Token) -> i32 {
    let text = token.text.as_str();
    let lower = token.lower.as_str();
    if is_stopword(lower) && !is_domain_keyword(lower) {
        return 0;
    }

    let mut score = 0;
    if is_domain_keyword(lower) {
        score += 6;
    }
    if text.chars().any(|c| c.is_ascii_digit()) {
        score += 6;
    }
    if text.contains('-') || text.contains('_') || text.contains('/') || text.contains('#') {
        score += 6;
    }
    if text.chars().any(|c| c.is_ascii_uppercase())
        && text.chars().any(|c| c.is_ascii_lowercase())
        && text.chars().count() >= 4
    {
        score += 4;
    }
    if text.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2 {
        score += 5;
    }
    if lower.chars().count() >= 8 {
        score += 3;
    }
    score
}

fn recency_bonus(index: usize) -> i32 {
    match index {
        0 => 3,
        1 => 1,
        _ => 0,
    }
}

fn contains_sensitive_marker(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    SENSITIVE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

fn is_domain_keyword(token: &str) -> bool {
    DOMAIN_KEYWORDS.contains(&token)
}

const SENSITIVE_MARKERS: &[&str] = &[
    "password",
    "passcode",
    "otp",
    "one time password",
    "secret",
    "private key",
    "api key",
    "access token",
    "bearer token",
    "credit card",
    "card number",
    "cvv",
    "ssn",
    "salary",
    "bank account",
    "phone number",
    "home address",
];

const DOMAIN_KEYWORDS: &[&str] = &[
    "app",
    "apps",
    "backend",
    "brand",
    "brands",
    "cloud",
    "context",
    "dictation",
    "dictations",
    "hallucinated",
    "hint",
    "hints",
    "local",
    "meaning",
    "model",
    "name",
    "names",
    "polish",
    "profile",
    "prompt",
    "recent",
    "speech",
    "support",
    "term",
    "terms",
    "topic",
    "topics",
    "transcript",
    "ttl",
    "vocab",
    "vocabulary",
    "wins",
];

const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "agar", "all", "also", "an", "and", "any", "are", "as", "at",
    "aur", "bas", "be", "been", "before", "being", "bhai", "bhi", "bilkul", "but", "by", "can",
    "could", "did", "do", "does", "doing", "done", "ek", "for", "from", "give", "hai", "hain",
    "have", "him", "his", "ho", "how", "i", "if", "in", "into", "is", "it", "its", "just", "ka",
    "kab", "kar", "karna", "karo", "ke", "ki", "ko", "kyu", "like", "main", "make", "mein",
    "mujhe", "nai", "nahi", "no", "not", "of", "on", "only", "or", "par", "please", "rakho",
    "same", "se", "should", "so", "take", "that", "the", "then", "these", "this", "those", "to",
    "use", "used", "using", "was", "were", "what", "when", "will", "with", "would", "yaar", "yeh",
    "you",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_short_terms_without_copying_sensitive_sentences() {
        let transcripts = vec![
            "My password is blue tiger and my salary is private. Recent speech hints block, same app dictations, current transcript wins, hallucinated names brand.".to_string(),
        ];

        let hints = extract_recent_speech_hints(&transcripts);
        let joined = hints.join(" | ").to_lowercase();

        assert!(joined.contains("recent speech hints"));
        assert!(joined.contains("current transcript"));
        assert!(joined.contains("hallucinated names"));
        assert!(!joined.contains("password"));
        assert!(!joined.contains("blue tiger"));
        assert!(!joined.contains("salary"));
    }

    #[test]
    fn caps_hints_to_prompt_budget() {
        let transcripts = vec![
            ((0..80)
                .map(|i| format!("SuperSpecificModelName{i} transcript context"))
                .collect::<Vec<_>>()
                .join(". ")),
        ];

        let hints = extract_recent_speech_hints(&transcripts);
        assert!(hints.join(", ").chars().count() <= RECENT_SPEECH_HINTS_MAX_CHARS);
    }
}
