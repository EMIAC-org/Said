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

    // MARK: Single lowercase word — the user's real case

    func testLowercaseMishearingIsLearned() {
        // "jaan" -> "jai" and "ladle" -> "laddu" are lowercase single words but
        // clear STT mis-hearings (shared onset, close spelling). These MUST learn —
        // this is exactly what the user reported wasn't working.
        XCTAssertTrue(LearnedAliasResolver.isSafe(heard: "jaan", correct: "jai"))
        XCTAssertTrue(LearnedAliasResolver.isSafe(heard: "ladle", correct: "laddu"))
    }

    func testLowercaseRephraseIsRejected() {
        // A different word entirely (different onset) is a rephrase, not a
        // mis-hearing — never learn it as a blanket rewrite.
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "hello", correct: "world"))
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "great", correct: "awful"))
    }

    func testEditDistanceCapBlocksHalfWordRewrites() {
        // The single-word apply gate caps distance at 2, so a distinct word that
        // merely shares an onset can't ride in.
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "breakfast", correct: "breakdown"))
        XCTAssertTrue(LearnedAliasResolver.isSafe(heard: "jaan", correct: "jai"))
    }

    func testEditDistanceSanity() {
        XCTAssertEqual(LearnedAliasResolver.editDistance("jaan", "jai"), 2)
        XCTAssertEqual(LearnedAliasResolver.editDistance("ladle", "laddu"), 2)
        XCTAssertEqual(LearnedAliasResolver.editDistance("abc", "abc"), 0)
        XCTAssertEqual(LearnedAliasResolver.editDistance("", "abc"), 3)
    }

    // MARK: STORE-time gate — homophone / word-swap corruption guard
    //
    // These are the cases the adversarial review reproduced: teaching a real-word
    // homophone once would otherwise become a permanent global rewrite that
    // corrupts correct dictation. isSafeToLearn must refuse them while still
    // learning genuine name mis-hearings.

    func testHomophonesAreNeverLearned() {
        // High-frequency homophones (caught by the common-word backstop).
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "their", correct: "there"))
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "loose", correct: "lose"))
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "were", correct: "where"))
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "week", correct: "weak"))
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "form", correct: "from"))
    }

    func testRealWordSwapsNotInCommonListAreStillRefused() {
        // Not in the common-word list, but both are real dictionary words — the
        // dictionary check must still refuse them.
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "desert", correct: "dessert"))
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "advice", correct: "advise"))
    }

    func testGenuineNameMishearingsAreLearned() {
        // At least one side is a coined term the dictionary doesn't know → learn it.
        XCTAssertTrue(LearnedAliasResolver.isSafeToLearn(heard: "jaan", correct: "jai"))
        XCTAssertTrue(LearnedAliasResolver.isSafeToLearn(heard: "ladle", correct: "laddu"))
        XCTAssertTrue(LearnedAliasResolver.isSafeToLearn(heard: "anukar", correct: "Anugra"))
    }

    func testMultiWordRephraseIsNotLearned() {
        // All-ordinary-words multi-word edit is a rephrase, not a name fix.
        XCTAssertFalse(LearnedAliasResolver.isSafeToLearn(heard: "see you tomorrow", correct: "call me later"))
    }

    func testMultiWordNameCorrectionIsLearned() {
        XCTAssertTrue(LearnedAliasResolver.isSafeToLearn(heard: "ankur gupta", correct: "anugra"))
        XCTAssertTrue(LearnedAliasResolver.isSafeToLearn(heard: "super base", correct: "Supabase"))
    }

    func testApplyGateRejectsCommonHomophone() {
        // Defense in depth: even the cheap apply-time gate rejects a common
        // homophone, so a stale stored rule can't corrupt output.
        XCTAssertFalse(LearnedAliasResolver.isSafe(heard: "their", correct: "there"))
    }

    // MARK: End-to-end — diff → safety gate → apply (the whole learn-from-edit path)

    func testMultiWordTeachThenApply() {
        // 1) The user fixed "jaan bhavani ladle" -> "jai bhavani laddu".
        // 2) The diff isolates each changed word.
        // 3) The safety gate keeps both (lowercase mis-hearings).
        // 4) Next dictation of the same phrase is auto-corrected.
        var learned: [LearnedAliasPair] = []
        for seg in TeachFixDiff.changedSegments(original: "jaan bhavani ladle", edited: "jai bhavani laddu") {
            if LearnedAliasResolver.isSafeToLearn(heard: seg.heard, correct: seg.correct) {
                learned.append(LearnedAliasPair(heard: seg.heard, correct: seg.correct))
            }
        }
        XCTAssertEqual(learned.count, 2)
        let out = LearnedAliasResolver.apply(
            "jaan bhavani ladle",
            transcript: "jaan bhavani ladle",
            aliases: learned
        )
        XCTAssertEqual(out, "jai bhavani laddu")
    }

    func testTaughtNameLearnedFromHistoryAppliesLater() {
        // "ankur gupta" -> "anugra" (multi-word) learns, then applies to new text.
        var learned: [LearnedAliasPair] = []
        for seg in TeachFixDiff.changedSegments(original: "tell ankur gupta hi", edited: "tell anugra hi") {
            if LearnedAliasResolver.isSafeToLearn(heard: seg.heard, correct: seg.correct) {
                learned.append(LearnedAliasPair(heard: seg.heard, correct: seg.correct))
            }
        }
        XCTAssertFalse(learned.isEmpty)
        let out = LearnedAliasResolver.apply(
            "ankur gupta is here",
            transcript: "ankur gupta is here",
            aliases: learned
        )
        XCTAssertEqual(out, "anugra is here")
    }
}
