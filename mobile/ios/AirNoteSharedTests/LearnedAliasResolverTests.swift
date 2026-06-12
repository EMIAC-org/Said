import XCTest
@testable import AirNoteShared

/// Tests for the on-device learned-correction resolver. The whole point is to
/// fix taught names WITHOUT over-correcting, so most of these guard the failure
/// modes (substrings, common words, lowercase targets, missing evidence).
final class LearnedAliasResolverTests: XCTestCase {
    private func alias(_ h: String, _ c: String) -> LearnedAliasPair { LearnedAliasPair(heard: h, correct: c) }

    func testFixesTaughtName() {
        let out = LearnedAliasResolver.apply(
            "message anukar today",
            transcript: "message anukar today",
            aliases: [alias("anukar", "Anugra")]
        )
        XCTAssertEqual(out, "message Anugra today")
    }

    func testCaseInsensitiveMatchKeepsStoredCasing() {
        let out = LearnedAliasResolver.apply(
            "Anukar and ANUKAR",
            transcript: "Anukar and ANUKAR",
            aliases: [alias("anukar", "Anugra")]
        )
        XCTAssertEqual(out, "Anugra and Anugra")
    }

    func testNeverReplacesInsideAnotherWord() {
        // "anukari" must stay intact — whole-word match only.
        let out = LearnedAliasResolver.apply(
            "anukari is not anukar",
            transcript: "anukari is not anukar",
            aliases: [alias("anukar", "Anugra")]
        )
        XCTAssertEqual(out, "anukari is not Anugra")
    }

    func testMultiWordLongestFirst() {
        let out = LearnedAliasResolver.apply(
            "deploy to super base now",
            transcript: "deploy to super base now",
            aliases: [alias("super base", "Supabase"), alias("base", "Base")]
        )
        XCTAssertEqual(out, "deploy to Supabase now")
    }

    func testLowercaseCommonTargetIsRejected() {
        // target "bar" is an ordinary lowercase word → must NOT auto-apply.
        let out = LearnedAliasResolver.apply(
            "go to the foo",
            transcript: "go to the foo",
            aliases: [alias("foo", "bar")]
        )
        XCTAssertEqual(out, "go to the foo")
    }

    func testCommonWordSourceIsRejected() {
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "hai", correct: "Hi"))
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "main", correct: "Main"))
    }

    func testNoEvidenceNoChange() {
        // The heard form isn't present anywhere → leave output alone.
        let out = LearnedAliasResolver.apply(
            "hello world",
            transcript: "hello world",
            aliases: [alias("anukar", "Anugra")]
        )
        XCTAssertEqual(out, "hello world")
    }

    func testAlreadyCorrectIsLeftAlone() {
        let out = LearnedAliasResolver.apply(
            "message Anugra",
            transcript: "message anukar",
            aliases: [alias("anukar", "Anugra")]
        )
        XCTAssertEqual(out, "message Anugra")
    }

    func testProperNounTargetWithDigitsAndSymbolsAllowed() {
        XCTAssertTrue(LearnedAliasResolver.isSafe(heard: "n eight n", correct: "n8n"))
        XCTAssertTrue(LearnedAliasResolver.isSafe(heard: "super base", correct: "Supabase"))
        XCTAssertTrue(LearnedAliasResolver.isSafe(heard: "anukar", correct: "Anugra"))
    }

    func testEmptyAndIdentityRejected() {
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "", correct: "X"))
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "abc", correct: "abc"))
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "a", correct: "Anugra")) // too short
    }
}
