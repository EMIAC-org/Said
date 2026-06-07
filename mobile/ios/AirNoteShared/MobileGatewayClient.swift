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

public enum GatewayEnvironment {
    public static func makeClient() -> any MobileGatewayClient {
        if BuildConfig.useMockGateway {
            return MockMobileGatewayClient()
        }
        return HTTPMobileGatewayClient(baseURL: BuildConfig.gatewayBaseURL)
    }
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

public final class HTTPMobileGatewayClient: MobileGatewayClient {
    private let baseURL: URL
    private let session: URLSession
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder

    public init(baseURL: URL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
        self.encoder = JSONEncoder()
        self.decoder = JSONDecoder()
        self.encoder.dateEncodingStrategy = .iso8601
        self.decoder.dateDecodingStrategy = .iso8601
    }

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/mobile/sessions"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = try encoder.encode(request)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(MobileSessionResponse.self, from: data)
    }

    public func sendEvent(_ event: MobileEvent) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/mobile/events"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = try encoder.encode(event)

        let (_, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
    }

    private static func validate(response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.badServerResponse)
        }
    }
}
