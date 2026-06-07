import Foundation
import AirNoteShared

enum SessionState: Equatable {
    case idle
    case needsMainAppSession
    case ready
    case recording(startedAt: Date)
    case processing
    case insertReady(BridgeResult)
    case inserted
    case savedToHistory
    case retryableError(String)
    case stale
}

extension SessionState {
    var isLive: Bool {
        if case .recording = self { return true }
        return self == .ready || self == .processing
    }
}
