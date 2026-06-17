import XCTest
@testable import AirNoteShared

/// Tests for the keyboard's in-place "Teach a fix" diff — the logic that
/// isolates the user's edited version of just our insertion from the text around
/// the cursor, so we only call the server when there's a real correction.
final class TeachFixDiffTests: XCTestCase {
    func testDetectsEditAfterPrefix() {
        // Field already had "Hi ", we inserted "Karan Jaansi", user fixed it.
        let outcome = TeachFixDiff.evaluate(
            insertedText: "Karan Jaansi",
            insertPrefix: "Hi ",
            currentBeforeCursor: "Hi Karan Jhansi"
        )
        XCTAssertEqual(outcome, .edited("Karan Jhansi"))
    }

    func testNoPrefixStillDetectsEdit() {
        let outcome = TeachFixDiff.evaluate(
            insertedText: "Jaansi",
            insertPrefix: "",
            currentBeforeCursor: "Jhansi"
        )
        XCTAssertEqual(outcome, .edited("Jhansi"))
    }

    func testUnchangedInsertionReturnsUnchanged() {
        let outcome = TeachFixDiff.evaluate(
            insertedText: "Karan Jaansi",
            insertPrefix: "Hi ",
            currentBeforeCursor: "Hi Karan Jaansi"
        )
        XCTAssertEqual(outcome, .unchanged)
    }

    func testTrailingWhitespaceIsNotAnEdit() {
        let outcome = TeachFixDiff.evaluate(
            insertedText: "hello world",
            insertPrefix: "",
            currentBeforeCursor: "hello world   "
        )
        XCTAssertEqual(outcome, .unchanged)
    }

    func testDeletedInsertionReturnsEmpty() {
        let outcome = TeachFixDiff.evaluate(
            insertedText: "Karan Jaansi",
            insertPrefix: "Hi ",
            currentBeforeCursor: "Hi "
        )
        XCTAssertEqual(outcome, .empty)
    }

    func testAppendedTextIsTreatedAsAnEdit() {
        // Appending more words still produces a diff; the server's analyze-edit
        // decides whether any of it is a learnable correction (it no-ops on pure
        // additions), so it's safe to forward.
        let outcome = TeachFixDiff.evaluate(
            insertedText: "thanks",
            insertPrefix: "",
            currentBeforeCursor: "thanks a lot"
        )
        XCTAssertEqual(outcome, .edited("thanks a lot"))
    }

    func testPrefixMismatchFallsBackToFullContext() {
        // If the app truncated/changed the captured prefix, we fall back to the
        // full before-cursor text rather than crashing or mis-stripping.
        let outcome = TeachFixDiff.evaluate(
            insertedText: "Jaansi",
            insertPrefix: "Some old prefix that no longer matches",
            currentBeforeCursor: "Jhansi"
        )
        XCTAssertEqual(outcome, .edited("Jhansi"))
    }

    // MARK: Both-sides reconstruction (cursor left mid-insertion)

    func testReconstructsEditFromBothSidesOfCursor() {
        // We inserted "jaan bhavani ladle", the user fixed "jaan"->"jai" and left
        // the cursor right after "jai". Reading only the before-cursor text would
        // capture just "jai" and over-replace; both sides reconstruct the whole edit.
        let outcome = TeachFixDiff.evaluate(
            insertedText: "jaan bhavani ladle",
            insertPrefix: "",
            currentBeforeCursor: "jai",
            insertSuffix: "",
            currentAfterCursor: " bhavani ladle"
        )
        XCTAssertEqual(outcome, .edited("jai bhavani ladle"))
    }

    // MARK: changedSegments (every changed word-run is its own pair)

    private func pairs(_ original: String, _ edited: String) -> [[String]] {
        TeachFixDiff.changedSegments(original: original, edited: edited).map { [$0.heard, $0.correct] }
    }

    func testSingleWordCorrectionIsOnePair() {
        XCTAssertEqual(pairs("jaan bhavani ladle", "jai bhavani ladle"), [["jaan", "jai"]])
    }

    func testTwoNonAdjacentCorrectionsStaySeparate() {
        // The exact bug the user hit: "jaan … ladle" -> "jai … laddu" must teach
        // TWO precise rules, not collapse "jaan bhavani ladle" -> "jai".
        XCTAssertEqual(
            pairs("jaan bhavani ladle", "jai bhavani laddu"),
            [["jaan", "jai"], ["ladle", "laddu"]]
        )
    }

    func testAdjacentChangesAreOneSpan() {
        // Adjacent edits with no anchor between them are indistinguishable from a
        // phrase correction, so they form a single span.
        XCTAssertEqual(pairs("ankur gupta hello", "anugra das hello"), [["ankur gupta", "anugra das"]])
    }

    func testWholeReplacementCollapsesWhenNoCommonWords() {
        XCTAssertEqual(pairs("jaan bhavani ladle", "jai"), [["jaan bhavani ladle", "jai"]])
    }

    func testNoChangeYieldsNoPairs() {
        XCTAssertTrue(pairs("hello world", "hello world").isEmpty)
    }

    func testPureInsertionYieldsNoPair() {
        XCTAssertTrue(pairs("hello", "hello world").isEmpty)
    }

    func testPureDeletionYieldsNoPair() {
        XCTAssertTrue(pairs("hello world", "hello").isEmpty)
    }
}
