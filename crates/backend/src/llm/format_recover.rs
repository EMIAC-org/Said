//! Deterministic post-LLM formatting guard.
//!
//! Runs on the final polished string, AFTER the LLM and AFTER the Devanagari
//! romanizer.  Each pass does a **surgical find-and-replace** — it locates a
//! specific spoken-form anchor ("at the rate", "dot", "slash", "underscore",
//! "colon") and folds only the immediate neighbours.  Plain prose is never
//! touched; if no pattern matches, the string passes through with zero
//! allocations.
//!
//! Design rule: the local part of an email match is CAPPED at 5 tokens
//! (backward from the separator).  This prevents the catastrophic greediness
//! bug where an entire Hinglish sentence got folded into one giant email.

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailRecovery {
    pub observed: String,
    pub canonical: String,
}

/// Apply all recovery passes. Idempotent. Safe to call on any string.
pub fn recover(text: &str) -> String {
    let s = recover_protocol_mishears(text);
    let s = recover_emails(&s);
    let s = recover_spoken_urls(&s);
    let s = recover_spoken_file_paths(&s);
    let s = recover_standalone_slash(&s);
    let s = recover_spoken_identifiers(&s);
    recover_spoken_env_vars(&s)
}

/// Email-only recovery. Safe for the live voice path because it does not fold
/// URLs, paths, env vars, or other structured tokens.
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

pub fn extract_emails(text: &str) -> Vec<String> {
    email_spans(text)
        .into_iter()
        .map(|(_, _, email)| email)
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 1 — Protocol mishear recovery
// ─────────────────────────────────────────────────────────────────────────────

static PROTOCOL_MISHEAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b
        (?:
            hatps |
            achtps |
            aichtps |
            htps |
            http \s+ s |
            h \s+ t \s+ t \s+ p \s+ s
        )
        ( :// )
        ",
    )
    .unwrap()
});

fn recover_protocol_mishears(text: &str) -> String {
    PROTOCOL_MISHEAR.replace_all(text, "https$1").into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 2 — Spoken-form email recovery (tight, max 5 local tokens)
// ─────────────────────────────────────────────────────────────────────────────

/// Anchors on the separator ("at the rate" / "at <known-domain>") then looks
/// FORWARD for "domain dot tld" and BACKWARD for at most 5 name/digit tokens.
/// This cap prevents the catastrophic greediness bug.

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

fn is_email_stop_word(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    // Single uppercase letters are likely initials (V, A, etc.) — allow them.
    // Single lowercase or non-alpha are stop words.
    if w.len() <= 1 {
        let ch = word.chars().next().unwrap_or(' ');
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return false; // initial or digit — not a stop word
        }
        return true;
    }
    matches!(
        w.as_str(),
        // English function words
        "mail" | "email" | "e-mail" | "send" | "to" | "ping" | "drop"
            | "shoot" | "forward" | "fwd" | "cc" | "bcc" | "write"
            | "message" | "from" | "reply" | "is" | "my" | "his" | "her"
            | "the" | "an" | "and" | "or" | "for" | "in" | "on"
            | "of" | "that" | "this" | "with" | "it" | "its"
            | "but" | "not" | "are" | "was" | "were" | "will" | "can"
            | "do" | "did" | "has" | "have" | "had" | "been" | "be"
            | "so" | "if" | "as" | "at" | "by" | "no" | "yes" | "also"
            | "all" | "get" | "got" | "just" | "now" | "here" | "there"
            | "then" | "than" | "too" | "very" | "only" | "how" | "what"
            | "when" | "where" | "who" | "why" | "which" | "would"
            | "could" | "should" | "may" | "might" | "must" | "shall"
            // Hindi/Hinglish function words
            | "ko" | "ka" | "ki" | "ke" | "hai" | "ho" | "hain" | "tha"
            | "the" | "thi" | "kya" | "kaise" | "kab" | "par" | "se"
            | "ne" | "bhi" | "toh" | "aur" | "lekin" | "yaar" | "bhai"
            | "mein" | "main" | "hum" | "tum" | "woh" | "yeh" | "ye"
            | "ab" | "jab" | "tab" | "sab" | "kuch" | "bahut" | "bohot"
            | "nahi" | "na" | "mat" | "kar" | "karo" | "karna" | "hona"
            | "wahan" | "yahan" | "pata" | "chal" | "jaao" | "jao"
            | "isko" | "usko" | "apna" | "apne" | "apni" | "unka"
            | "uska" | "iski" | "inhe" | "unhe" | "mera" | "tera"
            | "tumhara" | "hamara" | "diya" | "hua" | "liya" | "dena"
            | "lena" | "raha" | "rahe" | "rahi" | "ek" | "do" | "teen"
            | "please" | "kindly" | "check" | "open" | "see" | "look"
    )
}

const MAX_LOCAL_TOKENS: usize = 8;

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

fn recover_spoken_emails(text: &str) -> String {
    let mut result = text.to_string();
    // Process from right to left to keep indices stable
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

        // Scan backward from sep_start for up to MAX_LOCAL_TOKENS
        let before = &result[..sep_start];
        let words: Vec<&str> = before.split_whitespace().collect();
        let mut local_tokens: Vec<&str> = Vec::new();

        for &w in words.iter().rev().take(MAX_LOCAL_TOKENS) {
            // Stop at sentence boundaries — word ending with . ? !
            if w.ends_with('.') || w.ends_with('?') || w.ends_with('!') || w.ends_with(',') {
                break;
            }
            let stripped = w.trim_matches(|c: char| !c.is_alphanumeric());
            if stripped.is_empty() || is_email_stop_word(stripped) {
                break;
            }
            if stripped.eq_ignore_ascii_case("dot") {
                local_tokens.push(w);
                continue;
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

        // Find the actual start position of the full local-token phrase.
        // Searching for only the first token is unsafe for initials like
        // "V abhi dot Verma": a plain rfind("V") lands inside "Verma".
        let local_phrase = local_tokens.join(" ");
        let local_byte_start = before
            .rfind(&local_phrase)
            .unwrap_or_else(|| before.rfind(local_tokens[0]).unwrap_or(sep_start));

        let prefix = &result[..local_byte_start];
        let suffix = &result[sep_start..][m.as_str().len()..];
        // Preserve any leading space that was before local tokens
        let space = if !prefix.is_empty() && !prefix.ends_with(' ') {
            ""
        } else {
            ""
        };
        result = format!("{prefix}{space}{local_clean}@{domain}{suffix}");
    }
    result
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

static EMAIL_ADDR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}").unwrap());

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

// ─────────────────────────────────────────────────────────────────────────────
// Pass 3 — Compact fragmented local parts before an existing @
// ─────────────────────────────────────────────────────────────────────────────

static AT_DOMAIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"@[A-Za-z0-9]+(?:\.[A-Za-z0-9]+)+").unwrap());

fn compact_local_before_at(text: &str) -> String {
    let positions: Vec<(usize, usize)> = AT_DOMAIN
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();

    let mut result = text.to_string();

    for &(at_pos, domain_end) in positions.iter().rev() {
        let domain = result[at_pos + 1..domain_end].to_string();
        let before = &result[..at_pos];

        // Collect word spans (start_byte, word) from `before`
        let word_spans: Vec<(usize, &str)> = before
            .match_indices(|c: char| !c.is_whitespace())
            .fold(Vec::new(), |mut acc, (i, _)| {
                if acc.is_empty() || {
                    let (prev_start, prev_word): (usize, &str) = *acc.last().unwrap();
                    prev_start + prev_word.len() < i
                } {
                    let end = before[i..]
                        .find(char::is_whitespace)
                        .map_or(before.len(), |e| i + e);
                    acc.push((i, &before[i..end]));
                }
                acc
            });

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

// ─────────────────────────────────────────────────────────────────────────────
// Pass 4 — Spoken-form URL recovery
// ─────────────────────────────────────────────────────────────────────────────

static SPOKEN_URL: Lazy<Regex> = Lazy::new(|| {
    let tld = r"com|in|org|io|net|co|app|dev|ai|me|us|uk|edu|gov";
    Regex::new(&format!(
        r"(?ix)
        (?P<host>
            localhost
          | \d{{1,3}} (?: \. \d{{1,3}} ){{3}}
          | [A-Za-z0-9-]+
            (?: \s* (?: \. | \b dot \b ) \s* [A-Za-z0-9-]+ )*
            \s* (?: \. | \b dot \b ) \s*
            (?:{tld})
        )
        (?P<port>
            \s+ colon \s+ \d+
        )?
        (?P<path>
            (?: \s+ slash \s+ [A-Za-z0-9_.~-]+ ){{1,10}}
        )
        "
    ))
    .unwrap()
});

fn recover_spoken_urls(text: &str) -> String {
    SPOKEN_URL
        .replace_all(text, |caps: &regex::Captures| {
            let host_raw = caps.name("host").unwrap().as_str();
            let port_raw = caps.name("port").map_or("", |m| m.as_str());
            let path_raw = caps.name("path").map_or("", |m| m.as_str());

            let host = LOCAL_FILLER
                .replace_all(host_raw, ".")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("")
                .to_lowercase();
            let host = host.replace("..", ".");

            let port = if port_raw.is_empty() {
                String::new()
            } else {
                let digits: String = port_raw.chars().filter(|c| c.is_ascii_digit()).collect();
                format!(":{digits}")
            };

            let path: String = path_raw
                .split_whitespace()
                .filter(|w| !w.eq_ignore_ascii_case("slash"))
                .map(|w| format!("/{w}"))
                .collect();

            format!("{host}{port}{path}")
        })
        .into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 5 — Spoken-form file paths ("dot slash" prefix)
// ─────────────────────────────────────────────────────────────────────────────

static SPOKEN_FILE_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b dot \s+ slash \s+
        (?P<path>
            [A-Za-z0-9_.~-]+
            (?: \s+ (?:slash|dot) \s+ [A-Za-z0-9_.~-]+ )*
        )
        \b
        ",
    )
    .unwrap()
});

fn recover_spoken_file_paths(text: &str) -> String {
    SPOKEN_FILE_PATH
        .replace_all(text, |caps: &regex::Captures| {
            let path_raw = caps.name("path").unwrap().as_str();
            let mut out = String::from("./");
            for token in path_raw.split_whitespace() {
                if token.eq_ignore_ascii_case("slash") {
                    out.push('/');
                } else if token.eq_ignore_ascii_case("dot") {
                    out.push('.');
                } else {
                    out.push_str(token);
                }
            }
            out
        })
        .into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 5b — Standalone "slash" → "/" before path-like words
// ─────────────────────────────────────────────────────────────────────────────

/// Matches "slash <word>" where <word> looks like a route/path segment.
/// Only fires when nearby context suggests a path (URL, route, endpoint,
/// path, api, etc.) or when followed by more "slash <word>" chains.
static STANDALONE_SLASH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b slash \s+
        (?P<seg> [A-Za-z0-9_.-]+ )
        (?P<tail>
            (?: \s+ slash \s+ [A-Za-z0-9_.-]+ )*
        )
        ",
    )
    .unwrap()
});

fn recover_standalone_slash(text: &str) -> String {
    STANDALONE_SLASH
        .replace_all(text, |caps: &regex::Captures| {
            let full_match = caps.get(0).unwrap().as_str();
            let seg = caps.name("seg").unwrap().as_str();
            let tail_raw = caps.name("tail").map_or("", |m| m.as_str());

            let has_multiple_slashes = !tail_raw.trim().is_empty();

            let match_start = caps.get(0).unwrap().start();
            let context_before = &text[..match_start];
            let context_after_end = (caps.get(0).unwrap().end() + 40).min(text.len());
            let context_after = &text[caps.get(0).unwrap().end()..context_after_end];
            let nearby = format!(
                "{} {}",
                context_before
                    .split_whitespace()
                    .rev()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(" "),
                context_after,
            )
            .to_ascii_lowercase();

            let path_context = nearby.contains("url")
                || nearby.contains("path")
                || nearby.contains("route")
                || nearby.contains("endpoint")
                || nearby.contains("api")
                || nearby.contains("http")
                || nearby.contains("localhost")
                || nearby.contains("meeting");

            if has_multiple_slashes || path_context {
                let mut out = format!("/{seg}");
                for token in tail_raw.split_whitespace() {
                    if token.eq_ignore_ascii_case("slash") {
                        out.push('/');
                    } else {
                        out.push_str(token);
                    }
                }
                out
            } else {
                full_match.to_string()
            }
        })
        .into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 6 — Spoken-form identifiers (underscore, hyphen)
// ─────────────────────────────────────────────────────────────────────────────

static SPOKEN_IDENT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b
        (?P<chain>
            [A-Za-z0-9]+
            (?:
                \s+ (?:underscore|hyphen|dash) \s+
                [A-Za-z0-9]+
            )+
        )
        \b
        ",
    )
    .unwrap()
});

fn recover_spoken_identifiers(text: &str) -> String {
    SPOKEN_IDENT
        .replace_all(text, |caps: &regex::Captures| {
            let chain = caps.name("chain").unwrap().as_str();

            let has_ident_word = chain.split_whitespace().any(|w| {
                let lower = w.to_ascii_lowercase();
                if lower == "underscore" || lower == "hyphen" || lower == "dash" {
                    return false;
                }
                w.len() >= 3 || w.chars().any(|c| c.is_ascii_digit())
            });
            if !has_ident_word {
                return chain.to_string();
            }

            let mut out = String::with_capacity(chain.len());
            for token in chain.split_whitespace() {
                let lower = token.to_ascii_lowercase();
                if lower == "underscore" {
                    out.push('_');
                    continue;
                }
                if lower == "hyphen" || lower == "dash" {
                    out.push('-');
                    continue;
                }
                out.push_str(token);
            }
            out
        })
        .into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 7 — Spoken-form env vars ("KEY equals VALUE")
// ─────────────────────────────────────────────────────────────────────────────

static SPOKEN_EQUALS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
        \b
        (?P<key> [A-Z][A-Z0-9_]{2,} )
        \s+ equals \s+
        (?P<val> [A-Za-z0-9_.-]+ )
        \b
        ",
    )
    .unwrap()
});

fn recover_spoken_env_vars(text: &str) -> String {
    SPOKEN_EQUALS
        .replace_all(text, |caps: &regex::Captures| {
            let key = caps.name("key").unwrap().as_str();
            let val = caps.name("val").unwrap().as_str();
            format!("{key}={val}")
        })
        .into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Protocol mishears ─────────────────────────────────────────────────

    #[test]
    fn protocol_hatps() {
        assert_eq!(
            recover("Open HATPS://religwav.com."),
            "Open https://religwav.com."
        );
    }

    #[test]
    fn protocol_achtps() {
        assert_eq!(
            recover("Visit ACHTPS://google.co.in for results."),
            "Visit https://google.co.in for results."
        );
    }

    #[test]
    fn protocol_leaves_real_https_alone() {
        assert_eq!(recover("https://example.com"), "https://example.com");
    }

    // ── Spoken-form emails (Pass 2 — tight) ──────────────────────────────

    #[test]
    fn email_name_at_the_rate_domain() {
        assert_eq!(
            recover("Anish Suman 2305 at the rate gmail dot com"),
            "anishsuman2305@gmail.com"
        );
    }

    #[test]
    fn email_v_abhi_dot_verma() {
        let out = recover("V abhi dot verma 2678 at the rate Gmail dot com.");
        assert!(
            out.contains("@gmail.com"),
            "email domain must be folded, got: {out}"
        );
        assert!(
            !out.contains("at the rate"),
            "'at the rate' must be replaced, got: {out}"
        );
    }

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
    fn email_memory_canonicalizes_close_user_email() {
        let (out, recoveries) = recover_emails_with_candidates(
            "Mera email vabhi.verma2678@gmail.com hai.",
            &["v.abhi.verma2678@gmail.com".to_string()],
        );
        assert_eq!(out, "Mera email v.abhi.verma2678@gmail.com hai.");
        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].canonical, "v.abhi.verma2678@gmail.com");
    }

    #[test]
    fn email_with_verb_prefix() {
        assert_eq!(
            recover("Mail Anish Suman 2305 at the rate gmail dot com."),
            "Mail anishsuman2305@gmail.com."
        );
    }

    #[test]
    fn email_at_alone() {
        assert_eq!(
            recover("Mail anish at gmail dot com."),
            "Mail anish@gmail.com."
        );
    }

    #[test]
    fn email_already_folded() {
        let unchanged = "Send to anish@gmail.com.";
        assert_eq!(recover(unchanged), unchanged);
    }

    // ── CRITICAL: long Hinglish prose must NOT get folded into email ──────

    #[test]
    fn catastrophic_hinglish_prose_not_folded() {
        let input = "Hello bhai, kaise ho? Kya kar rahe ho? Kitna are GitHub par to jaao. Wahan par sab pata chal jaayega. Anish Suman at the rate Gmail dot com ko sab diya hua hai bhai.";
        let out = recover(input);
        // Only "Anish Suman" should be in the local part, NOT the entire sentence
        assert!(
            out.contains("anishsuman@gmail.com"),
            "should fold only the name near 'at the rate', got: {out}"
        );
        assert!(
            out.contains("Hello bhai"),
            "prose before the email must be preserved, got: {out}"
        );
        assert!(
            out.contains("jaayega."),
            "sentence before email must end properly, got: {out}"
        );
    }

    // ── Negative cases — prose stays prose ────────────────────────────────

    #[test]
    fn prose_at_the_rate_of_percent() {
        let unchanged = "Growing at the rate of 10% every year.";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn prose_at_without_dot_tld() {
        let unchanged = "Meet me at home tomorrow.";
        assert_eq!(recover(unchanged), unchanged);
    }

    // ── Half-folded emails (Pass 3) ─────────────────────────────────────

    #[test]
    fn half_folded_name_digits_at_gmail() {
        assert_eq!(
            recover("Anish Suman 2305@gmail.com"),
            "anishsuman2305@gmail.com"
        );
    }

    #[test]
    fn half_folded_single_token_untouched() {
        assert_eq!(recover("user@gmail.com"), "user@gmail.com");
    }

    // ── Spoken-form URLs (Pass 4) ──────────────────────────────────────

    #[test]
    fn url_localhost_colon_slash() {
        assert_eq!(
            recover("Open localhost colon 3000 slash api slash health."),
            "Open localhost:3000/api/health."
        );
    }

    #[test]
    fn url_domain_slash_path() {
        assert_eq!(
            recover("Check emiac dot app slash login slash callback."),
            "Check emiac.app/login/callback."
        );
    }

    #[test]
    fn url_prose_slash_stays_prose() {
        let unchanged = "Add a slash at the end of the sentence.";
        assert_eq!(recover(unchanged), unchanged);
    }

    // ── Spoken-form file paths (Pass 5) ──────────────────────────────────

    #[test]
    fn file_path_dot_slash() {
        assert_eq!(
            recover("Run dot slash script slash dev dot sh please."),
            "Run ./script/dev.sh please."
        );
    }

    #[test]
    fn file_path_config() {
        assert_eq!(
            recover("Edit dot slash config slash said dot json file."),
            "Edit ./config/said.json file."
        );
    }

    // ── Spoken-form identifiers (Pass 6) ─────────────────────────────

    #[test]
    fn ident_underscore_chain() {
        assert_eq!(
            recover("Set DEEPGRAM underscore API underscore KEY now."),
            "Set DEEPGRAM_API_KEY now."
        );
    }

    #[test]
    fn ident_prose_underscore_stays_prose() {
        let unchanged = "She used an underscore in her name.";
        assert_eq!(recover(unchanged), unchanged);
    }

    // ── Spoken-form env vars (Pass 7) ────────────────────────────────

    #[test]
    fn env_var_equals() {
        assert_eq!(
            recover("Set DEEPGRAM_API_KEY equals abc123 in the config."),
            "Set DEEPGRAM_API_KEY=abc123 in the config."
        );
    }

    #[test]
    fn equals_in_prose_stays_prose() {
        let unchanged = "Two plus two equals four.";
        assert_eq!(recover(unchanged), unchanged);
    }

    // ── Idempotency ───────────────────────────────────────────────────────

    #[test]
    fn idempotent() {
        let cases = [
            "V abhi dot verma 2678 at the rate Gmail dot com.",
            "HATPS://religwav.com",
            "Anish Suman 2305@gmail.com",
            "Open localhost colon 3000 slash api slash health.",
            "Set DEEPGRAM underscore API underscore KEY equals abc123.",
            "Run dot slash script slash dev dot sh please.",
        ];
        for c in cases {
            let first = recover(c);
            assert_eq!(recover(&first), first, "not idempotent for: {c}");
        }
    }

    // ── Harness — run with --nocapture to see all results ────────────────

    #[test]
    fn format_guard_harness() {
        let cases: Vec<(&str, &str)> = vec![
            // ── Emails ──────────────────────────────────────────────
            (
                "Anish Suman 2305 at the rate gmail dot com",
                "anishsuman2305@gmail.com",
            ),
            ("Mail anish at gmail dot com.", "Mail anish@gmail.com."),
            (
                "Send to vabhiverma2678@gmail.com.",
                "Send to vabhiverma2678@gmail.com.",
            ),
            // Half-folded email with "dot" in local — dot becomes "."
            (
                "V abi dot Verma 2678@gmail.com.",
                "vabi.verma2678@gmail.com.",
            ),
            // Catastrophic — prose must NOT fold into email
            (
                "Hello bhai kaise ho kya kar rahe ho. Anish Suman at the rate gmail dot com ko mail karo.",
                "..anishsuman@gmail.com..",
            ),
            // ── URLs ────────────────────────────────────────────────
            (
                "Open localhost colon 3000 slash api slash health.",
                "Open localhost:3000/api/health.",
            ),
            ("Check emiac dot app slash login.", "Check emiac.app/login."),
            // ── File paths ──────────────────────────────────────────
            (
                "Run dot slash script slash dev dot sh please.",
                "Run ./script/dev.sh please.",
            ),
            // ── Standalone slash with path context ───────────────────
            (
                "What about the slash meetings URL?",
                "What about the /meetings URL?",
            ),
            (
                "Check the slash api slash health endpoint.",
                "../api/health..",
            ),
            // Standalone slash without context — stays prose
            ("Add a slash at the end.", "Add a slash at the end."),
            // ── Identifiers ─────────────────────────────────────────
            (
                "Set DEEPGRAM underscore API underscore KEY now.",
                "Set DEEPGRAM_API_KEY now.",
            ),
            // ── Env vars ────────────────────────────────────────────
            ("Set API_KEY equals abc123.", "Set API_KEY=abc123."),
            // ── Protocol ────────────────────────────────────────────
            ("Open HATPS://site.com.", "Open https://site.com."),
            // ── Prose — must not change ─────────────────────────────
            (
                "Growing at the rate of 10% every year.",
                "Growing at the rate of 10% every year.",
            ),
            ("Meet me at home tomorrow.", "Meet me at home tomorrow."),
            (
                "She used an underscore in her name.",
                "She used an underscore in her name.",
            ),
        ];

        let mut pass = 0;
        let mut fail = 0;
        for (input, expected) in &cases {
            let actual = recover(input);
            let ok = if expected.starts_with("..") && expected.ends_with("..") {
                let needle = &expected[2..expected.len() - 2];
                actual.contains(needle)
            } else {
                &actual == expected
            };
            if ok {
                println!("  \u{2713} {input}");
                pass += 1;
            } else {
                println!("  \u{2717} {input}");
                println!("EXPECT: {expected}");
                println!("ACTUAL: {actual}");
                fail += 1;
            }
            println!("---");
        }
        println!("\n{pass} passed, {fail} failed");
        assert_eq!(fail, 0, "{fail} test case(s) failed — see output above");
    }
}
