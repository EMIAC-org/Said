import Foundation

/// Pure text-scoping helpers used by the keyboard's "select → rewrite" resolver.
/// Kept in AirNoteShared (not the keyboard target) so it can be unit-tested.
public enum TextScope {
    /// The trailing sentence of `text` — split on `.`/`!`/`?`/newline — or the whole
    /// buffer (capped) when there's no terminator. Returns nil for empty/whitespace
    /// input. This is the keyboard's "the sentence before the cursor" fallback target.
    public static func lastSentence(in text: String, cap: Int = 240) -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let r = trimmed.range(of: "[.!?\\n]\\s*", options: [.regularExpression, .backwards]) {
            let tail = String(trimmed[r.upperBound...]).trimmingCharacters(in: .whitespacesAndNewlines)
            if !tail.isEmpty { return tail }
        }
        return String(trimmed.suffix(cap))
    }
}
