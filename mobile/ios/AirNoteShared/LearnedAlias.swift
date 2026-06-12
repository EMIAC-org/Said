import Foundation

/// A single learned correction: the STT mis-heard `heard`, the user meant
/// `correct`. Stored locally (App Group) when the user teaches, so the client
/// can apply it to dictation output even though the server's streaming path
/// doesn't merge learned aliases.
public struct LearnedAliasPair: Codable, Equatable, Hashable {
    public let heard: String
    public let correct: String
    public init(heard: String, correct: String) {
        self.heard = heard
        self.correct = correct
    }
}

/// Applies a personal "heard -> meant" dictionary to dictation output, on-device.
///
/// Deliberately EXACT and conservative — ported from the desktop's proven
/// server-side resolver and the research consensus: whole-word/boundary-gated
/// match only (never substrings), longest-phrase-first, the target's stored
/// casing wins, common words are never touched, and a correction is only applied
/// when the target looks like a name/brand/code (capital, digit, or symbol) so an
/// ordinary lowercase word can't be hijacked. No fuzzy/phonetic matching — that
/// over-corrects; instead each specific mis-spelling is stored as its own rule.
public enum LearnedAliasResolver {
    /// Common English + Hinglish words a learned alias must never involve.
    private static let commonWords: Set<String> = [
        "the", "a", "an", "is", "to", "and", "of", "in", "on", "for", "it", "i",
        "you", "me", "we", "they", "this", "that", "be", "do", "go", "can", "will",
        "main", "mein", "mai", "hai", "ho", "ka", "ke", "ki", "ko", "se", "par",
        "pe", "kya", "nahi", "haan", "bhai", "yaar", "kaam", "time", "data",
    ]

    public static func apply(_ output: String, transcript: String, aliases: [LearnedAliasPair]) -> String {
        guard !aliases.isEmpty else { return output }

        // Longest phrase first (more words, then more characters) so multi-word
        // corrections win before their single-word parts.
        let rules = aliases
            .filter { isSafe(heard: $0.heard, correct: $0.correct) }
            .sorted {
                let lw = wordCount($0.heard), rw = wordCount($1.heard)
                return lw != rw ? lw > rw : $0.heard.count > $1.heard.count
            }

        var result = output
        for rule in rules {
            // Already correct in the output — nothing to do.
            if containsWholeWord(result, rule.correct) { continue }
            // Evidence: the heard form must actually appear (in the raw transcript
            // or the output) before we rewrite anything.
            guard containsWholeWord(transcript, rule.heard) || containsWholeWord(result, rule.heard) else { continue }
            result = replaceWholeWord(in: result, heard: rule.heard, correct: rule.correct)
        }
        return result
    }

    // MARK: Safety (ported from is_runtime_exact_alias_safe)

    static func isSafe(heard: String, correct: String) -> Bool {
        let h = heard.trimmingCharacters(in: .whitespacesAndNewlines)
        let c = correct.trimmingCharacters(in: .whitespacesAndNewlines)
        let hn = h.lowercased(), cn = c.lowercased()
        guard h.count >= 2, !hn.isEmpty, !cn.isEmpty, hn != cn else { return false }
        guard wordCount(h) <= 4, wordCount(c) <= 4 else { return false }
        guard !isCommon(hn), !isCommon(cn) else { return false }
        // The target must look like a custom term (name/brand/code), not an
        // ordinary lowercase word — else we'd risk rewriting normal speech.
        let targetIsProtected = c.contains { $0.isUppercase || $0.isNumber || "_-./@".contains($0) }
        return targetIsProtected || wordCount(h) > 1
    }

    private static func isCommon(_ normalized: String) -> Bool {
        wordCount(normalized) == 1 && commonWords.contains(normalized)
    }

    // MARK: Helpers

    static func wordCount(_ text: String) -> Int {
        text.split { $0 == " " || $0 == "\t" || $0 == "\n" }.filter { !$0.isEmpty }.count
    }

    /// Whole-word, case-insensitive, Unicode-boundary-aware containment.
    static func containsWholeWord(_ text: String, _ phrase: String) -> Bool {
        guard let regex = boundaryRegex(for: phrase) else { return false }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        return regex.firstMatch(in: text, options: [], range: range) != nil
    }

    /// Replace every whole-word occurrence of `heard` with `correct` (stored
    /// casing preserved), leaving substrings inside larger words untouched.
    static func replaceWholeWord(in text: String, heard: String, correct: String) -> String {
        guard let regex = boundaryRegex(for: heard) else { return text }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        let template = NSRegularExpression.escapedTemplate(for: correct)
        return regex.stringByReplacingMatches(in: text, options: [], range: range, withTemplate: template)
    }

    private static func boundaryRegex(for phrase: String) -> NSRegularExpression? {
        let trimmed = phrase.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        // Collapse internal whitespace so a multi-word phrase matches any run of
        // spaces between its words.
        let words = trimmed.split { $0 == " " || $0 == "\t" || $0 == "\n" }
        let escaped = words.map { NSRegularExpression.escapedPattern(for: String($0)) }.joined(separator: "\\s+")
        // (?<![\w]) / (?![\w]) are Unicode-letter boundaries that also treat
        // digits/underscore as word chars — closer to intent than \b for names.
        let pattern = "(?<![\\p{L}\\p{N}_])\(escaped)(?![\\p{L}\\p{N}_])"
        return try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive])
    }
}
