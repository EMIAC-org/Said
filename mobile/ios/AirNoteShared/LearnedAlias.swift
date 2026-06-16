import Foundation
#if canImport(UIKit)
import UIKit
#endif

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
/// casing wins, and common words are never touched.
///
/// Two gates, deliberately separated (storage is stricter than application):
/// - `isSafeToLearn` (store-time, rare): rejects homophone/word-swaps and
///   all-ordinary-word rephrases using the system dictionary, so a real word can
///   never become a permanent global rewrite. A single lowercase target is only
///   learned when it looks like a name mis-hearing (shared onset + tiny edit
///   distance) AND isn't a real dictionary word on both sides.
/// - `isSafe` (apply-time, every dictation): a cheap structural backstop. Every
///   stored rule has already passed `isSafeToLearn`, so this only needs to be
///   fast, not exhaustive.
public enum LearnedAliasResolver {
    /// Common English + Hinglish words (incl. classic homophones) a learned alias
    /// must never involve — the cheap apply-time backstop for over-correction.
    private static let commonWords: Set<String> = [
        // English function + very high frequency
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "to", "too",
        "two", "of", "in", "on", "at", "and", "or", "but", "not", "no", "for",
        "it", "its", "i", "you", "your", "he", "she", "we", "they", "them", "this",
        "that", "these", "those", "do", "does", "did", "go", "can", "could", "will",
        "would", "should", "have", "has", "had", "get", "got", "with", "width",
        "from", "form", "as", "if", "then", "than", "so", "my", "me", "him", "her",
        "our", "their", "there", "here", "hear", "where", "wear", "what", "when",
        "who", "why", "how", "which", "all", "any", "some", "more", "most", "now",
        "new", "knew", "know", "see", "sea", "say", "said", "way", "day", "time",
        "year", "week", "weak", "well", "man", "men", "woman", "women", "sun",
        "son", "sit", "site", "set", "let", "put", "run", "ran", "won", "one",
        "want", "wont", "big", "bad", "bed", "bag", "top", "car", "care", "bar",
        "bare", "bear", "off", "out", "about", "up", "down", "left", "right",
        "write", "read", "red", "road", "rode", "by", "buy", "bye", "pay", "paid",
        "plan", "plane", "plain", "quite", "quiet", "quit", "loose", "lose",
        "advice", "advise", "desert", "dessert", "brake", "break", "personal",
        "personnel", "weather", "whether", "gray", "grey", "cot", "cut", "cat",
        "hop", "hope", "test", "text", "piece", "peace", "four", "nice", "life",
        "like", "good", "great", "small", "long", "short", "data", "very", "just",
        // Hinglish high frequency
        "main", "mein", "mai", "hai", "hain", "ho", "hota", "hoti", "ka", "ke",
        "ki", "ko", "se", "par", "pe", "kya", "nahi", "nahin", "haan", "bhai",
        "yaar", "kaam", "acha", "accha", "theek", "thik", "bahut", "kuch", "sab",
        "abhi", "kal", "aaj", "kar", "karo", "kyun", "kaise", "kaisa", "kahan",
        "yahan", "wahan", "tum", "aap", "hum", "mera", "meri", "tera", "liye",
        "wala", "wali", "bas", "phir", "toh",
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

    // MARK: Safety

    /// Apply-time gate — cheap, runs on EVERY dictation via `apply()`. A rule only
    /// reaches the stored set after passing the stricter `isSafeToLearn`, so this
    /// is a fast structural backstop, not the primary guard.
    static func isSafe(heard: String, correct: String) -> Bool {
        let h = heard.trimmingCharacters(in: .whitespacesAndNewlines)
        let c = correct.trimmingCharacters(in: .whitespacesAndNewlines)
        let hn = h.lowercased(), cn = c.lowercased()
        guard h.count >= 2, !hn.isEmpty, !cn.isEmpty, hn != cn else { return false }
        guard wordCount(h) <= 5, wordCount(c) <= 5 else { return false }
        guard !isCommon(hn), !isCommon(cn) else { return false }
        // A protected target (Name/brand/code) or a multi-word phrase is always
        // safe to apply.
        let targetIsProtected = c.contains { $0.isUppercase || $0.isNumber || "_-./@".contains($0) }
        if targetIsProtected || wordCount(h) > 1 { return true }
        // A single lowercase word (a name typed lowercase, e.g. "jaan" -> "jai") is
        // applied ONLY when it looks like a real STT mis-hearing — shared onset +
        // a TINY edit distance (cap 2) so a distinct word ("breakfast" ->
        // "breakdown") can't ride in. A rephrase ("hello" -> "world") is rejected
        // by the onset mismatch.
        return hn.first == cn.first && editDistance(hn, cn) <= 2
    }

    /// Store-time gate — stricter; the user (or server) is about to PERSIST a rule
    /// that will auto-apply to every future dictation, so reject anything that
    /// would corrupt ordinary speech. Uses the system dictionary so a real
    /// word -> real word swap (a homophone like "their" -> "there") is refused,
    /// while a name mis-hearing ("jaan" -> "jai", "ladle" -> "laddu") is kept.
    public static func isSafeToLearn(heard: String, correct: String) -> Bool {
        guard isSafe(heard: heard, correct: correct) else { return false }
        let h = heard.trimmingCharacters(in: .whitespacesAndNewlines)
        let c = correct.trimmingCharacters(in: .whitespacesAndNewlines)
        let hn = h.lowercased(), cn = c.lowercased()
        let protectedTarget = c.contains { $0.isUppercase || $0.isNumber || "_-./@".contains($0) }
        if protectedTarget { return true }
        if wordCount(h) > 1 || wordCount(c) > 1 {
            // Multi-word, lowercase: allow a name/term correction (some token is a
            // coined word), refuse an all-ordinary-words rephrase like
            // "see you tomorrow" -> "call me later".
            let tokens = (hn + " " + cn).split { $0 == " " || $0 == "\t" || $0 == "\n" }.map(String.init)
            return tokens.contains { !isCommon($0) && !isRealWord($0) }
        }
        // Single lowercase word: refuse only if BOTH sides are real dictionary
        // words (a homophone/word swap); allow when either side is a coined
        // term/name the dictionary doesn't know.
        return !(isRealWord(hn) && isRealWord(cn))
    }

    /// Whether the system dictionary recognises `word` (a real English word). Used
    /// only at store time. Returns false (treat as a coined term) when no spell
    /// checker is available — the structural gates still apply.
    static func isRealWord(_ word: String) -> Bool {
        let w = word.trimmingCharacters(in: .whitespacesAndNewlines)
        guard w.count >= 2 else { return false }
        #if canImport(UIKit)
        let checker = UITextChecker()
        let range = NSRange(location: 0, length: w.utf16.count)
        let misspelled = checker.rangeOfMisspelledWord(
            in: w, range: range, startingAt: 0, wrap: false, language: "en_US"
        )
        return misspelled.location == NSNotFound
        #else
        return false
        #endif
    }

    /// Levenshtein edit distance, for the single-word mis-hearing check.
    static func editDistance(_ a: String, _ b: String) -> Int {
        let s = Array(a), t = Array(b)
        if s.isEmpty { return t.count }
        if t.isEmpty { return s.count }
        var prev = Array(0...t.count)
        var cur = Array(repeating: 0, count: t.count + 1)
        for i in 1...s.count {
            cur[0] = i
            for j in 1...t.count {
                let cost = s[i - 1] == t[j - 1] ? 0 : 1
                cur[j] = Swift.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost)
            }
            swap(&prev, &cur)
        }
        return prev[t.count]
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
