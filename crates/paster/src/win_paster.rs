//! Pure logic for the Windows paster (UTF-16 unit encoding + clipboard
//! state machine). Compiled on every host so unit tests run on the macOS
//! dev box and on every CI runner — the Win32 plumbing in [`crate::imp_windows`]
//! only adds the actual `SendInput` / `OpenClipboard` calls on top.
//!
//! Why this matters: Windows `SendInput(KEYEVENTF_UNICODE)` ships **one UTF-16
//! code unit per `INPUT` event**. Characters outside the BMP (emoji, some CJK
//! extensions) need a surrogate pair — two events per character. Getting the
//! encoding wrong manifests as garbled text in the user's app, so we keep
//! this layer pure and tested instead of weaving it into the Win32 path.

/// Encode a Rust `&str` into a flat vector of UTF-16 code units suitable for
/// `SendInput(KEYEVENTF_UNICODE)`.
///
/// Characters in the BMP (codepoint <= 0xFFFF) emit one code unit; characters
/// above the BMP emit a surrogate pair (two units). Newlines become CR+LF
/// (`\r\n`) since most Windows controls expect carriage returns, not bare LF.
pub fn text_to_utf16_units(text: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\n' {
            out.push(b'\r' as u16);
            out.push(b'\n' as u16);
        } else {
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push(*unit);
            }
        }
    }
    out
}

/// Build the UTF-16 buffer for `SetClipboardData(CF_UNICODETEXT, ...)`.
/// Same encoding as the keystroke path but **with a NUL terminator** —
/// CF_UNICODETEXT requires a zero-terminated string.
pub fn text_to_clipboard_utf16(text: &str) -> Vec<u16> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    units
}

/// True when the given UTF-16 code unit is a high surrogate (the first half
/// of a surrogate pair). Used by Win32 path to mark surrogate-pair events
/// with `KEYEVENTF_KEYUP` correctly.
pub fn is_high_surrogate(unit: u16) -> bool {
    (0xD800..=0xDBFF).contains(&unit)
}

/// True when the given UTF-16 code unit is a low surrogate.
pub fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_encodes_to_empty_vec() {
        assert_eq!(text_to_utf16_units(""), Vec::<u16>::new());
    }

    #[test]
    fn ascii_one_unit_per_char() {
        let out = text_to_utf16_units("Hello");
        assert_eq!(out, vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]);
    }

    #[test]
    fn newline_becomes_crlf() {
        // Critical for Windows controls — Notepad and most edit fields drop
        // bare LFs and produce a literal "vertical tab" glyph instead.
        let out = text_to_utf16_units("a\nb");
        assert_eq!(
            out,
            vec![b'a' as u16, b'\r' as u16, b'\n' as u16, b'b' as u16]
        );
    }

    #[test]
    fn bmp_unicode_one_unit_per_char() {
        // U+00E9 (é) and U+0301 (combining acute) — both in BMP, one unit each.
        let out = text_to_utf16_units("café");
        assert_eq!(out, vec![0x63, 0x61, 0x66, 0xE9]);
    }

    #[test]
    fn devanagari_one_unit_per_codepoint() {
        // Hinglish-relevant: ensure Devanagari script encodes correctly.
        // U+0928 (न), U+092E (म), U+0938 (स), U+094D (◌्), U+0924 (त)
        let out = text_to_utf16_units("नमस्ते");
        assert_eq!(out, vec![0x0928, 0x092E, 0x0938, 0x094D, 0x0924, 0x0947]);
    }

    #[test]
    fn emoji_emits_surrogate_pair() {
        // 😀 (U+1F600) is above the BMP — must encode as surrogate pair.
        let out = text_to_utf16_units("😀");
        assert_eq!(out.len(), 2);
        assert!(is_high_surrogate(out[0]));
        assert!(is_low_surrogate(out[1]));
        assert_eq!(out[0], 0xD83D);
        assert_eq!(out[1], 0xDE00);
    }

    #[test]
    fn mixed_bmp_and_supplementary() {
        // Mix ASCII + BMP + supplementary plane to exercise the loop:
        // "a" + "é" + "😀" + "b" -> 1 + 1 + 2 + 1 = 5 units.
        let out = text_to_utf16_units("aé😀b");
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], b'a' as u16);
        assert_eq!(out[1], 0xE9);
        assert_eq!(out[2], 0xD83D); // emoji high surrogate
        assert_eq!(out[3], 0xDE00); // emoji low surrogate
        assert_eq!(out[4], b'b' as u16);
    }

    #[test]
    fn surrogate_classifiers_are_exhaustive() {
        // Boundary checks for the surrogate-range predicates.
        assert!(!is_high_surrogate(0xD7FF));
        assert!(is_high_surrogate(0xD800));
        assert!(is_high_surrogate(0xDBFF));
        assert!(!is_high_surrogate(0xDC00));

        assert!(!is_low_surrogate(0xDBFF));
        assert!(is_low_surrogate(0xDC00));
        assert!(is_low_surrogate(0xDFFF));
        assert!(!is_low_surrogate(0xE000));

        // Random in-BMP scalar is neither.
        assert!(!is_high_surrogate(b'A' as u16));
        assert!(!is_low_surrogate(b'A' as u16));
    }

    #[test]
    fn clipboard_utf16_is_nul_terminated() {
        let out = text_to_clipboard_utf16("hi");
        assert_eq!(out, vec![b'h' as u16, b'i' as u16, 0]);
    }

    #[test]
    fn clipboard_utf16_empty_is_just_nul() {
        assert_eq!(text_to_clipboard_utf16(""), vec![0]);
    }

    #[test]
    fn clipboard_utf16_does_not_translate_newlines() {
        // Setting clipboard is different from typing — apps that paste
        // from CF_UNICODETEXT handle newline translation themselves.
        // We pass through as the user authored.
        let out = text_to_clipboard_utf16("a\nb");
        assert_eq!(out, vec![b'a' as u16, b'\n' as u16, b'b' as u16, 0]);
    }

    /// Stress: a long string mixing many Unicode planes encodes deterministically.
    #[test]
    fn long_mixed_string_round_trips() {
        let s = "Hello! こんにちは 😀 नमस्ते";
        let units = text_to_utf16_units(s);
        // Decode back via standard String::from_utf16 (filtering CR inserted
        // by our newline translation — there are no newlines here so it's a
        // direct round-trip).
        let decoded = String::from_utf16(&units).expect("valid utf-16");
        assert_eq!(decoded, s);
    }
}
