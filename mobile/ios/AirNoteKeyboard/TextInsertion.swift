import UIKit
import AirNoteShared

struct TextInsertion {
    let documentProxy: UITextDocumentProxy

    func insert(_ result: BridgeResult) {
        switch result.insertPolicy {
        case .insertAtCursor:
            documentProxy.insertText(result.polished)
        case .replaceSelectedText:
            documentProxy.insertText(result.polished)
        case .copyOnly, .saveToHistory:
            break
        }
    }
}
