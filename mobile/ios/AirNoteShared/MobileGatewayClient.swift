import Foundation

public final class GatewayAuthTokenBox {
    public var accessToken: String?
    public var refreshToken: String?

    public init(accessToken: String? = nil, refreshToken: String? = nil) {
        self.accessToken = accessToken
        self.refreshToken = refreshToken
    }
}

public struct MobileAuthRequest: Codable, Equatable {
    public var email: String
    public var password: String
    public var signup: Bool

    public init(email: String, password: String, signup: Bool) {
        self.email = email
        self.password = password
        self.signup = signup
    }
}

public struct MobileAuthResponse: Codable, Equatable {
    public var token: String
    public var refreshToken: String
    public var account: MobileAccount
    public var policy: MobilePolicy

    enum CodingKeys: String, CodingKey {
        case token
        case refreshToken = "refresh_token"
        case account
        case policy
    }
}

public struct MobileAccount: Codable, Equatable {
    public var id: String
    public var email: String
    public var licenseTier: String

    public init(id: String, email: String, licenseTier: String) {
        self.id = id
        self.email = email
        self.licenseTier = licenseTier
    }

    enum CodingKeys: String, CodingKey {
        case id
        case email
        case licenseTier = "license_tier"
    }
}

public struct MobilePolicy: Codable, Equatable {
    public var mobileEnabled: Bool
    public var maxRecordingSeconds: Int
    public var streamingEnabled: Bool
    public var audioRetentionSeconds: Int
    public var rawTextRetention: String
    public var learningMode: String
    public var allowTranscriptHistory: Bool

    enum CodingKeys: String, CodingKey {
        case mobileEnabled = "mobile_enabled"
        case maxRecordingSeconds = "max_recording_seconds"
        case streamingEnabled = "streaming_enabled"
        case audioRetentionSeconds = "audio_retention_seconds"
        case rawTextRetention = "raw_text_retention"
        case learningMode = "learning_mode"
        case allowTranscriptHistory = "allow_transcript_history"
    }
}

public struct MobileBootstrap: Codable, Equatable {
    public var schema: String
    public var gatewayRegion: String
    public var minSupportedIOSVersion: String
    public var minSupportedAppVersion: String
    public var features: [String: Bool]
    public var limits: MobileLimits

    enum CodingKeys: String, CodingKey {
        case schema
        case gatewayRegion = "gateway_region"
        case minSupportedIOSVersion = "min_supported_ios_version"
        case minSupportedAppVersion = "min_supported_app_version"
        case features
        case limits
    }
}

public struct MobileLimits: Codable, Equatable {
    public var maxRecordingSeconds: Int
    public var maxAudioBytes: Int

    enum CodingKeys: String, CodingKey {
        case maxRecordingSeconds = "max_recording_seconds"
        case maxAudioBytes = "max_audio_bytes"
    }
}

public struct RuntimeConfigResponse: Codable, Equatable {
    public var schema: String
    public var runtime: RuntimeConfig
    public var account: MobileAccount
    public var currentVocabHash: String

    enum CodingKeys: String, CodingKey {
        case schema
        case runtime
        case account
        case currentVocabHash = "current_vocab_hash"
    }
}

public struct RuntimeConfig: Codable, Equatable {
    public var mode: String
    public var sessionPath: String
    public var voiceWSPath: String
    public var batchPath: String
    public var eventPath: String
    public var vocabSnapshotPath: String
    public var maxRecordingSeconds: Int
    public var streamingEnabled: Bool
    public var batchFallbackEnabled: Bool
    public var rawAudioRetention: String
    public var rawTextRetention: String
    public var learningMode: String
    public var status: String

    enum CodingKeys: String, CodingKey {
        case mode
        case sessionPath = "session_path"
        case voiceWSPath = "voice_ws_path"
        case batchPath = "batch_path"
        case eventPath = "event_path"
        case vocabSnapshotPath = "vocab_snapshot_path"
        case maxRecordingSeconds = "max_recording_seconds"
        case streamingEnabled = "streaming_enabled"
        case batchFallbackEnabled = "batch_fallback_enabled"
        case rawAudioRetention = "raw_audio_retention"
        case rawTextRetention = "raw_text_retention"
        case learningMode = "learning_mode"
        case status
    }
}

public struct MobileSessionRequest: Codable, Equatable {
    public var clientRequestID: String
    public var deviceID: String
    public var platform: String
    public var surface: MobileSurface
    public var languageHint: LanguageHint
    public var style: DictationStyle
    public var keyboardContext: KeyboardContext
    public var vocabSnapshotHash: String?

    public init(clientRequestID: String, deviceID: String, languageHint: LanguageHint, style: DictationStyle, keyboardContext: KeyboardContext, surface: MobileSurface = .iosKeyboard, vocabSnapshotHash: String? = nil) {
        self.clientRequestID = clientRequestID
        self.deviceID = deviceID
        self.platform = "ios"
        self.surface = surface
        self.languageHint = languageHint
        self.style = style
        self.keyboardContext = keyboardContext
        self.vocabSnapshotHash = vocabSnapshotHash
    }

    enum CodingKeys: String, CodingKey {
        case clientRequestID = "client_request_id"
        case deviceID = "device_id"
        case platform
        case surface
        case languageHint = "language_hint"
        case style
        case keyboardContext = "keyboard_context"
        case vocabSnapshotHash = "vocab_snapshot_hash"
    }
}

public struct MobileSessionResponse: Codable, Equatable {
    public var sessionID: String
    public var sessionToken: String
    public var expiresAt: Date
    public var streamingEnabled: Bool
    public var currentVocabHash: String
    public var voiceWSURL: String?
    public var batchURL: String?
    public var maxRecordingSeconds: Int?

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case sessionToken = "session_token"
        case expiresAt = "expires_at"
        case streamingEnabled = "streaming_enabled"
        case currentVocabHash = "current_vocab_hash"
        case voiceWSURL = "voice_ws_url"
        case batchURL = "batch_url"
        case maxRecordingSeconds = "max_recording_seconds"
    }
}

public struct MobileDictationResponse: Codable, Equatable {
    public var requestID: String
    public var sessionID: String?
    public var transcript: String
    public var polished: String
    public var language: LanguageHint
    public var style: DictationStyle
    public var latencyMS: Int
    public var mock: Bool

    enum CodingKeys: String, CodingKey {
        case requestID = "request_id"
        case sessionID = "session_id"
        case transcript
        case polished
        case language
        case style
        case latencyMS = "latency_ms"
        case mock
    }
}

public protocol MobileGatewayClient {
    func bootstrap() async throws -> MobileBootstrap
    func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse
    func refresh(refreshToken: String) async throws -> String
    func runtimeConfig() async throws -> RuntimeConfigResponse
    func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse
    func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse
    func sendEvent(_ event: MobileEvent) async throws
}

public typealias GatewayAuthTokenProvider = () -> String?

public enum GatewayEnvironment {
    public static func makeClient(authTokenProvider: GatewayAuthTokenProvider? = nil) -> any MobileGatewayClient {
        if BuildConfig.useMockGateway {
            return MockMobileGatewayClient()
        }
        return HTTPMobileGatewayClient(baseURL: BuildConfig.gatewayBaseURL, authTokenProvider: authTokenProvider)
    }
}

public struct MockMobileGatewayClient: MobileGatewayClient {
    public init() {}

    public func bootstrap() async throws -> MobileBootstrap {
        MobileBootstrap(
            schema: "airnote.mobile.bootstrap.v1",
            gatewayRegion: "mock",
            minSupportedIOSVersion: "17.0",
            minSupportedAppVersion: "0.1.0",
            features: [
                "ios_keyboard": true,
                "ios_action_button": true,
                "streaming_voice": true,
                "batch_fallback": true,
                "explicit_learning": true
            ],
            limits: MobileLimits(maxRecordingSeconds: BuildConfig.maxRecordingSeconds, maxAudioBytes: 15_728_640)
        )
    }

    public func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse {
        MobileAuthResponse(
            token: "mock-access-token",
            refreshToken: "mock-refresh-token",
            account: MobileAccount(id: "mock-account", email: request.email, licenseTier: "free"),
            policy: MobilePolicy(
                mobileEnabled: true,
                maxRecordingSeconds: BuildConfig.maxRecordingSeconds,
                streamingEnabled: true,
                audioRetentionSeconds: 0,
                rawTextRetention: "none",
                learningMode: "insert_first_learn_later",
                allowTranscriptHistory: true
            )
        )
    }

    public func refresh(refreshToken: String) async throws -> String {
        "mock-access-token"
    }

    public func runtimeConfig() async throws -> RuntimeConfigResponse {
        RuntimeConfigResponse(
            schema: "airnote.runtime.config.v1",
            runtime: RuntimeConfig(
                mode: "server_first_mobile",
                sessionPath: "/v1/runtime/sessions",
                voiceWSPath: "/v1/runtime/voice",
                batchPath: "/v1/runtime/voice/batch",
                eventPath: "/v1/runtime/events",
                vocabSnapshotPath: "/v1/mobile/vocab/snapshot",
                maxRecordingSeconds: BuildConfig.maxRecordingSeconds,
                streamingEnabled: true,
                batchFallbackEnabled: true,
                rawAudioRetention: "none",
                rawTextRetention: "none",
                learningMode: "insert_first_learn_later",
                status: "mock_pipeline"
            ),
            account: MobileAccount(id: "mock-account", email: "mock@airnote.local", licenseTier: "free"),
            currentVocabHash: "mock-vocab-v1"
        )
    }

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        MobileSessionResponse(
            sessionID: "mock-ios-session",
            sessionToken: "mock-session-token",
            expiresAt: Date().addingTimeInterval(15 * 60),
            streamingEnabled: true,
            currentVocabHash: "mock-vocab-v1",
            voiceWSURL: "/v1/runtime/voice?session_id=mock-ios-session&session_token=mock-session-token",
            batchURL: "/v1/runtime/voice/batch",
            maxRecordingSeconds: BuildConfig.maxRecordingSeconds
        )
    }

    public func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse {
        MobileDictationResponse(
            requestID: RequestId.make(),
            sessionID: sessionID,
            transcript: "kal ka update concise banake rahul ko bhej do",
            polished: "Kal ka update concise bana ke Rahul ko bhej do.",
            language: languageHint == .auto ? .hinglish : languageHint,
            style: style,
            latencyMS: 420,
            mock: true
        )
    }

    public func sendEvent(_ event: MobileEvent) async throws {}
}

public final class HTTPMobileGatewayClient: MobileGatewayClient {
    private let baseURL: URL
    private let session: URLSession
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private let authTokenProvider: GatewayAuthTokenProvider?

    public init(baseURL: URL, session: URLSession = .shared, authTokenProvider: GatewayAuthTokenProvider? = nil) {
        self.baseURL = baseURL
        self.session = session
        self.encoder = JSONEncoder()
        self.decoder = JSONDecoder()
        self.authTokenProvider = authTokenProvider
        self.encoder.dateEncodingStrategy = .iso8601
        self.decoder.dateDecodingStrategy = .iso8601
    }

    public func bootstrap() async throws -> MobileBootstrap {
        let request = URLRequest(url: baseURL.appendingPathComponent("v1/mobile/bootstrap"))
        let (data, response) = try await session.data(for: request)
        try Self.validate(response: response)
        return try decoder.decode(MobileBootstrap.self, from: data)
    }

    public func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/auth/mobile-email"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = try encoder.encode(request)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(MobileAuthResponse.self, from: data)
    }

    public func refresh(refreshToken: String) async throws -> String {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/auth/mobile-refresh"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = try encoder.encode(["refresh_token": refreshToken])

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        let decoded = try decoder.decode(RefreshResponse.self, from: data)
        return decoded.token
    }

    public func runtimeConfig() async throws -> RuntimeConfigResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/config"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(RuntimeConfigResponse.self, from: data)
    }

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/sessions"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(request)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(MobileSessionResponse.self, from: data)
    }

    public func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse {
        let boundary = "AirNoteBoundary-\(UUID().uuidString)"
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/voice/batch"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = Self.multipartBody(
            boundary: boundary,
            audio: audio,
            fields: [
                "session_id": sessionID ?? "",
                "device_id": deviceID,
                "language_hint": languageHint.rawValue,
                "style": style.rawValue
            ]
        )

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(MobileDictationResponse.self, from: data)
    }

    public func sendEvent(_ event: MobileEvent) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/events"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(event)

        let (_, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
    }

    private func authorize(_ request: inout URLRequest) {
        guard
            let token = authTokenProvider?()?.trimmingCharacters(in: .whitespacesAndNewlines),
            !token.isEmpty
        else {
            return
        }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    }

    private struct RefreshResponse: Decodable {
        var token: String
    }

    private static func multipartBody(boundary: String, audio: Data, fields: [String: String]) -> Data {
        var body = Data()
        for (key, value) in fields where !value.isEmpty {
            body.appendString("--\(boundary)\r\n")
            body.appendString("Content-Disposition: form-data; name=\"\(key)\"\r\n\r\n")
            body.appendString("\(value)\r\n")
        }
        body.appendString("--\(boundary)\r\n")
        body.appendString("Content-Disposition: form-data; name=\"audio\"; filename=\"airnote.pcm\"\r\n")
        body.appendString("Content-Type: audio/pcm; codecs=pcm_s16le; rate=16000\r\n\r\n")
        body.append(audio)
        body.appendString("\r\n--\(boundary)--\r\n")
        return body
    }

    private static func validate(response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.badServerResponse)
        }
    }
}

private extension Data {
    mutating func appendString(_ string: String) {
        append(Data(string.utf8))
    }
}
