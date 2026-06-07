import Foundation

public struct MobileSessionRequest: Codable, Equatable {
    public var clientRequestID: String
    public var deviceID: String
    public var languageHint: LanguageHint
    public var style: DictationStyle
    public var keyboardContext: KeyboardContext

    public init(clientRequestID: String, deviceID: String, languageHint: LanguageHint, style: DictationStyle, keyboardContext: KeyboardContext) {
        self.clientRequestID = clientRequestID
        self.deviceID = deviceID
        self.languageHint = languageHint
        self.style = style
        self.keyboardContext = keyboardContext
    }

    enum CodingKeys: String, CodingKey {
        case clientRequestID = "client_request_id"
        case deviceID = "device_id"
        case languageHint = "language_hint"
        case style
        case keyboardContext = "keyboard_context"
    }
}

public struct MobileSessionResponse: Codable, Equatable {
    public var sessionID: String
    public var sessionToken: String
    public var expiresAt: Date
    public var streamingEnabled: Bool
    public var currentVocabHash: String

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case sessionToken = "session_token"
        case expiresAt = "expires_at"
        case streamingEnabled = "streaming_enabled"
        case currentVocabHash = "current_vocab_hash"
    }
}

public protocol MobileGatewayClient {
    func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse
    func sendEvent(_ event: MobileEvent) async throws
}

public struct MockMobileGatewayClient: MobileGatewayClient {
    public init() {}

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        MobileSessionResponse(
            sessionID: "mock-ios-session",
            sessionToken: "mock-session-token",
            expiresAt: Date().addingTimeInterval(15 * 60),
            streamingEnabled: true,
            currentVocabHash: "mock-vocab-v1"
        )
    }

    public func sendEvent(_ event: MobileEvent) async throws {}
}
