import Foundation

public enum LanguageHint: String, Codable, CaseIterable {
    case auto
    case en
    case hi
    case hinglish
}

public enum DictationStyle: String, Codable, CaseIterable {
    case direct
    case work
    case casual
    case email
    case notes
}

public enum MobileSurface: String, Codable {
    case iosKeyboard = "ios_keyboard"
    case iosActionButton = "ios_action_button"
}

public enum BridgeSessionState: String, Codable {
    case notConfigured = "not_configured"
    case needsFullAccess = "needs_full_access"
    case needsMainAppSession = "needs_main_app_session"
    case sessionStartRequested = "session_start_requested"
    case ready
    case recording
    case processing
    case insertReady = "insert_ready"
    case inserted
    case error
    case staleSession = "stale_session"
}

public struct BridgeSession: Codable, Equatable {
    public var schema: String
    public var sessionID: String
    public var deviceID: String
    public var state: BridgeSessionState
    public var startedAt: Date
    public var expiresAt: Date
    public var heartbeatAt: Date
    public var languageHint: LanguageHint
    public var style: DictationStyle
    public var surface: MobileSurface
    public var gatewayRegion: String
    public var resultSeq: UInt64
    public var commandSeq: UInt64

    public init(
        schema: String = "airnote.ios.bridge.session.v1",
        sessionID: String,
        deviceID: String,
        state: BridgeSessionState,
        startedAt: Date,
        expiresAt: Date,
        heartbeatAt: Date,
        languageHint: LanguageHint,
        style: DictationStyle,
        surface: MobileSurface,
        gatewayRegion: String,
        resultSeq: UInt64,
        commandSeq: UInt64
    ) {
        self.schema = schema
        self.sessionID = sessionID
        self.deviceID = deviceID
        self.state = state
        self.startedAt = startedAt
        self.expiresAt = expiresAt
        self.heartbeatAt = heartbeatAt
        self.languageHint = languageHint
        self.style = style
        self.surface = surface
        self.gatewayRegion = gatewayRegion
        self.resultSeq = resultSeq
        self.commandSeq = commandSeq
    }

    enum CodingKeys: String, CodingKey {
        case schema
        case sessionID = "session_id"
        case deviceID = "device_id"
        case state
        case startedAt = "started_at"
        case expiresAt = "expires_at"
        case heartbeatAt = "heartbeat_at"
        case languageHint = "language_hint"
        case style
        case surface
        case gatewayRegion = "gateway_region"
        case resultSeq = "result_seq"
        case commandSeq = "command_seq"
    }
}

public struct KeyboardContext: Codable, Equatable {
    public var beforeText: String
    public var afterText: String
    public var selectedText: String
    public var hostAppLabel: String
    public var fieldHint: String

    public init(beforeText: String, afterText: String, selectedText: String, hostAppLabel: String, fieldHint: String) {
        self.beforeText = beforeText
        self.afterText = afterText
        self.selectedText = selectedText
        self.hostAppLabel = hostAppLabel
        self.fieldHint = fieldHint
    }

    enum CodingKeys: String, CodingKey {
        case beforeText = "before_text"
        case afterText = "after_text"
        case selectedText = "selected_text"
        case hostAppLabel = "host_app_label"
        case fieldHint = "field_hint"
    }
}

public enum BridgeCommandKind: String, Codable {
    case startSession = "start_session"
    case startRecording = "start_recording"
    case stopRecording = "stop_recording"
    case cancelRecording = "cancel_recording"
    case requestInsert = "request_insert"
    case clearState = "clear_state"
}

public struct BridgeCommand: Codable, Equatable {
    public var schema: String
    public var commandID: String
    public var commandSeq: UInt64
    public var kind: BridgeCommandKind
    public var createdAt: Date
    public var keyboardContext: KeyboardContext
    public var languageHint: LanguageHint
    public var style: DictationStyle
    public var clientRequestID: String

    public init(kind: BridgeCommandKind, commandSeq: UInt64, keyboardContext: KeyboardContext, languageHint: LanguageHint, style: DictationStyle, clientRequestID: String) {
        self.schema = "airnote.ios.bridge.command.v1"
        self.commandID = UUID().uuidString
        self.commandSeq = commandSeq
        self.kind = kind
        self.createdAt = Date()
        self.keyboardContext = keyboardContext
        self.languageHint = languageHint
        self.style = style
        self.clientRequestID = clientRequestID
    }

    enum CodingKeys: String, CodingKey {
        case schema
        case commandID = "command_id"
        case commandSeq = "command_seq"
        case kind
        case createdAt = "created_at"
        case keyboardContext = "keyboard_context"
        case languageHint = "language_hint"
        case style
        case clientRequestID = "client_request_id"
    }
}

public enum BridgeResultState: String, Codable {
    case partial
    case final
    case error
    case expired
}

public enum InsertPolicy: String, Codable {
    case insertAtCursor = "insert_at_cursor"
    case replaceSelectedText = "replace_selected_text"
    case copyOnly = "copy_only"
    case saveToHistory = "save_to_history"
}

public struct BridgeResult: Codable, Equatable {
    public var schema: String
    public var resultSeq: UInt64
    public var sessionID: String
    public var clientRequestID: String
    public var requestID: String
    public var state: BridgeResultState
    public var transcript: String
    public var polished: String
    public var language: LanguageHint
    public var style: DictationStyle
    public var latencyMS: Int
    public var createdAt: Date
    public var expiresAt: Date
    public var insertPolicy: InsertPolicy
    public var learningAllowed: Bool

    enum CodingKeys: String, CodingKey {
        case schema
        case resultSeq = "result_seq"
        case sessionID = "session_id"
        case clientRequestID = "client_request_id"
        case requestID = "request_id"
        case state
        case transcript
        case polished
        case language
        case style
        case latencyMS = "latency_ms"
        case createdAt = "created_at"
        case expiresAt = "expires_at"
        case insertPolicy = "insert_policy"
        case learningAllowed = "learning_allowed"
    }
}

public enum TerminalOutcome: String, Codable {
    case inserted
    case copied
    case savedToHistory = "saved_to_history"
    case canceled
    case failed
    case expired
}

public struct BridgeAck: Codable, Equatable {
    public var schema: String
    public var resultSeq: UInt64
    public var sessionID: String
    public var clientRequestID: String
    public var outcome: TerminalOutcome
    public var acknowledgedAt: Date

    public init(resultSeq: UInt64, sessionID: String, clientRequestID: String, outcome: TerminalOutcome) {
        self.schema = "airnote.ios.bridge.ack.v1"
        self.resultSeq = resultSeq
        self.sessionID = sessionID
        self.clientRequestID = clientRequestID
        self.outcome = outcome
        self.acknowledgedAt = Date()
    }

    enum CodingKeys: String, CodingKey {
        case schema
        case resultSeq = "result_seq"
        case sessionID = "session_id"
        case clientRequestID = "client_request_id"
        case outcome
        case acknowledgedAt = "acknowledged_at"
    }
}
