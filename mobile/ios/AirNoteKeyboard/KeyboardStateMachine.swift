import Foundation
import AirNoteShared

public enum KeyboardState: Equatable {
    case notConfigured
    case needsFullAccess
    case needsMainAppSession
    case ready
    case recording
    case processing(String)
    case insertReady(BridgeResult)
    case inserted
    case copied
    case savedToHistory
    case error(String)
    case staleSession
    case unsupportedSecureField
}

public struct KeyboardStateMachine {
    public private(set) var state: KeyboardState = .notConfigured
    public private(set) var lastInsertedResultSeq: UInt64 = 0

    public init() {}

    public mutating func apply(session: BridgeSession?) {
        guard let session else {
            state = .needsMainAppSession
            return
        }

        switch session.state {
        case .ready:
            state = .ready
        case .recording:
            state = .recording
        case .processing:
            state = .processing("Preparing insert")
        case .staleSession:
            state = .staleSession
        default:
            state = .needsMainAppSession
        }
    }

    public mutating func apply(result: BridgeResult) -> Bool {
        guard result.resultSeq > lastInsertedResultSeq else {
            return false
        }
        state = .insertReady(result)
        return true
    }

    public mutating func acknowledgeInserted(resultSeq: UInt64) {
        lastInsertedResultSeq = max(lastInsertedResultSeq, resultSeq)
        state = .inserted
    }

    public mutating func acknowledgeCopied(resultSeq: UInt64) {
        lastInsertedResultSeq = max(lastInsertedResultSeq, resultSeq)
        state = .copied
    }

    public mutating func acknowledgeSaved(resultSeq: UInt64) {
        lastInsertedResultSeq = max(lastInsertedResultSeq, resultSeq)
        state = .savedToHistory
    }

    public mutating func markUnsupportedSecureField() {
        state = .unsupportedSecureField
    }
}
