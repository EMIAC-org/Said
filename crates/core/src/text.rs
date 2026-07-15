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

/// Accumulates arbitrary byte chunks into complete, UTF-8 SSE lines.
///
/// HTTP stream chunks are byte boundaries, not character boundaries. Decoding
/// each chunk independently can replace a split multi-byte character with
/// `\u{FFFD}`. Newlines are ASCII and cannot occur inside a UTF-8 character, so
/// it is safe to wait for a complete line before decoding it.
#[derive(Debug, Default)]
pub struct Utf8LineBuffer {
    pending: Vec<u8>,
}

impl Utf8LineBuffer {
    /// Adds a byte chunk and returns every newline-terminated UTF-8 line.
    ///
    /// Returned lines exclude their trailing `\n`; an optional `\r` is left for
    /// callers that need to support CRLF framing.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, std::string::FromUtf8Error> {
        self.pending.extend_from_slice(chunk);

        let mut lines = Vec::new();
        while let Some(newline) = self.pending.iter().position(|&byte| byte == b'\n') {
            let mut bytes: Vec<u8> = self.pending.drain(..=newline).collect();
            bytes.pop(); // trailing newline
            lines.push(String::from_utf8(bytes)?);
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::{Utf8LineBuffer, truncate_utf8};

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

    #[test]
    fn buffers_split_unicode_until_a_complete_line_arrives() {
        let line = "data: {\"text\":\"A — नमस्ते\"}\n";
        let bytes = line.as_bytes();
        // Split after the first byte of the em dash's three-byte UTF-8 sequence.
        let split = bytes.iter().position(|&byte| byte == 0xe2).unwrap() + 1;
        let mut buffer = Utf8LineBuffer::default();

        assert!(buffer.push(&bytes[..split]).unwrap().is_empty());
        assert_eq!(
            buffer.push(&bytes[split..]).unwrap(),
            vec![line.trim_end_matches('\n')]
        );
    }
}
