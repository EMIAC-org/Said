//! Output-language script guard (Hinglish guarantee).
//!
//! Ported from the desktop backend's `llm/script.rs`. Deterministic, dependency
//! free. After the LLM polishes a transcript, this guarantees Hinglish output
//! never contains Devanagari (or other non-Latin scripts the model occasionally
//! hallucinates). It is a guardrail, not a full transliterator.
//!
//! All Devanagari is written as `\u{...}` escapes so the source is immune to
//! Unicode normalization (precomposed vs base+nukta) of multi-codepoint glyphs.

pub fn contains_devanagari(text: &str) -> bool {
    text.chars().any(is_devanagari)
}

fn is_devanagari(ch: char) -> bool {
    ('\u{0900}'..='\u{097F}').contains(&ch)
}

fn independent_vowel(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{0905}' => "a",  // अ
        '\u{0906}' => "aa", // आ
        '\u{0907}' => "i",  // इ
        '\u{0908}' => "ee", // ई
        '\u{0909}' => "u",  // उ
        '\u{090A}' => "oo", // ऊ
        '\u{090F}' => "e",  // ए
        '\u{0910}' => "ai", // ऐ
        '\u{0913}' => "o",  // ओ
        '\u{0914}' => "au", // औ
        '\u{090B}' => "ri", // ऋ
        _ => return None,
    })
}

fn consonant(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{0915}' => "k",  // क
        '\u{0916}' => "kh", // ख
        '\u{0917}' => "g",  // ग
        '\u{0918}' => "gh", // घ
        '\u{0919}' => "ng", // ङ
        '\u{091A}' => "ch", // च
        '\u{091B}' => "ch", // छ
        '\u{091C}' => "j",  // ज
        '\u{091D}' => "jh", // झ
        '\u{091E}' => "ny", // ञ
        '\u{091F}' => "t",  // ट
        '\u{0920}' => "th", // ठ
        '\u{0921}' => "d",  // ड
        '\u{0922}' => "dh", // ढ
        '\u{0923}' => "n",  // ण
        '\u{0924}' => "t",  // त
        '\u{0925}' => "th", // थ
        '\u{0926}' => "d",  // द
        '\u{0927}' => "dh", // ध
        '\u{0928}' => "n",  // न
        '\u{092A}' => "p",  // प
        '\u{092B}' => "ph", // फ
        '\u{092C}' => "b",  // ब
        '\u{092D}' => "bh", // भ
        '\u{092E}' => "m",  // म
        '\u{092F}' => "y",  // य
        '\u{0930}' => "r",  // र
        '\u{0932}' => "l",  // ल
        '\u{0935}' => "v",  // व
        '\u{0936}' => "sh", // श
        '\u{0937}' => "sh", // ष
        '\u{0938}' => "s",  // स
        '\u{0939}' => "h",  // ह
        '\u{0958}' => "q",  // क़
        '\u{0959}' => "kh", // ख़
        '\u{095A}' => "gh", // ग़
        '\u{095B}' => "z",  // ज़
        '\u{095C}' => "d",  // ड़
        '\u{095D}' => "dh", // ढ़
        '\u{095E}' => "f",  // फ़
        _ => return None,
    })
}

fn matra(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{093E}' => "aa", // ा
        '\u{093F}' => "i",  // ि
        '\u{0940}' => "ee", // ी
        '\u{0941}' => "u",  // ु
        '\u{0942}' => "oo", // ू
        '\u{0943}' => "ri", // ृ
        '\u{0947}' => "e",  // े
        '\u{0948}' => "ai", // ै
        '\u{094B}' => "o",  // ो
        '\u{094C}' => "au", // ौ
        _ => return None,
    })
}

fn diacritic(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{0902}' | '\u{0901}' => "n", // ं ँ
        '\u{0903}' => "h",              // ः
        '\u{093C}' => "",               // ़ (nukta)
        '\u{093D}' => "",               // ऽ (avagraha)
        _ => return None,
    })
}

const HALANT: char = '\u{094D}'; // ्

/// Romanize Devanagari into readable Hinglish while leaving existing Roman text
/// untouched.
pub fn romanize_devanagari(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut seen_vowel_in_word = false;

    while i < len {
        let ch = chars[i];

        if !is_devanagari(ch) && ch != HALANT {
            if ch.is_whitespace() || ch.is_ascii_punctuation() {
                seen_vowel_in_word = false;
            }
            out.push(ch);
            i += 1;
            continue;
        }

        if let Some(v) = independent_vowel(ch) {
            out.push_str(v);
            seen_vowel_in_word = true;
            i += 1;
            continue;
        }

        if let Some(base) = consonant(ch) {
            out.push_str(base);
            let next = if i + 1 < len { Some(chars[i + 1]) } else { None };
            match next {
                Some(n) if matra(n).is_some() => {
                    out.push_str(matra(n).unwrap_or_default());
                    seen_vowel_in_word = true;
                    i += 2;
                }
                Some(c) if c == HALANT => {
                    i += 2;
                }
                _ => {
                    let drop = match next {
                        None => true,
                        Some(n) if !is_devanagari(n) => true,
                        Some(n) if consonant(n).is_some() => {
                            if !seen_vowel_in_word {
                                false
                            } else {
                                next_consonant_has_vowel(&chars, i + 1)
                            }
                        }
                        _ => false,
                    };
                    if !drop {
                        out.push('a');
                        seen_vowel_in_word = true;
                    }
                    i += 1;
                }
            }
            continue;
        }

        if let Some(v) = matra(ch).or_else(|| diacritic(ch)) {
            out.push_str(v);
            if matra(ch).is_some() {
                seen_vowel_in_word = true;
            }
            i += 1;
            continue;
        }

        if ch == HALANT {
            i += 1;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    out
}

/// Check whether the consonant at `pos` will end up carrying a vowel sound.
fn next_consonant_has_vowel(chars: &[char], pos: usize) -> bool {
    let next_after = if pos + 1 < chars.len() {
        Some(chars[pos + 1])
    } else {
        None
    };
    match next_after {
        Some(n) if matra(n).is_some() => true,
        Some(c) if c == HALANT => true,
        None => false,
        Some(n) if !is_devanagari(n) => false,
        Some(n) if consonant(n).is_some() => true,
        _ => true,
    }
}

pub fn enforce_roman_hinglish(text: &str) -> String {
    if contains_devanagari(text) {
        romanize_devanagari(text)
    } else {
        text.to_string()
    }
}

/// Strip characters from non-Latin scripts that the LLM sometimes hallucinates
/// (CJK, Arabic, Cyrillic, etc). Keeps ASCII + common Unicode punctuation +
/// Latin Extended (accents).
pub fn strip_non_latin_scripts(text: &str) -> String {
    text.chars()
        .filter(|c| {
            c.is_ascii()
                || matches!(*c as u32,
                    0x2000..=0x206F
                    | 0x00C0..=0x024F
                )
        })
        .collect()
}

/// Gateway entry point: enforce the configured output language on polished text.
///
/// - `hinglish` → romanize any Devanagari, then strip stray non-Latin scripts.
/// - `hindi`    → leave Devanagari intact (Hindi output is allowed).
/// - others     → pass through unchanged.
pub fn apply_script_guard(text: &str, language: &str) -> String {
    match language {
        "hinglish" => strip_non_latin_scripts(&enforce_roman_hinglish(text)),
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn romanizes_common_hindi_to_hinglish_script() {
        let out = enforce_roman_hinglish("आज बहुत काम था, मैं थक गया हूँ.");
        assert!(!contains_devanagari(&out));
        assert!(out.contains("aaj"));
        assert!(out.contains("main"));
    }

    #[test]
    fn leaves_roman_text_unchanged() {
        let text = "Aaj bahut kaam tha, but deployment went fine.";
        assert_eq!(enforce_roman_hinglish(text), text);
    }

    #[test]
    fn schwa_deletion_word_final() {
        assert_eq!(romanize_devanagari("काम"), "kaam");
        assert_eq!(romanize_devanagari("कम"), "kam");
    }

    #[test]
    fn schwa_deletion_medial() {
        assert_eq!(romanize_devanagari("इसका"), "iskaa");
        assert_eq!(romanize_devanagari("बहुत"), "bahut");
    }

    #[test]
    fn mixed_text_romanizes_only_devanagari() {
        let out = romanize_devanagari("यह बहुत अच्छा है yaar");
        assert_eq!(out, "yah bahut achchaa hai yaar");
    }

    #[test]
    fn apply_guard_only_acts_on_hinglish() {
        assert_eq!(apply_script_guard("नमस्ते", "hindi"), "नमस्ते");
        assert!(!contains_devanagari(&apply_script_guard("नमस्ते", "hinglish")));
        assert_eq!(apply_script_guard("hello", "english"), "hello");
    }
}
