import XCTest
@testable import AirNoteShared

/// Regression + invariant tests for the Hinglish (Devanagari→Roman) guard.
/// The non-negotiable invariant: enforceRomanHinglish never returns Devanagari,
/// never crashes, and is idempotent — for ANY input.
final class HinglishScriptTests: XCTestCase {

    private func hasDevanagari(_ s: String) -> Bool {
        s.unicodeScalars.contains { (0x0900...0x097F).contains(Int($0.value)) }
    }

    // MARK: Correctness

    func testKnownPhrases() {
        let cases: [(String, String)] = [
            ("anugrah है। tum kaise ho?", "anugrah hai. tum kaise ho?"),
            ("काम", "kaam"),
            ("बहुत", "bahut"),
            ("यह", "yah"),
            ("चाहिए", "chaahie"),
            ("आज", "aaj"),
            ("भी", "bhee"),
            ("यह बहुत अच्छा है yaar", "yah bahut achchaa hai yaar"),
            ("मेरा फोन नंबर ९८७६ है", "meraa phon nambar 9876 hai"),
            ("संभव है", "sambhav hai"),
            ("ॐ नमः", "om namah"),
        ]
        for (input, expected) in cases {
            XCTAssertEqual(HinglishScript.enforceRomanHinglish(input), expected, "input: \(input)")
        }
    }

    func testRomanAndEnglishLeftUnchanged() {
        let romans = [
            "Aaj bahut kaam tha, but deployment went fine.",
            "Hello, how are you?",
            "Mera naam Anubhav hai.",
            "Ship it to anugra @ 5pm — done.",
        ]
        for text in romans {
            XCTAssertEqual(HinglishScript.enforceRomanHinglish(text), text)
        }
    }

    // MARK: Invariant — no Devanagari can ever leak

    func testEveryDevanagariScalarProducesNoDevanagari() {
        for codepoint in 0x0900...0x097F {
            guard let scalar = Unicode.Scalar(codepoint) else { continue }
            let ch = String(scalar)
            for context in [ch, "x" + ch, ch + "y", "क" + ch, ch + "म", "hello " + ch + " world"] {
                let out = HinglishScript.enforceRomanHinglish(context)
                XCTAssertFalse(hasDevanagari(out), "U+\(String(format: "%04X", codepoint)) leaked in '\(context)' -> '\(out)'")
            }
        }
    }

    func testFuzzNeverLeaksAndIsIdempotent() {
        var rng = SystemRandomNumberGenerator()
        let pools: [[UInt32]] = [
            Array(0x0900...0x097F),
            Array(0x20...0x7E).map { UInt32($0) },
            [0x200C, 0x200D, 0x0964, 0x0965, 0x2014, 0x1F600, 0x4E2D, 0x094D, 0x0905],
        ]
        for _ in 0..<5000 {
            let n = Int.random(in: 0...50, using: &rng)
            var view = String.UnicodeScalarView()
            for _ in 0..<n {
                let pool = pools[Int.random(in: 0..<pools.count, using: &rng)]
                if let s = Unicode.Scalar(pool[Int.random(in: 0..<pool.count, using: &rng)]) { view.append(s) }
            }
            let input = String(view)
            let out = HinglishScript.enforceRomanHinglish(input)
            XCTAssertFalse(hasDevanagari(out), "leak on: \(input.unicodeScalars.map { String(format: "U+%04X", $0.value) })")
            XCTAssertEqual(HinglishScript.enforceRomanHinglish(out), out, "not idempotent")
        }
    }

    func testEdgeCasesDoNotCrash() {
        let edges = ["", "   ", "\n\t", "12345", "👍🏽 कैसे हो 中文 مرحبا", "क्ष्ण्र्त्र", "ऀँंःऄॐ",
                     String(repeating: "बहुत अच्छा है ", count: 5000)]
        for text in edges {
            let out = HinglishScript.enforceRomanHinglish(text)
            XCTAssertFalse(hasDevanagari(out))
        }
    }
}
