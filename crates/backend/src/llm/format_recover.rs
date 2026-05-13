//! Deterministic post-LLM cleanup for the two spoken-form patterns the
//! polish LLM most often leaves un-folded:
//!
//!   1. Misheard URL protocols. STT routinely transcribes "h t t p s ://"
//!      as an acronym ("HATPS", "ACHTPS", "AICHTPS", "HTPS", "HTTP S").
//!      Smart_format normalizes `://` but leaves the misheard token. The
//!      LLM is supposed to fix it via few-shot exemplars but is unreliable
//!      when the surrounding text is mixed-script or has confidence markers.
//!
//!   2. Spoken-form emails. "<name parts> [dot <name>] [digits] at the rate
//!      <domain> dot <tld>" — Deepgram's smart_format only handles this in
//!      pure-English audio; for Hinglish (`lang=hi` or `lang=multi`) the
//!      pattern arrives at the LLM untouched. Few-shot exemplars catch most
//!      shapes but miss when confidence markers `[word?NN%]` are present or
//!      when Devanagari fragments leak through.
//!
//! This module is the safety net. It runs AFTER the LLM polish and AFTER
//! stream_safety scrub, on the final polished string only (not on every
//! streamed token). It is intentionally conservative: only highly-specific
//! shapes get folded. Plain prose is never touched.
//!
//! See `crates/backend/src/llm/script.rs` for the Devanagari romanizer that
//! runs in a different layer (per-token, during streaming).

use once_cell::sync::Lazy;
use regex::Regex;

/// Apply all recovery passes. Idempotent. Safe to call on any string.
pub fn recover(text: &str) -> String {
    let s = recover_protocol_mishears(text);
    let s = recover_spoken_emails(&s);
    compact_local_before_at(&s)
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 1 — Protocol mishear recovery
// ─────────────────────────────────────────────────────────────────────────────

/// Misheard variants of "https" that appear immediately before `://`. The
/// list is closed-form: we have only ever observed these in production logs.
/// New shapes are easy to add — append to the alternation.
///
/// Case-insensitive. Word-boundary anchored on the left so `whatps://`
/// inside another word would not match.
static PROTOCOL_MISHEAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b                            # word boundary on the left
        (?:                           # any of these misheard acronyms
            hatps |
            achtps |
            aichtps |
            htps |
            http \s+ s |              # 'HTTP S' with whitespace
            h \s+ t \s+ t \s+ p \s+ s # 'H T T P S' fully spelled
        )
        ( :// )                       # capture the protocol terminator
        ",
    )
    .unwrap()
});

fn recover_protocol_mishears(text: &str) -> String {
    PROTOCOL_MISHEAR.replace_all(text, "https$1").into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 2 — Spoken-form email recovery
// ─────────────────────────────────────────────────────────────────────────────

/// Match the spoken email shape end-to-end so we never partially fold prose.
///
/// Anatomy of the pattern:
///   <local>  ::= word-fragment ( ('.'|'dot') word-fragment )* digits?
///   <sep>    ::= 'at the rate' | 'at gmail/outlook/...' | '@'
///   <tld>    ::= 'com'|'in'|'org'|'io'|'net'|'co'|'app'|'dev'|'ai'|'co.in'|'co.uk'
///   <domain> ::= word-fragment ( 'dot' word-fragment )* 'dot' <tld>
///
/// Conservative rules baked in:
///   • Domain MUST end with a known TLD via "dot <tld>" (or already-folded
///     ".<tld>"). This is what stops us from mangling
///     "growing at the rate of 10% per year".
///   • Local part must produce at least one alphanumeric character.
///   • At least one separator marker present ("at the rate" or "@").
///   • Case-insensitive everywhere.
///
/// Result format: `[local]@[domain].[tld]`, all lowercase, no internal
/// whitespace.
static SPOKEN_EMAIL: Lazy<Regex> = Lazy::new(|| {
    let tld = r"com|in|org|io|net|co|app|dev|ai|me|us|uk|edu|gov";
    // The capture groups are intentionally permissive — final cleanup
    // happens in the replacement callback in `recover_spoken_emails`.
    //
    // `verb` captures common email-action verbs at the start (Mail, Send to,
    // Ping, etc.) so they stay outside the local part instead of being
    // gobbled into "mailanish...".
    Regex::new(&format!(
        r"(?ix)
        (?P<verb>                           # ── optional leading verb (preserved as-is)
            \b
            (?: mail | email | e-mail
              | send \s+ (?: a \s+ )? (?: mail | email )?
              | send \s+ to | send
              | ping | drop | shoot | forward | fwd
              | cc | bcc
              | write | write \s+ to
              | message
              | to
            )
            \b \s+
        )?
        (?P<local>                          # ── local part
            [A-Za-z0-9]+                    # opening fragment (V, abhi, anish, ...)
            (?:                             # zero or more separated fragments
                (?: \s* (?: \. | \b dot \b ) \s* | \s+ )
                [A-Za-z0-9]+
            )*
        )
        (?:                                 # ── separator (allow zero-ws before @)
            \s* @
          | \s+ at \s+ the \s+ rate
          | \s+ at \s+
        )
        \s*
        (?P<domain>                         # ── domain (must include 'dot <tld>')
            [A-Za-z0-9]+
            (?: \s* (?: \. | \b dot \b ) \s* [A-Za-z0-9]+ )*
            \s* (?: \. | \b dot \b ) \s*
            (?: {tld} )
            (?: \s* (?: \. | \b dot \b ) \s* (?: {tld} ) )?
        )
        \b
        ",
    ))
    .unwrap()
});

/// Tokens we collapse out of the local/domain parts. Case-insensitive,
/// whole-word match only.
static LOCAL_FILLER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:dot|at the rate|at)\b").unwrap());

fn recover_spoken_emails(text: &str) -> String {
    SPOKEN_EMAIL
        .replace_all(text, |caps: &regex::Captures| {
            let verb_raw = caps.name("verb").map_or("", |m| m.as_str());
            let local_raw = caps.name("local").map_or("", |m| m.as_str());
            let domain_raw = caps.name("domain").map_or("", |m| m.as_str());

            // Local part: drop "dot" tokens, collapse all whitespace and
            // punctuation, lowercase.
            let local = compact_local(local_raw);
            if !local.chars().any(|c| c.is_alphanumeric()) {
                return caps.get(0).unwrap().as_str().to_string();
            }

            // Domain part: replace " dot " with ".", strip stray whitespace,
            // lowercase. Validate the final shape — must contain at least
            // one '.', no whitespace.
            let domain = compact_domain(domain_raw);
            if !domain.contains('.') || domain.chars().any(char::is_whitespace) {
                return caps.get(0).unwrap().as_str().to_string();
            }

            format!("{verb_raw}{local}@{domain}")
        })
        .into_owned()
}

fn compact_local(raw: &str) -> String {
    let stripped = LOCAL_FILLER.replace_all(raw, " ");
    let mut out = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

fn compact_domain(raw: &str) -> String {
    // Replace " dot " (and "dot " at edges) with "."
    let dotted = LOCAL_FILLER.replace_all(raw, ".");
    let mut out = String::with_capacity(dotted.len());
    for ch in dotted.chars() {
        if ch.is_whitespace() {
            continue;
        }
        out.extend(ch.to_lowercase());
    }
    // Collapse runs of dots that may have formed (e.g. ". .com")
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    out.trim_matches('.').to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 3 — Compact fragmented local parts before an existing @
// ─────────────────────────────────────────────────────────────────────────────

/// Matches `@domain.tld` so we can anchor backward-scans on it.
static AT_DOMAIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"@[A-Za-z0-9]+(?:\.[A-Za-z0-9]+)+").unwrap());

/// Known email domains — when we see `<fragments> at <domain>`, if the domain
/// is one of these we can be confident this is an email even without "the rate".
static KNOWN_EMAIL_DOMAINS: &[&str] = &[
    "gmail",
    "outlook",
    "yahoo",
    "hotmail",
    "protonmail",
    "icloud",
    "aol",
    "zoho",
    "yandex",
    "mail",
    "live",
    "msn",
    "rediffmail",
    "proton",
];

/// Catches "<fragments> at <known-domain> dot <tld>" where the LLM wrote
/// "at" instead of "@" but the domain is recognizable.
static AT_KNOWN_DOMAIN: Lazy<Regex> = Lazy::new(|| {
    let domains = KNOWN_EMAIL_DOMAINS.join("|");
    Regex::new(&format!(
        r"(?ix)
        (?P<frags>
            [A-Za-z0-9]+
            (?:[ \t,.\x{{00A0}}]+[A-Za-z0-9]+)*
        )
        \s+ at \s+
        (?P<domain>
            (?:{domains})
            \s* (?:\. | \bdot\b) \s*
            [A-Za-z]{{2,6}}
            (?: \s* (?:\. | \bdot\b) \s* [A-Za-z]{{2,6}} )?
        )
        \b
        ",
    ))
    .unwrap()
});

/// Words that commonly precede an email address but are NOT part of the
/// local part. When the backward scan hits one of these it stops.
fn is_email_verb(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
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
            | "at"
            | "the"
            | "is"
            | "my"
            | "his"
            | "her"
    )
}

fn compact_local_before_at(text: &str) -> String {
    // Collect match positions first to avoid borrow conflicts.
    let positions: Vec<(usize, usize)> = AT_DOMAIN
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();

    let mut result = text.to_string();

    // Process from right to left so replacements don't shift earlier indices.
    for &(at_pos, domain_end) in positions.iter().rev() {
        let domain = result[at_pos + 1..domain_end].to_string();
        let before = &result[..at_pos];

        let mut fragments: Vec<String> = Vec::new();
        let mut scan_end = before.len();

        for token in before.split(|c: char| !c.is_alphanumeric()).rev() {
            if token.is_empty() {
                continue;
            }
            if is_email_verb(token) {
                break;
            }
            fragments.push(token.to_string());
            let token_start = before[..scan_end].rfind(token).unwrap_or(0);
            scan_end = token_start;
        }

        fragments.reverse();

        if fragments.len() < 2 {
            continue;
        }

        let local = join_fragments_vec_owned(&fragments);
        if local.is_empty() {
            continue;
        }

        let prefix = &result[..scan_end];
        let suffix = &result[domain_end..];
        result = format!("{prefix}{local}@{domain}{suffix}");
    }

    // Second sub-pass: "<fragments> at <known-domain> dot <tld>"
    AT_KNOWN_DOMAIN
        .replace_all(&result, |caps: &regex::Captures| {
            let frags = caps.name("frags").unwrap().as_str();
            let domain_raw = caps.name("domain").unwrap().as_str();
            let local = join_fragments(frags);
            if local.is_empty() {
                return caps.get(0).unwrap().as_str().to_string();
            }
            let domain = compact_domain(domain_raw);
            if !domain.contains('.') {
                return caps.get(0).unwrap().as_str().to_string();
            }
            format!("{local}@{domain}")
        })
        .into_owned()
}

fn join_fragments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

fn join_fragments_vec_owned(fragments: &[String]) -> String {
    let mut out = String::new();
    for frag in fragments {
        for ch in frag.chars() {
            if ch.is_alphanumeric() {
                out.extend(ch.to_lowercase());
            }
        }
    }
    out
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
    fn protocol_aichtps() {
        assert_eq!(
            recover("AICHTPS://emiac.app/login"),
            "https://emiac.app/login"
        );
    }

    #[test]
    fn protocol_http_s_spaced() {
        assert_eq!(
            recover("Open HTTP S://api.example.com"),
            "Open https://api.example.com"
        );
    }

    #[test]
    fn protocol_h_t_t_p_s_fully_spelled() {
        assert_eq!(
            recover("Go to H T T P S://emiac.app/login."),
            "Go to https://emiac.app/login."
        );
    }

    #[test]
    fn protocol_case_insensitive() {
        assert_eq!(recover("hatps://site.com"), "https://site.com");
        assert_eq!(recover("Hatps://site.com"), "https://site.com");
    }

    #[test]
    fn protocol_leaves_real_https_alone() {
        assert_eq!(recover("https://example.com"), "https://example.com");
        assert_eq!(recover("http://example.com"), "http://example.com");
    }

    // ── Spoken-form emails ────────────────────────────────────────────────

    #[test]
    fn email_v_abhi_dot_verma_digits() {
        // The exact production-failing case the user hit repeatedly.
        assert_eq!(
            recover("V abhi dot verma 2678 at the rate Gmail dot com."),
            "vabhiverma2678@gmail.com."
        );
    }

    #[test]
    fn email_anish_suman_with_digits() {
        assert_eq!(
            recover("Mail Anish Suman two three zero five at the rate gmail dot com."),
            // "two three zero five" stays as spoken — smart_format/LLM handles
            // digit-word normalization in earlier layers. This recover pass
            // only folds the email-shaped pattern around the existing form.
            "Mail anishsumantwothreezerofive@gmail.com."
        );
    }

    #[test]
    fn email_already_folded_passes_through() {
        let unchanged = "Send to vabhiverma2678@gmail.com.";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn email_with_outlook_tld() {
        assert_eq!(
            recover("Mail A B C dot rahul 99 at the rate Outlook dot in."),
            "Mail abcrahul99@outlook.in."
        );
    }

    #[test]
    fn email_compound_tld_co_in() {
        assert_eq!(
            recover("Send to test at the rate google dot co dot in."),
            "Send to test@google.co.in."
        );
    }

    #[test]
    fn email_at_alone_with_dot_tld() {
        // "<name> at <domain> dot com" — common when STT drops "the rate"
        assert_eq!(
            recover("Mail anish at gmail dot com."),
            "Mail anish@gmail.com."
        );
    }

    #[test]
    fn email_with_at_symbol_already() {
        // Half-folded: smart_format/LLM made the @ but left "dot com"
        assert_eq!(
            recover("Mail anish@gmail dot com."),
            "Mail anish@gmail.com."
        );
    }

    // ── Negative cases — prose stays prose ────────────────────────────────

    #[test]
    fn prose_at_the_rate_of_percent_stays_prose() {
        let unchanged = "Growing at the rate of 10% every year.";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn prose_at_without_dot_tld_stays_prose() {
        // "at home" — no `dot <tld>` after, must not fold.
        let unchanged = "Meet me at home tomorrow.";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn prose_at_the_rate_without_dot_tld_stays_prose() {
        let unchanged = "Interest accumulates at the rate of nine percent annually.";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn prose_dot_com_in_url_context_not_folded() {
        // No "at" or "@" separator → don't fold even if "dot com" appears.
        let unchanged = "Visit example dot com tomorrow.";
        assert_eq!(recover(unchanged), unchanged);
    }

    // ── Half-folded emails (Pass 3) ─────────────────────────────────────

    #[test]
    fn half_folded_name_comma_digits_at_gmail() {
        assert_eq!(
            recover("Anish Suman, 2305@gmail.com"),
            "anishsuman2305@gmail.com"
        );
    }

    #[test]
    fn half_folded_name_space_digits_at_gmail() {
        assert_eq!(
            recover("Manish Suman 2 0305@gmail.com"),
            "manishsuman20305@gmail.com"
        );
    }

    #[test]
    fn half_folded_v_abhi_dot_verma() {
        assert_eq!(
            recover("V abhi. verma 2678@gmail.com"),
            "vabhiverma2678@gmail.com"
        );
    }

    #[test]
    fn half_folded_preserves_surrounding_text() {
        assert_eq!(
            recover("Send to Anish Suman 2305@gmail.com tomorrow."),
            "Send to anishsuman2305@gmail.com tomorrow."
        );
    }

    #[test]
    fn half_folded_single_token_before_at_untouched() {
        assert_eq!(recover("user@gmail.com"), "user@gmail.com");
    }

    #[test]
    fn half_folded_no_domain_dot_untouched() {
        assert_eq!(recover("hello there@home"), "hello there@home");
    }

    #[test]
    fn half_folded_mention_untouched() {
        let unchanged = "hello @mention in chat";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn at_known_domain_without_symbol() {
        assert_eq!(
            recover("Anish Suman 2305 at gmail dot com"),
            "anishsuman2305@gmail.com"
        );
    }

    #[test]
    fn at_known_domain_outlook() {
        assert_eq!(recover("rahul 99 at outlook dot in"), "rahul99@outlook.in");
    }

    #[test]
    fn at_unknown_domain_not_folded() {
        let unchanged = "data at myserver dot internal";
        assert_eq!(recover(unchanged), unchanged);
    }

    #[test]
    fn half_folded_time_fragment_colon_stripped() {
        // Deepgram sometimes outputs "03:05" when user says "0305"
        // The colon is not alphanumeric so join_fragments drops it
        assert_eq!(
            recover("Anish Suman 2 03:05@gmail.com"),
            "anishsuman20305@gmail.com"
        );
    }

    // ── Idempotency ───────────────────────────────────────────────────────

    #[test]
    fn idempotent_applies_once() {
        let s = recover("V abhi dot verma 2678 at the rate Gmail dot com.");
        assert_eq!(recover(&s), s);

        let t = recover("HATPS://religwav.com");
        assert_eq!(recover(&t), t);

        let u = recover("Anish Suman, 2305@gmail.com");
        assert_eq!(recover(&u), u);
    }
}
