import UIKit
import AirNoteShared

struct ContextReader {
    let documentProxy: UITextDocumentProxy

    func read() -> KeyboardContext {
        KeyboardContext(
            beforeText: String((documentProxy.documentContextBeforeInput ?? "").suffix(280)),
            afterText: String((documentProxy.documentContextAfterInput ?? "").prefix(120)),
            selectedText: documentProxy.selectedText ?? "",
            // iOS deliberately hides the host app's identity from keyboards.
            hostAppLabel: "unknown",
            fieldHint: Self.fieldHint(for: documentProxy)
        )
    }

    /// Infers the field type from the host's requested keyboard so the server can
    /// tune polish (e.g. don't capitalize an email address).
    static func fieldHint(for proxy: UITextDocumentProxy) -> String {
        switch proxy.keyboardType {
        case .emailAddress:
            return "email"
        case .URL, .webSearch:
            return "url"
        case .numberPad, .phonePad, .decimalPad, .asciiCapableNumberPad:
            return "number"
        case .twitter:
            return "social"
        case .namePhonePad:
            return "name"
        default:
            return "text"
        }
    }
}
