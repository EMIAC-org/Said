import UIKit
import AirNoteShared

struct TextInsertion {
    let documentProxy: UITextDocumentProxy

    var isUnsupportedSecureField: Bool {
        let surroundingText = [
            documentProxy.documentContextBeforeInput,
            documentProxy.documentContextAfterInput,
            documentProxy.selectedText
        ]
        .compactMap { $0 }
        .joined(separator: " ")
        .lowercased()

        let blockedHints = [
            "password",
            "passcode",
            "otp",
            "one time",
            "one-time",
            "verification code",
            "security code",
            "cvv",
            "card number"
        ]

        return blockedHints.contains { surroundingText.contains($0) }
    }

    @discardableResult
    func insert(_ result: BridgeResult) -> Bool {
        guard !isUnsupportedSecureField else {
            return false
        }
        switch result.insertPolicy {
        case .insertAtCursor:
            documentProxy.insertText(result.polished)
            return true
        case .replaceSelectedText:
            documentProxy.insertText(result.polished)
            return true
        case .copyOnly, .saveToHistory:
            return false
        }
    }
}
