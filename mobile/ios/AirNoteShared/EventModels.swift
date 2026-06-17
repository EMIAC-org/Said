import Foundation

public enum MobileEventType: String, Codable {
    case setupStarted = "setup_started"
    case setupMicReady = "setup_mic_ready"
    case setupKeyboardEnabled = "setup_keyboard_enabled"
    case setupFullAccessReady = "setup_full_access_ready"
    case sessionCreated = "session_created"
    case sessionReady = "session_ready"
    case sessionStale = "session_stale"
    case audioStarted = "audio_started"
    case audioStopped = "audio_stopped"
    case gatewayWsConnected = "gateway_ws_connected"
    case gatewayWsFailed = "gateway_ws_failed"
    case insertSucceeded = "insert_succeeded"
    case insertFailed = "insert_failed"
    case insertCopied = "insert_copied"
    case correctionLearnSpellingTapped = "correction_learn_spelling_tapped"
}

public struct RedactedContext: Codable, Equatable {
    public var hostAppLabel: String?
    public var fieldHint: String?
    public var networkType: String?
    public var latencyMS: Int?

    public init(hostAppLabel: String? = nil, fieldHint: String? = nil, networkType: String? = nil, latencyMS: Int? = nil) {
        self.hostAppLabel = hostAppLabel
        self.fieldHint = fieldHint
        self.networkType = networkType
        self.latencyMS = latencyMS
    }

    enum CodingKeys: String, CodingKey {
        case hostAppLabel = "host_app_label"
        case fieldHint = "field_hint"
        case networkType = "network_type"
        case latencyMS = "latency_ms"
    }
}

public struct MobileEvent: Codable, Equatable {
    public var eventID: String
    public var schema: String
    public var occurredAt: Date
    public var deviceID: String
    public var sessionID: String?
    public var clientRequestID: String?
    public var build: String
    public var platform: String
    public var surface: String
    public var eventType: MobileEventType
    public var redactedContext: RedactedContext

    public init(deviceID: String, eventType: MobileEventType, redactedContext: RedactedContext, sessionID: String? = nil, clientRequestID: String? = nil) {
        self.eventID = UUID().uuidString
        self.schema = "airnote.mobile.event.v1"
        self.occurredAt = Date()
        self.deviceID = deviceID
        self.sessionID = sessionID
        self.clientRequestID = clientRequestID
        self.build = "0.1.0(0)"
        self.platform = "ios"
        self.surface = "ios_keyboard"
        self.eventType = eventType
        self.redactedContext = redactedContext
    }

    enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case schema
        case occurredAt = "occurred_at"
        case deviceID = "device_id"
        case sessionID = "session_id"
        case clientRequestID = "client_request_id"
        case build
        case platform
        case surface
        case eventType = "event_type"
        case redactedContext = "redacted_context"
    }
}
