import UIKit
import AirNoteShared

struct ContextReader {
    let documentProxy: UITextDocumentProxy

    func read() -> KeyboardContext {
        KeyboardContext(
            beforeText: documentProxy.documentContextBeforeInput ?? "",
            afterText: documentProxy.documentContextAfterInput ?? "",
            selectedText: documentProxy.selectedText ?? "",
            hostAppLabel: "unknown",
            fieldHint: "unknown"
        )
    }
}
