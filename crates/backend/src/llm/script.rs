//! Script utilities for enforcing output-language contracts.

pub fn contains_devanagari(text: &str) -> bool {
    text.chars().any(is_devanagari)
}

fn is_devanagari(ch: char) -> bool {
    ('\u{0900}'..='\u{097F}').contains(&ch)
}

fn independent_vowel(ch: char) -> Option<&'static str> {
    Some(match ch {
        'अ' => "a",
        'आ' => "aa",
        'इ' => "i",
        'ई' => "ee",
        'उ' => "u",
        'ऊ' => "oo",
        'ए' => "e",
        'ऐ' => "ai",
        'ओ' => "o",
        'औ' => "au",
        'ऋ' => "ri",
        _ => return None,
    })
}

fn consonant(ch: char) -> Option<&'static str> {
    Some(match ch {
        'क' => "k",
        'ख' => "kh",
        'ग' => "g",
        'घ' => "gh",
        'ङ' => "ng",
        'च' => "ch",
        'छ' => "ch",
        'ज' => "j",
        'झ' => "jh",
        'ञ' => "ny",
        'ट' => "t",
        'ठ' => "th",
        'ड' => "d",
        'ढ' => "dh",
        'ण' => "n",
        'त' => "t",
        'थ' => "th",
        'द' => "d",
        'ध' => "dh",
        'न' => "n",
        'प' => "p",
        'फ' => "ph",
        'ब' => "b",
        'भ' => "bh",
        'म' => "m",
        'य' => "y",
        'र' => "r",
        'ल' => "l",
        'व' => "v",
        'श' => "sh",
        'ष' => "sh",
        'स' => "s",
        'ह' => "h",
        'क़' => "q",
        'ख़' => "kh",
        'ग़' => "gh",
        'ज़' => "z",
        'ड़' => "d",
        'ढ़' => "dh",
        'फ़' => "f",
        _ => return None,
    })
}

fn matra(ch: char) -> Option<&'static str> {
    Some(match ch {
        'ा' => "aa",
        'ि' => "i",
        'ी' => "ee",
        'ु' => "u",
        'ू' => "oo",
        'ृ' => "ri",
        'े' => "e",
        'ै' => "ai",
        'ो' => "o",
        'ौ' => "au",
        _ => return None,
    })
}

fn diacritic(ch: char) -> Option<&'static str> {
    Some(match ch {
        'ं' | 'ँ' => "n",
        'ः' => "h",
        '़' => "",
        'ऽ' => "",
        _ => return None,
    })
}

/// Romanize Devanagari into readable Hinglish while leaving existing Roman
/// English/Hinglish untouched. This is a deterministic guardrail, not a full
/// linguistic transliterator; the LLM prompt still carries the primary style
/// instruction.
pub fn romanize_devanagari(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    // Track whether we've seen a vowel-sound-producing character in the
    // current Devanagari word. This helps apply the rule: "schwa is only
    // deleted from non-initial syllables."
    let mut seen_vowel_in_word = false;

    while i < len {
        let ch = chars[i];

        // Reset word tracking at word boundaries (non-Devanagari chars)
        if !is_devanagari(ch) && ch != '्' {
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
                Some('्') => {
                    // Halant: suppress inherent vowel, skip the halant
                    i += 2;
                }
                _ => {
                    // Decide whether to emit the inherent "a" (schwa).
                    //
                    // Hindi schwa deletion heuristic:
                    //  1. Word-final: always drop.
                    //  2. Medial: drop only if this is NOT the first
                    //     vowel-sound in the word AND the next char is a
                    //     consonant that itself carries a vowel (matra or
                    //     inherent). This prevents clusters like "bhut"
                    //     for "बहुत" while still producing "isk" in "इसका".
                    let drop = match next {
                        None => true, // end of string
                        Some(n) if !is_devanagari(n) => true, // word boundary
                        Some(n) if consonant(n).is_some() => {
                            // Only apply medial deletion if we've already
                            // seen a vowel in this word (not first syllable).
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

        if ch == '्' {
            i += 1;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    out
}

/// Returns true when the character at `pos` is the last Devanagari character
/// in the current word (i.e., the next char is non-Devanagari or end-of-string).
fn is_word_final(chars: &[char], pos: usize) -> bool {
    let next = if pos + 1 < chars.len() {
        Some(chars[pos + 1])
    } else {
        None
    };
    match next {
        None => true,
        Some(n) => !is_devanagari(n),
    }
}

/// Check whether the consonant at `pos` in `chars` will end up carrying a
/// vowel sound (explicit matra, or inherent "a" that won't be deleted).
fn next_consonant_has_vowel(chars: &[char], pos: usize) -> bool {
    let next_after = if pos + 1 < chars.len() {
        Some(chars[pos + 1])
    } else {
        None
    };
    match next_after {
        Some(n) if matra(n).is_some() => true,
        Some('्') => true,
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
        // Word-final consonants should NOT get inherent "a"
        assert_eq!(romanize_devanagari("काम"), "kaam");
        assert_eq!(romanize_devanagari("कम"), "kam");
    }

    #[test]
    fn schwa_deletion_medial() {
        // Medial schwa before another consonant is dropped
        assert_eq!(romanize_devanagari("इसका"), "iskaa");
        assert_eq!(romanize_devanagari("बहुत"), "bahut");
    }

    #[test]
    fn schwa_deletion_yah() {
        // यह = य(y) + ह(h, word-final → no schwa) = "yah"
        assert_eq!(romanize_devanagari("यह"), "yah");
    }

    #[test]
    fn schwa_deletion_chaahie() {
        // चाहिए = च(ch) + ा(aa) + ह(h) + ि(i) + ए(e)
        assert_eq!(romanize_devanagari("चाहिए"), "chaahie");
    }

    #[test]
    fn schwa_deletion_mixed_text() {
        let out = romanize_devanagari("यह बहुत अच्छा है yaar");
        // अच्छा = अ(a) + च(ch) + ्(halant) + छ(ch) + ा(a, word-final)
        assert_eq!(out, "yah bahut achchaa hai yaar");
    }

    #[test]
    fn independent_vowels_still_work() {
        assert_eq!(romanize_devanagari("आज"), "aaj");
    }

    #[test]
    fn bhi_unchanged() {
        // भी = भ(bh) + ी(ee)
        assert_eq!(romanize_devanagari("भी"), "bhee");
    }
}
