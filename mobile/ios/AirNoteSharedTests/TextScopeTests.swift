import XCTest
@testable import AirNoteShared

/// Tests for the keyboard rewrite resolver's "sentence before the cursor" fallback.
final class TextScopeTests: XCTestCase {

    func testTrailingSentenceAfterTerminator() {
        XCTAssertEqual(TextScope.lastSentence(in: "Hi there. Please send the file"), "Please send the file")
        XCTAssertEqual(TextScope.lastSentence(in: "Done! kaam ho gaya"), "kaam ho gaya")
        XCTAssertEqual(TextScope.lastSentence(in: "Are you free?"), "Are you free?")
    }

    func testSplitsOnNewline() {
        XCTAssertEqual(TextScope.lastSentence(in: "line one\nline two"), "line two")
    }

    func testWholeBufferWhenNoTerminator() {
        XCTAssertEqual(TextScope.lastSentence(in: "just a phrase"), "just a phrase")
    }

    func testEmptyAndWhitespaceReturnNil() {
        XCTAssertNil(TextScope.lastSentence(in: ""))
        XCTAssertNil(TextScope.lastSentence(in: "   \n  "))
    }

    func testCapsLongBuffer() {
        let long = String(repeating: "a", count: 500)
        XCTAssertEqual(TextScope.lastSentence(in: long, cap: 240)?.count, 240)
    }
}
