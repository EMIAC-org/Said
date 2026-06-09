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
}
