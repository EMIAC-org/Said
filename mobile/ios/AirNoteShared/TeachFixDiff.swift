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
        currentBeforeCursor: String
    ) -> Outcome {
        var editedRaw = currentBeforeCursor
        if !insertPrefix.isEmpty, currentBeforeCursor.hasPrefix(insertPrefix) {
            editedRaw = String(currentBeforeCursor.dropFirst(insertPrefix.count))
        }
        let edited = editedRaw.trimmingCharacters(in: .whitespacesAndNewlines)
        let original = insertedText.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !edited.isEmpty else { return .empty }
        guard edited != original else { return .unchanged }
        return .edited(edited)
    }
}
