//! Small text utilities shared across the workspace.

/// Truncate a string to at most `max_bytes` bytes **without splitting a UTF-8
/// character**.
///
/// `&s[..n]` panics with "byte index N is not a char boundary" when `n` lands
/// in the middle of a multi-byte codepoint. Since AirNote routinely handles
/// Hinglish/Devanagari (multi-byte) text — especially in error-path previews of
/// upstream HTTP bodies — a naive `&s[..s.len().min(n)]` is a latent panic.
/// This walks back to the nearest char boundary instead, so it is always safe.
///
/// Returns a borrowed slice; the whole string is returned when it already fits.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Remove stray leading/trailing ellipsis runs (`...`, `…`) that some STT
/// engines emit on continuation, e.g. `"...And jo speed hai"`.
///
/// Deliberately light: it only touches the very start and end, and only when
/// the run is a genuine ellipsis — the Unicode `…` or **two or more** ASCII
/// dots. A single sentence-final period is preserved (`"hai."` stays `"hai."`),
/// interior dots are never touched (`"a...b"`, URLs, `n.n`), and everything
/// else — words, casing, other punctuation — is left exactly as-is. The
/// whitespace the ellipsis was hiding is trimmed so `"... And"` becomes `"And"`.
pub fn strip_edge_ellipses(text: &str) -> String {
    fn is_dotish(c: char) -> bool {
        c == '.' || c == '…'
    }
    fn is_ellipsis_run(run: &str) -> bool {
        run.contains('…') || run.chars().filter(|c| *c == '.').count() >= 2
    }

    let mut s = text.trim_start_matches(char::is_whitespace);

    // Leading run.
    let lead_len: usize = s
        .chars()
        .take_while(|c| is_dotish(*c))
        .map(char::len_utf8)
        .sum();
    if lead_len > 0 && is_ellipsis_run(&s[..lead_len]) {
        s = s[lead_len..].trim_start();
    }

    s = s.trim_end_matches(char::is_whitespace);

    // Trailing run.
    let trail_len: usize = s
        .chars()
        .rev()
        .take_while(|c| is_dotish(*c))
        .map(char::len_utf8)
        .sum();
    if trail_len > 0 && is_ellipsis_run(&s[s.len() - trail_len..]) {
        s = s[..s.len() - trail_len].trim_end();
    }

    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::truncate_utf8;

    #[test]
    fn returns_whole_string_when_within_budget() {
        assert_eq!(truncate_utf8("hello", 300), "hello");
        assert_eq!(truncate_utf8("hello", 5), "hello");
    }

    #[test]
    fn truncates_ascii_at_exact_byte() {
        assert_eq!(truncate_utf8("hello world", 5), "hello");
    }

    #[test]
    fn never_splits_a_multibyte_char() {
        // Each Devanagari char here is 3 bytes. Slicing at a non-boundary byte
        // would panic with the naive `&s[..n]`; this must back off cleanly.
        let s = "नमस्ते"; // multiple 3-byte codepoints
        for n in 0..=s.len() + 4 {
            let out = truncate_utf8(s, n);
            // Must always be a valid prefix (no panic, valid UTF-8 boundary).
            assert!(s.starts_with(out));
            assert!(out.len() <= n.min(s.len()));
        }
    }

    #[test]
    fn handles_emoji_and_mixed_scripts() {
        let s = "ok👍 भाई done"; // 4-byte emoji + 3-byte Devanagari + ascii
        for n in 0..=s.len() {
            let out = truncate_utf8(s, n);
            assert!(s.starts_with(out));
        }
    }

    use super::strip_edge_ellipses;

    #[test]
    fn strips_leading_ellipsis_from_stt() {
        // The screenshot case: Deepgram continuation artifact.
        assert_eq!(
            strip_edge_ellipses(
                "...And jo speed hai, latency sab decent hai. Koi dikkat nahin aati hai abhi."
            ),
            "And jo speed hai, latency sab decent hai. Koi dikkat nahin aati hai abhi."
        );
        assert_eq!(strip_edge_ellipses("… and then"), "and then");
        assert_eq!(strip_edge_ellipses(".. hello"), "hello");
    }

    #[test]
    fn strips_trailing_ellipsis_but_keeps_sentence_period() {
        assert_eq!(strip_edge_ellipses("theek hai..."), "theek hai");
        assert_eq!(strip_edge_ellipses("theek hai …"), "theek hai");
        // A normal sentence-final period must survive.
        assert_eq!(strip_edge_ellipses("theek hai."), "theek hai.");
    }

    #[test]
    fn leaves_interior_and_non_ellipsis_untouched() {
        // Interior dots, URLs, decimals, initials — never touched.
        assert_eq!(
            strip_edge_ellipses("go to acme.app/login"),
            "go to acme.app/login"
        );
        assert_eq!(
            strip_edge_ellipses("version 2.4.3 ready"),
            "version 2.4.3 ready"
        );
        assert_eq!(strip_edge_ellipses("a...b"), "a...b");
        // A lone leading period (rare) is not an ellipsis — preserved.
        assert_eq!(strip_edge_ellipses(".env file"), ".env file");
        // Both edges at once.
        assert_eq!(strip_edge_ellipses("...hello..."), "hello");
        // All-dots collapses to empty.
        assert_eq!(strip_edge_ellipses("..."), "");
        assert_eq!(strip_edge_ellipses("normal text"), "normal text");
    }
}
