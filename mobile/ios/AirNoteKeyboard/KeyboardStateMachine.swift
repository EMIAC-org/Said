import Foundation
import AirNoteShared

public enum KeyboardState: Equatable {
    case notConfigured
    case needsFullAccess
    case needsMainAppSession
    case ready
    case recording
    case dictatingInApp   // app is recording (handoff); user should swipe back
    case processing(String)
    case insertReady(BridgeResult)
    case secureCopyReady(BridgeResult)
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
        case .notConfigured:
            state = .notConfigured
        case .needsFullAccess:
            state = .needsFullAccess
        case .needsMainAppSession:
            state = .needsMainAppSession
        case .sessionStartRequested:
            state = .processing("Opening AirNote Session")
        case .ready:
            state = .ready
        case .recording:
            state = .recording
        case .processing:
            state = .processing("Preparing insert")
        case .insertReady:
            switch state {
            case .inserted, .copied, .savedToHistory:
                break
            default:
                state = .processing("Preparing insert")
            }
        case .inserted:
            state = .inserted
        case .error:
            state = .error("Retry available from AirNote.")
        case .staleSession:
            state = .staleSession
        }
    }

    public mutating func apply(result: BridgeResult, secureField: Bool = false) -> Bool {
        guard result.resultSeq > lastInsertedResultSeq else {
            return false
        }
        state = secureField ? .secureCopyReady(result) : .insertReady(result)
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
        if case .insertReady(let result) = state {
            state = .secureCopyReady(result)
        } else {
            state = .unsupportedSecureField
        }
    }
}
