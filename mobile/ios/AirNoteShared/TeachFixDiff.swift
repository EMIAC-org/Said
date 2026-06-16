import Foundation

/// Pure logic for the keyboard's in-place "Teach a fix" feature.
///
/// While AirNote's keyboard is the active keyboard, it can read the text around
/// the cursor (`textDocumentProxy.documentContextBeforeInput`). After we insert a
/// polished result, the user may correct a word using this keyboard. To learn
/// from that, we need the user's edited version of *just our insertion* — not the
/// text that was already in the field. This isolates it by stripping the prefix
/// captured at insert time, and classifies the result so the keyboard can show
/// the right message and only call the server when there's a real correction.
public enum TeachFixDiff {
    public enum Outcome: Equatable {
        /// Nothing left after the insertion — nudge the user to fix it first.
        case empty
        /// The insertion is unchanged — nothing to teach.
        case unchanged
        /// The user edited the insertion to this corrected text.
        case edited(String)
    }

    public static func evaluate(
        insertedText: String,
        insertPrefix: String,
        currentBeforeCursor: String,
        insertSuffix: String = "",
        currentAfterCursor: String = ""
    ) -> Outcome {
        // The user may leave the cursor mid-insertion (e.g. fix "jaan"->"jai" and
        // not move to the end), so reconstruct the WHOLE edited insertion from both
        // sides: drop the captured prefix from the before-cursor text and the
        // captured suffix from the after-cursor text, then join. Reading only the
        // before-cursor text captured a partial edit and over-replaced the phrase.
        var beforePart = currentBeforeCursor
        if !insertPrefix.isEmpty, currentBeforeCursor.hasPrefix(insertPrefix) {
            beforePart = String(currentBeforeCursor.dropFirst(insertPrefix.count))
        }
        var afterPart = currentAfterCursor
        if !insertSuffix.isEmpty, currentAfterCursor.hasSuffix(insertSuffix) {
            afterPart = String(currentAfterCursor.dropLast(insertSuffix.count))
        }
        let edited = (beforePart + afterPart).trimmingCharacters(in: .whitespacesAndNewlines)
        let original = insertedText.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !edited.isEmpty else { return .empty }
        guard edited != original else { return .unchanged }
        return .edited(edited)
    }

    /// The minimal changed word-span between what we inserted and what the user
    /// edited it to — strips the common leading/trailing words so a fixed name
    /// ("ankur gupta" → "anugra") is isolated from the surrounding sentence and
    /// can be stored as a precise heard→meant rule.
    public static func changedSegment(original: String, edited: String) -> (heard: String, correct: String)? {
        let o = original.split { $0 == " " || $0 == "\t" || $0 == "\n" }.map(String.init)
        let e = edited.split { $0 == " " || $0 == "\t" || $0 == "\n" }.map(String.init)
        guard !o.isEmpty, !e.isEmpty else { return nil }
        var start = 0
        while start < o.count, start < e.count, o[start].caseInsensitiveCompare(e[start]) == .orderedSame {
            start += 1
        }
        var oEnd = o.count, eEnd = e.count
        while oEnd > start, eEnd > start, o[oEnd - 1].caseInsensitiveCompare(e[eEnd - 1]) == .orderedSame {
            oEnd -= 1
            eEnd -= 1
        }
        let heard = o[start..<oEnd].joined(separator: " ")
        let correct = e[start..<eEnd].joined(separator: " ")
        guard !heard.isEmpty, !correct.isEmpty else { return nil }
        return (heard, correct)
    }
}
