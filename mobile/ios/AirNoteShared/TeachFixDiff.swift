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

    /// EVERY changed word-run between the inserted text and the user's edit, via a
    /// word-level LCS: the common words are anchors, and each gap between anchors
    /// is its own original→corrected pair. So correcting several words in one go —
    /// even non-adjacent ("jaan … ladle" → "jai … laddu") — teaches each one
    /// exactly, instead of collapsing them into a single wrong whole-phrase rule.
    public static func changedSegments(original: String, edited: String) -> [(heard: String, correct: String)] {
        let o = original.split { $0 == " " || $0 == "\t" || $0 == "\n" }.map(String.init)
        let e = edited.split { $0 == " " || $0 == "\t" || $0 == "\n" }.map(String.init)
        guard !o.isEmpty, !e.isEmpty else { return [] }

        // LCS length DP over case-insensitive token equality.
        let n = o.count, m = e.count
        var dp = Array(repeating: Array(repeating: 0, count: m + 1), count: n + 1)
        for i in stride(from: n - 1, through: 0, by: -1) {
            for j in stride(from: m - 1, through: 0, by: -1) {
                if o[i].caseInsensitiveCompare(e[j]) == .orderedSame {
                    dp[i][j] = dp[i + 1][j + 1] + 1
                } else {
                    dp[i][j] = max(dp[i + 1][j], dp[i][j + 1])
                }
            }
        }
        // Backtrack to the matched anchor index-pairs.
        var anchors: [(Int, Int)] = []
        var i = 0, j = 0
        while i < n, j < m {
            if o[i].caseInsensitiveCompare(e[j]) == .orderedSame {
                anchors.append((i, j)); i += 1; j += 1
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                i += 1
            } else {
                j += 1
            }
        }
        // Each gap (between anchors, and the trailing gap via the sentinel) where
        // BOTH sides have words is a real substitution — a learnable pair. Pure
        // insertions/deletions (one side empty) are skipped.
        var segments: [(heard: String, correct: String)] = []
        var oi = 0, ei = 0
        for (mo, me) in anchors + [(n, m)] {
            if oi < mo, ei < me {
                segments.append((o[oi..<mo].joined(separator: " "), e[ei..<me].joined(separator: " ")))
            }
            oi = mo + 1
            ei = me + 1
        }
        return segments
    }
}
