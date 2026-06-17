import Foundation

/// Deterministic Devanagari→Roman guard — a Swift port of the desktop backend's
/// `script.rs`. The cloud polish only *asks* the model to output Roman Hinglish;
/// when residual Devanagari slips through ("है"), this guarantees the final text
/// is pure Roman. It leaves already-Roman/English text untouched, so it is safe
/// to run unconditionally on any polished output.
public enum HinglishScript {

    public static func containsDevanagari(_ text: String) -> Bool {
        text.unicodeScalars.contains { isDevanagari($0) }
    }

    /// Romanize any Devanagari in `text`; leaves Roman/English untouched.
    ///
    /// HARD GUARANTEE: the returned string never contains a Devanagari scalar.
    /// The mapper covers vowels, consonants, matras, diacritics, digits, danda
    /// and om; anything it doesn't recognize (rare signs) is stripped in a final
    /// pass so the invariant always holds — for *any* input, without crashing.
    public static func enforceRomanHinglish(_ text: String) -> String {
        guard containsDevanagari(text) else { return text }
        let romanized = romanizeDevanagari(text)
        guard containsDevanagari(romanized) else { return romanized }
        // Safety net: drop any residual (unmapped) Devanagari scalar.
        var view = String.UnicodeScalarView()
        view.reserveCapacity(romanized.unicodeScalars.count)
        for scalar in romanized.unicodeScalars where !isDevanagari(scalar) {
            view.append(scalar)
        }
        return String(view)
    }

    private static func digit(_ ch: Character) -> String? {
        switch ch {
        case "०": return "0"
        case "१": return "1"
        case "२": return "2"
        case "३": return "3"
        case "४": return "4"
        case "५": return "5"
        case "६": return "6"
        case "७": return "7"
        case "८": return "8"
        case "९": return "9"
        default: return nil
        }
    }

    // MARK: - Internals (mirrors crates/backend/src/llm/script.rs)

    private static func isDevanagari(_ scalar: Unicode.Scalar) -> Bool {
        (0x0900...0x097F).contains(Int(scalar.value))
    }

    private static func isDevanagari(_ ch: Character) -> Bool {
        ch.unicodeScalars.allSatisfy { isDevanagari($0) } && !ch.unicodeScalars.isEmpty
    }

    private static func independentVowel(_ ch: Character) -> String? {
        switch ch {
        case "अ": return "a"
        case "आ": return "aa"
        case "इ": return "i"
        case "ई": return "ee"
        case "उ": return "u"
        case "ऊ": return "oo"
        case "ए": return "e"
        case "ऐ": return "ai"
        case "ओ": return "o"
        case "औ": return "au"
        case "ऋ": return "ri"
        case "ॠ": return "ree"
        case "ऌ": return "li"
        case "ॡ": return "lee"
        case "ॐ": return "om"
        default: return nil
        }
    }

    private static func consonant(_ ch: Character) -> String? {
        switch ch {
        case "क": return "k"
        case "ख": return "kh"
        case "ग": return "g"
        case "घ": return "gh"
        case "ङ": return "ng"
        case "च": return "ch"
        case "छ": return "ch"
        case "ज": return "j"
        case "झ": return "jh"
        case "ञ": return "ny"
        case "ट": return "t"
        case "ठ": return "th"
        case "ड": return "d"
        case "ढ": return "dh"
        case "ण": return "n"
        case "त": return "t"
        case "थ": return "th"
        case "द": return "d"
        case "ध": return "dh"
        case "न": return "n"
        case "प": return "p"
        case "फ": return "ph"
        case "ब": return "b"
        case "भ": return "bh"
        case "म": return "m"
        case "य": return "y"
        case "र": return "r"
        case "ल": return "l"
        case "व": return "v"
        case "श": return "sh"
        case "ष": return "sh"
        case "स": return "s"
        case "ह": return "h"
        case "क़": return "q"
        case "ख़": return "kh"
        case "ग़": return "gh"
        case "ज़": return "z"
        case "ड़": return "d"
        case "ढ़": return "dh"
        case "फ़": return "f"
        default: return nil
        }
    }

    private static func matra(_ ch: Character) -> String? {
        switch ch {
        case "ा": return "aa"
        case "ि": return "i"
        case "ी": return "ee"
        case "ु": return "u"
        case "ू": return "oo"
        case "ृ": return "ri"
        case "े": return "e"
        case "ै": return "ai"
        case "ो": return "o"
        case "ौ": return "au"
        default: return nil
        }
    }

    /// Nasal assimilation for anusvara/chandrabindu: "m" before a labial
    /// consonant, "n" otherwise (a readable Hinglish approximation).
    private static func anusvara(before next: Character?) -> String {
        switch next {
        case "प", "फ", "ब", "भ", "म": return "m"
        default: return "n"
        }
    }

    private static func diacritic(_ ch: Character) -> String? {
        switch ch {
        case "ं", "ँ": return "n"
        case "ः": return "h"
        case "़", "ऽ": return ""
        default: return nil
        }
    }

    private static func romanizeDevanagari(_ text: String) -> String {
        // Iterate Unicode SCALARS (matching the Rust `char` original), not Swift
        // grapheme clusters — otherwise a consonant + matra (e.g. क + ा = "का")
        // collapses into one Character and never matches the per-letter tables.
        let chars = text.unicodeScalars.map { Character($0) }
        let len = chars.count
        var out = String()
        out.reserveCapacity(text.count)
        var i = 0
        var seenVowelInWord = false

        while i < len {
            let ch = chars[i]

            // Devanagari danda / double-danda → period (sentence terminator).
            if ch == "।" || ch == "॥" {
                out.append(".")
                seenVowelInWord = false
                i += 1
                continue
            }

            if !isDevanagari(ch) && ch != "्" {
                if ch.isWhitespace || isAsciiPunctuation(ch) {
                    seenVowelInWord = false
                }
                out.append(ch)
                i += 1
                continue
            }

            if let d = digit(ch) {
                out.append(d)
                i += 1
                continue
            }

            if let v = independentVowel(ch) {
                out.append(v)
                seenVowelInWord = true
                i += 1
                continue
            }

            if let base = consonant(ch) {
                out.append(base)
                let next: Character? = (i + 1 < len) ? chars[i + 1] : nil
                if let n = next, let m = matra(n) {
                    out.append(m)
                    seenVowelInWord = true
                    i += 2
                } else if next == "्" {
                    i += 2
                } else {
                    let drop: Bool
                    switch next {
                    case nil:
                        drop = true
                    case let n? where !isDevanagari(n):
                        drop = true
                    case let n? where consonant(n) != nil:
                        drop = seenVowelInWord ? nextConsonantHasVowel(chars, i + 1) : false
                    default:
                        drop = false
                    }
                    if !drop {
                        out.append("a")
                        seenVowelInWord = true
                    }
                    i += 1
                }
                continue
            }

            // Anusvara / chandrabindu — a nasal that assimilates to the following
            // consonant ("नंबर" → "nambar", not "nanbar").
            if ch == "ं" || ch == "ँ" {
                let next: Character? = (i + 1 < len) ? chars[i + 1] : nil
                out.append(anusvara(before: next))
                i += 1
                continue
            }

            if let v = matra(ch) ?? diacritic(ch) {
                out.append(v)
                if matra(ch) != nil { seenVowelInWord = true }
                i += 1
                continue
            }

            if ch == "्" {
                i += 1
                continue
            }

            out.append(ch)
            i += 1
        }

        return out
    }

    private static func nextConsonantHasVowel(_ chars: [Character], _ pos: Int) -> Bool {
        let nextAfter: Character? = (pos + 1 < chars.count) ? chars[pos + 1] : nil
        switch nextAfter {
        case let n? where matra(n) != nil: return true
        case "्": return true
        case nil: return false
        case let n? where !isDevanagari(n): return false
        case let n? where consonant(n) != nil: return true
        default: return true
        }
    }

    private static func isAsciiPunctuation(_ ch: Character) -> Bool {
        guard let scalar = ch.unicodeScalars.first, ch.unicodeScalars.count == 1 else { return false }
        let v = scalar.value
        return (0x21...0x2F).contains(v) || (0x3A...0x40).contains(v)
            || (0x5B...0x60).contains(v) || (0x7B...0x7E).contains(v)
    }
}
