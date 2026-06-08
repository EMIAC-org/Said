//! Deterministic post-LLM formatting guard for the server runtime.
//!
//! Normal voice only uses the email-focused pass. Wider URL/path/env recovery
//! stays out of the server voice hot path until it has separate parity tests.

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailRecovery {
    pub observed: String,
    pub canonical: String,
}

/// Email-only recovery. Safe for the live voice path because it only folds
/// tightly anchored email forms around `at` / `at the rate` and an email domain.
pub fn recover_emails(text: &str) -> String {
    let s = recover_spoken_emails(text);
    compact_local_before_at(&s)
}

pub fn recover_emails_with_candidates(
    text: &str,
    canonical_candidates: &[String],
) -> (String, Vec<EmailRecovery>) {
    let mut result = recover_emails(text);
    let mut recoveries = Vec::new();
    if canonical_candidates.is_empty() {
        return (result, recoveries);
    }

    let spans = email_spans(&result);
    for (start, end, observed) in spans.into_iter().rev() {
        let Some(best) = best_email_candidate(&observed, canonical_candidates) else {
            continue;
        };
        if best == observed {
            continue;
        }
        result.replace_range(start..end, &best);
        recoveries.push(EmailRecovery {
            observed,
            canonical: best,
        });
    }
    recoveries.reverse();
    (result, recoveries)
}

static EMAIL_SEP: Lazy<Regex> = Lazy::new(|| {
    let tld = r"com|in|org|io|net|co|app|dev|ai|me|us|uk|edu|gov";
    Regex::new(&format!(
        r"(?ix)
        (?:
            \b at \s+ the \s+ rate \s+
          | \b at \s+
        )
        (?P<domain>
            (?:
                (?: g \s* mail | gee \s* mail | gmail | google \s* mail | yahoo | outlook | hotmail | icloud )
                \s* (?: \. | \b dot \b ) \s* (?: com )?
            )
          |
            (?:
                [A-Za-z0-9]+
                (?: \s* (?: \. | \b dot \b ) \s* [A-Za-z0-9]+ )*
                \s* (?: \. | \b dot \b ) \s*
                (?: {tld} )
                (?: \s* (?: \. | \b dot \b ) \s* (?: {tld} ) )?
            )
        )
        \b
        "
    ))
    .unwrap()
});

static LOCAL_FILLER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:dot|at the rate|at)\b").unwrap());

static EMAIL_ADDR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}").unwrap());

static AT_DOMAIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"@[A-Za-z0-9]+(?:\.[A-Za-z0-9]+)+").unwrap());

const MAX_LOCAL_TOKENS: usize = 8;

fn recover_spoken_emails(text: &str) -> String {
    let mut result = text.to_string();
    let matches: Vec<_> = EMAIL_SEP.find_iter(text).collect();
    for m in matches.into_iter().rev() {
        let sep_start = m.start();
        let domain_raw = EMAIL_SEP
            .captures(&result[sep_start..])
            .and_then(|c| c.name("domain"))
            .map(|d| d.as_str().to_string())
            .unwrap_or_default();

        let domain = compact_domain(&domain_raw);
        if !domain.contains('.') || domain.chars().any(char::is_whitespace) {
            continue;
        }

        let before = &result[..sep_start];
        let words: Vec<&str> = before.split_whitespace().collect();
        let mut local_tokens: Vec<&str> = Vec::new();

        for &w in words.iter().rev().take(MAX_LOCAL_TOKENS) {
            if w.ends_with('.') || w.ends_with('?') || w.ends_with('!') || w.ends_with(',') {
                break;
            }
            let stripped = w.trim_matches(|c: char| !c.is_alphanumeric());
            if stripped.is_empty() || is_email_stop_word(stripped) {
                break;
            }
            local_tokens.push(w);
        }

        if local_tokens.is_empty() {
            continue;
        }
        local_tokens.reverse();

        let local_clean = fold_email_local_tokens(&local_tokens);
        if local_clean.is_empty() {
            continue;
        }

        let local_phrase = local_tokens.join(" ");
        let local_byte_start = before
            .rfind(&local_phrase)
            .unwrap_or_else(|| before.rfind(local_tokens[0]).unwrap_or(sep_start));
        let prefix = &result[..local_byte_start];
        let suffix = &result[sep_start..][m.as_str().len()..];
        result = format!("{prefix}{local_clean}@{domain}{suffix}");
    }
    result
}

fn compact_local_before_at(text: &str) -> String {
    let positions: Vec<(usize, usize)> = AT_DOMAIN
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();

    let mut result = text.to_string();
    for &(at_pos, domain_end) in positions.iter().rev() {
        let domain = result[at_pos + 1..domain_end].to_string();
        let before = &result[..at_pos];
        let word_spans = word_spans(before);
        let mut fragments: Vec<(usize, &str)> = Vec::new();

        for &(pos, w) in word_spans.iter().rev().take(MAX_LOCAL_TOKENS) {
            if w.ends_with('.') || w.ends_with('?') || w.ends_with('!') || w.ends_with(',') {
                break;
            }
            let stripped = w.trim_matches(|c: char| !c.is_alphanumeric());
            if stripped.is_empty() || is_email_stop_word(stripped) {
                break;
            }
            fragments.push((pos, stripped));
        }

        fragments.reverse();
        if fragments.len() < 2 {
            continue;
        }

        let local_fragments: Vec<&str> = fragments.iter().map(|(_, frag)| *frag).collect();
        let local = fold_email_local_tokens(&local_fragments);
        if local.chars().all(|c| c == '.') || local.is_empty() {
            continue;
        }

        let first_byte = fragments[0].0;
        let prefix = result[..first_byte].trim_end();
        let suffix = &result[domain_end..];
        let sep = if prefix.is_empty() { "" } else { " " };
        result = format!("{prefix}{sep}{local}@{domain}{suffix}");
    }

    result
}

fn word_spans(text: &str) -> Vec<(usize, &str)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                spans.push((s, &text[s..idx]));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        spans.push((s, &text[s..]));
    }
    spans
}

fn compact_domain(raw: &str) -> String {
    let dotted = LOCAL_FILLER.replace_all(raw, ".");
    let mut out = String::with_capacity(dotted.len());
    for ch in dotted.chars() {
        if ch.is_whitespace() {
            continue;
        }
        out.extend(ch.to_lowercase());
    }
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    let out = out.trim_matches('.').to_string();
    match out.as_str() {
        "gmail" | "gmailcom" | "gemail" | "gmaildot" | "googlemail" => "gmail.com".to_string(),
        "yahoo" => "yahoo.com".to_string(),
        "outlook" => "outlook.com".to_string(),
        "hotmail" => "hotmail.com".to_string(),
        "icloud" => "icloud.com".to_string(),
        _ => out,
    }
}

fn fold_email_local_tokens(tokens: &[&str]) -> String {
    let mut out = String::new();
    for token in tokens {
        let stripped = token.trim_matches(|c: char| !c.is_alphanumeric());
        if stripped.is_empty() {
            continue;
        }
        let lower = stripped.to_ascii_lowercase();
        match lower.as_str() {
            "dot" => {
                if !out.is_empty() && !out.ends_with('.') {
                    out.push('.');
                }
            }
            "underscore" => {
                if !out.is_empty() && !out.ends_with('_') {
                    out.push('_');
                }
            }
            "hyphen" | "dash" => {
                if !out.is_empty() && !out.ends_with('-') {
                    out.push('-');
                }
            }
            _ => {
                if let Some(digit) = email_digit_word(stripped) {
                    out.push_str(digit);
                } else {
                    out.extend(
                        stripped
                            .chars()
                            .filter(|c| c.is_alphanumeric())
                            .flat_map(|c| c.to_lowercase()),
                    );
                }
            }
        }
    }
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    out.trim_matches('.').to_string()
}

fn is_email_stop_word(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    if w.len() <= 1 {
        let ch = word.chars().next().unwrap_or(' ');
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return false;
        }
        return true;
    }
    matches!(
        w.as_str(),
        "mail"
            | "email"
            | "e-mail"
            | "send"
            | "to"
            | "ping"
            | "drop"
            | "shoot"
            | "forward"
            | "fwd"
            | "cc"
            | "bcc"
            | "write"
            | "message"
            | "from"
            | "reply"
            | "is"
            | "my"
            | "his"
            | "her"
            | "the"
            | "an"
            | "and"
            | "or"
            | "for"
            | "in"
            | "on"
            | "of"
            | "that"
            | "this"
            | "with"
            | "it"
            | "its"
            | "but"
            | "not"
            | "are"
            | "was"
            | "were"
            | "will"
            | "can"
            | "do"
            | "did"
            | "has"
            | "have"
            | "had"
            | "been"
            | "be"
            | "so"
            | "if"
            | "as"
            | "at"
            | "by"
            | "no"
            | "yes"
            | "also"
            | "all"
            | "get"
            | "got"
            | "just"
            | "now"
            | "here"
            | "there"
            | "then"
            | "than"
            | "too"
            | "very"
            | "only"
            | "how"
            | "what"
            | "when"
            | "where"
            | "who"
            | "why"
            | "which"
            | "would"
            | "could"
            | "should"
            | "may"
            | "might"
            | "must"
            | "shall"
            | "ko"
            | "ka"
            | "ki"
            | "ke"
            | "hai"
            | "ho"
            | "hain"
            | "tha"
            | "thi"
            | "kya"
            | "kaise"
            | "kab"
            | "par"
            | "se"
            | "ne"
            | "bhi"
            | "toh"
            | "aur"
            | "lekin"
            | "yaar"
            | "bhai"
            | "mein"
            | "main"
            | "hum"
            | "tum"
            | "woh"
            | "yeh"
            | "ye"
            | "ab"
            | "jab"
            | "tab"
            | "sab"
            | "kuch"
            | "bahut"
            | "bohot"
            | "nahi"
            | "na"
            | "mat"
            | "kar"
            | "karo"
            | "karna"
            | "hona"
            | "wahan"
            | "yahan"
            | "pata"
            | "chal"
            | "jaao"
            | "jao"
            | "isko"
            | "usko"
            | "apna"
            | "apne"
            | "apni"
            | "unka"
            | "uska"
            | "iski"
            | "inhe"
            | "unhe"
            | "mera"
            | "tera"
            | "tumhara"
            | "hamara"
            | "diya"
            | "hua"
            | "liya"
            | "dena"
            | "lena"
            | "raha"
            | "rahe"
            | "rahi"
            | "ek"
            | "teen"
            | "please"
            | "kindly"
            | "check"
            | "open"
            | "see"
            | "look"
    )
}

fn email_digit_word(word: &str) -> Option<&'static str> {
    match word.to_ascii_lowercase().as_str() {
        "zero" | "shunya" => Some("0"),
        "one" | "ek" => Some("1"),
        "two" | "do" => Some("2"),
        "three" | "teen" => Some("3"),
        "four" | "char" | "chaar" => Some("4"),
        "five" | "paanch" | "panch" => Some("5"),
        "six" | "chheh" | "chah" | "cheh" | "chhe" | "che" => Some("6"),
        "seven" | "saat" => Some("7"),
        "eight" | "aath" => Some("8"),
        "nine" | "nau" => Some("9"),
        _ => None,
    }
}

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

fn best_email_candidate(observed: &str, candidates: &[String]) -> Option<String> {
    let observed_norm = compact_email(observed)?;
    let (observed_local, observed_domain) = split_email(observed)?;
    let mut best: Option<(String, f64)> = None;
    for candidate in candidates {
        let Some(candidate_norm) = compact_email(candidate) else {
            continue;
        };
        let Some((candidate_local, candidate_domain)) = split_email(candidate) else {
            continue;
        };
        let all_score = edit_similarity(&observed_norm, &candidate_norm);
        let local_score = edit_similarity(
            &compact_token(observed_local),
            &compact_token(candidate_local),
        );
        let domain_score = if observed_domain.eq_ignore_ascii_case(candidate_domain) {
            1.0
        } else {
            edit_similarity(
                &compact_token(observed_domain),
                &compact_token(candidate_domain),
            )
        };
        let score = (all_score * 0.45) + (local_score * 0.35) + (domain_score * 0.20);
        let strong_same_domain = domain_score >= 0.98 && local_score >= 0.86;
        let strong_overall = score >= 0.90;
        if !strong_same_domain && !strong_overall {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(_, best_score)| score > *best_score)
        {
            best = Some((candidate.clone(), score));
        }
    }
    best.map(|(candidate, _)| candidate)
}

fn split_email(email: &str) -> Option<(&str, &str)> {
    let (local, domain) = email.split_once('@')?;
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    Some((local, domain))
}

fn compact_email(email: &str) -> Option<String> {
    split_email(email)?;
    Some(compact_token(email))
}

fn compact_token(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn edit_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let d = levenshtein_chars(&a_chars, &b_chars) as f64;
    1.0 - d / (a_chars.len().max(b_chars.len()) as f64)
}

fn levenshtein_chars(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_missing_gmail_com_tld_defaults_to_gmail() {
        assert_eq!(
            recover_emails(
                "Mera jo email hai, voh hai V abhi dot Verma two six seven eight at the rate gmail dot."
            ),
            "Mera jo email hai, voh hai vabhi.verma2678@gmail.com."
        );
    }

    #[test]
    fn email_with_verb_prefix() {
        assert_eq!(
            recover_emails("Mail Anish Suman 2305 at the rate gmail dot com."),
            "Mail anishsuman2305@gmail.com."
        );
    }

    #[test]
    fn email_memory_canonicalizes_close_user_email() {
        let (out, recoveries) = recover_emails_with_candidates(
            "Mera email vabhi.verma2678@gmail.com hai.",
            &["v.abhi.verma2678@gmail.com".to_string()],
        );
        assert_eq!(out, "Mera email v.abhi.verma2678@gmail.com hai.");
        assert_eq!(recoveries.len(), 1);
    }

    #[test]
    fn catastrophic_hinglish_prose_not_folded() {
        let input = "Hello bhai, kaise ho? Kya kar rahe ho? Kitna are GitHub par to jaao. Wahan par sab pata chal jaayega. Anish Suman at the rate Gmail dot com ko sab diya hua hai bhai.";
        let out = recover_emails(input);
        assert!(out.contains("anishsuman@gmail.com"), "got: {out}");
        assert!(out.contains("Hello bhai"), "got: {out}");
        assert!(out.contains("jaayega."), "got: {out}");
    }

    #[test]
    fn prose_at_the_rate_of_percent_stays_prose() {
        let unchanged = "Growing at the rate of 10% every year.";
        assert_eq!(recover_emails(unchanged), unchanged);
    }
}
