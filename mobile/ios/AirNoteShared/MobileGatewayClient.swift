import Foundation

public final class GatewayAuthTokenBox {
    private enum Key {
        static let accessToken = "airnote.mobile.access_token"
        static let account = "airnote.mobile.account"
    }

    private let store: SecureStore
    private let encoder = JSONEncoder()

    public var accessToken: String?
    public var account: MobileAccount?

    public init(accessToken: String? = nil, account: MobileAccount? = nil, store: SecureStore = KeychainSecureStore()) {
        self.store = store
        self.accessToken = accessToken
        self.account = account
        if accessToken == nil {
            self.accessToken = Self.readString(Key.accessToken, from: store)
        }
        if account == nil {
            self.account = Self.readJSON(MobileAccount.self, key: Key.account, from: store)
        }
    }

    public func persist(accessToken: String, account: MobileAccount) {
        self.accessToken = accessToken
        self.account = account
        try? store.write(Data(accessToken.utf8), for: Key.accessToken)
        if let data = try? encoder.encode(account) {
            try? store.write(data, for: Key.account)
        }
    }

    public func clear() {
        accessToken = nil
        account = nil
        try? store.delete(Key.accessToken)
        try? store.delete(Key.account)
    }

    public static func savedAccessToken(store: SecureStore = KeychainSecureStore()) -> String? {
        readString(Key.accessToken, from: store)
    }

    private static func readString(_ key: String, from store: SecureStore) -> String? {
        guard let data = try? store.read(key), let value = String(data: data, encoding: .utf8) else {
            return nil
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func readJSON<T: Decodable>(_ type: T.Type, key: String, from store: SecureStore) -> T? {
        guard let data = try? store.read(key) else {
            return nil
        }
        return try? JSONDecoder().decode(T.self, from: data)
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
    public var account: MobileAccount
    public var refreshToken: String?
    public var policy: MobilePolicy?

    enum CodingKeys: String, CodingKey {
        case token
        case account
        case refreshToken = "refresh_token"
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

public struct RuntimeStatusResponse: Codable, Equatable {
    public var credentialEncryptionConfigured: Bool
    public var activeCredentialCount: Int
    public var runtimeSessionCount: Int
    public var learningEventCount: Int
    public var personalReplacementCount: Int
    public var personalVocabCount: Int
    public var personalAliasCount: Int
    public var activeEditPolicyCount: Int
    public var serverMemoryReady: Bool

    public var readinessLabel: String {
        if activeCredentialCount > 0 {
            return serverMemoryReady ? "server_memory_ready" : "server_ready"
        }
        return credentialEncryptionConfigured ? "needs_credentials" : "needs_runtime_key"
    }

    enum CodingKeys: String, CodingKey {
        case credentialEncryptionConfigured = "credential_encryption_configured"
        case activeCredentialCount = "active_credential_count"
        case runtimeSessionCount = "runtime_session_count"
        case learningEventCount = "learning_event_count"
        case personalReplacementCount = "personal_replacement_count"
        case personalVocabCount = "personal_vocab_count"
        case personalAliasCount = "personal_alias_count"
        case activeEditPolicyCount = "active_edit_policy_count"
        case serverMemoryReady = "server_memory_ready"
    }
}

public struct RuntimeSettingsResponse: Codable, Equatable {
    public var selectedModel: String
    public var outputLanguage: String
    public var tonePreset: String
    public var autoPaste: Bool
    public var editCapture: Bool
    public var learningEnabled: Bool
    public var serverRuntimeEnabled: Bool
    public var serverAudioRuntimeEnabled: Bool
    public var messagePolishMode: Bool
    public var version: Int

    enum CodingKeys: String, CodingKey {
        case selectedModel = "selected_model"
        case outputLanguage = "output_language"
        case tonePreset = "tone_preset"
        case autoPaste = "auto_paste"
        case editCapture = "edit_capture"
        case learningEnabled = "learning_enabled"
        case serverRuntimeEnabled = "server_runtime_enabled"
        case serverAudioRuntimeEnabled = "server_audio_runtime_enabled"
        case messagePolishMode = "message_polish_mode"
        case version
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

public struct RuntimeHistoryItem: Codable, Equatable, Identifiable {
    public var id: String
    public var runID: String?
    public var clientRunID: String?
    public var transcript: String
    public var polishedOutput: String?
    public var finalText: String?
    public var source: String
    public var platform: String?
    public var createdAt: Date

    public var displayText: String {
        let final = finalText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !final.isEmpty { return final }
        let polished = polishedOutput?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !polished.isEmpty { return polished }
        return transcript
    }

    public init(
        id: String,
        runID: String? = nil,
        clientRunID: String? = nil,
        transcript: String,
        polishedOutput: String? = nil,
        finalText: String? = nil,
        source: String,
        platform: String? = nil,
        createdAt: Date = Date()
    ) {
        self.id = id
        self.runID = runID
        self.clientRunID = clientRunID
        self.transcript = transcript
        self.polishedOutput = polishedOutput
        self.finalText = finalText
        self.source = source
        self.platform = platform
        self.createdAt = createdAt
    }

    enum CodingKeys: String, CodingKey {
        case id
        case runID = "run_id"
        case clientRunID = "client_run_id"
        case transcript
        case polishedOutput = "polished_output"
        case finalText = "final_text"
        case source
        case platform
        case createdAt = "created_at"
    }
}

public struct RuntimeLearningCandidate: Codable, Equatable, Identifiable {
    public var original: String
    public var corrected: String
    public var termType: String
    public var learnable: Bool
    public var tag: String

    public var id: String {
        "\(original)|\(corrected)|\(termType)|\(tag)"
    }

    public init(
        original: String,
        corrected: String,
        termType: String,
        learnable: Bool = true,
        tag: String = ""
    ) {
        self.original = original
        self.corrected = corrected
        self.termType = termType
        self.learnable = learnable
        self.tag = tag
    }

    enum CodingKeys: String, CodingKey {
        case original
        case corrected
        case termType = "term_type"
        case learnable
        case tag
    }
}

public struct RuntimeLearningAnalysis: Codable, Equatable {
    public var candidates: [RuntimeLearningCandidate]
    public var changed: Bool
    public var source: String

    public init(candidates: [RuntimeLearningCandidate], changed: Bool, source: String) {
        self.candidates = candidates
        self.changed = changed
        self.source = source
    }
}

public struct RuntimeLearningConfirmResult: Codable, Equatable {
    public var learnedCount: Int
    public var blockedCount: Int
    public var learnedTerms: [String]
    public var status: String

    public init(learnedCount: Int, blockedCount: Int, learnedTerms: [String], status: String) {
        self.learnedCount = learnedCount
        self.blockedCount = blockedCount
        self.learnedTerms = learnedTerms
        self.status = status
    }

    enum CodingKeys: String, CodingKey {
        case learnedCount = "learned_count"
        case blockedCount = "blocked_count"
        case learnedTerms = "learned_terms"
        case status
    }
}

public protocol MobileGatewayClient {
    func bootstrap() async throws -> MobileBootstrap
    func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse
    func runtimeStatus() async throws -> RuntimeStatusResponse
    func runtimeSettings() async throws -> RuntimeSettingsResponse
    func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse
    func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse
    func listHistory(limit: Int) async throws -> [RuntimeHistoryItem]
    func deleteHistory(id: String) async throws
    func analyzeEdit(recordingID: String, transcript: String, aiOutput: String, userKept: String) async throws -> RuntimeLearningAnalysis
    func confirmLearning(recordingID: String, items: [RuntimeLearningCandidate]) async throws -> RuntimeLearningConfirmResult
    func sendEvent(_ event: MobileEvent) async throws
}

public typealias GatewayAuthTokenProvider = () -> String?

public enum GatewayEnvironment {
    public static func makeClient(authTokenProvider: GatewayAuthTokenProvider? = nil) -> any MobileGatewayClient {
        if BuildConfig.useMockGateway {
            return MockMobileGatewayClient()
        }
        return HTTPMobileGatewayClient(
            baseURL: BuildConfig.gatewayBaseURL,
            authTokenProvider: authTokenProvider ?? { GatewayAuthTokenBox.savedAccessToken() }
        )
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
            account: MobileAccount(id: "mock-account", email: request.email, licenseTier: "free"),
            refreshToken: nil,
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

    public func runtimeStatus() async throws -> RuntimeStatusResponse {
        RuntimeStatusResponse(
            credentialEncryptionConfigured: true,
            activeCredentialCount: 2,
            runtimeSessionCount: 0,
            learningEventCount: 0,
            personalReplacementCount: 0,
            personalVocabCount: 0,
            personalAliasCount: 0,
            activeEditPolicyCount: 0,
            serverMemoryReady: false
        )
    }

    public func runtimeSettings() async throws -> RuntimeSettingsResponse {
        RuntimeSettingsResponse(
            selectedModel: "fast",
            outputLanguage: "hinglish",
            tonePreset: "work",
            autoPaste: true,
            editCapture: true,
            learningEnabled: true,
            serverRuntimeEnabled: true,
            serverAudioRuntimeEnabled: true,
            messagePolishMode: true,
            version: 1
        )
    }

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        MobileSessionResponse(
            sessionID: "mock-ios-session",
            sessionToken: "mock-session-token",
            expiresAt: Date().addingTimeInterval(15 * 60),
            streamingEnabled: true,
            currentVocabHash: "mock-vocab-v1",
            voiceWSURL: "/v1/runtime/voice/ws?token=mock-access-token",
            batchURL: "/v1/runtime/voice/wav",
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

    public func listHistory(limit: Int) async throws -> [RuntimeHistoryItem] {
        [
            RuntimeHistoryItem(
                id: "mock-history-1",
                runID: "mock-run-1",
                clientRunID: "mock-client-1",
                transcript: "kal ka update concise banake rahul ko bhej do",
                polishedOutput: "Kal ka update concise bana ke Rahul ko bhej do.",
                finalText: "Kal ka update concise bana ke Rahul ko bhej do.",
                source: "server_wav",
                platform: "ios",
                createdAt: Date()
            )
        ].prefix(max(1, min(limit, 200))).map { $0 }
    }

    public func deleteHistory(id: String) async throws {}

    public func analyzeEdit(recordingID: String, transcript: String, aiOutput: String, userKept: String) async throws -> RuntimeLearningAnalysis {
        let trimmedKept = userKept.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedOutput = aiOutput.trimmingCharacters(in: .whitespacesAndNewlines)
        let changed = trimmedKept != trimmedOutput
        let candidate = RuntimeLearningCandidate(
            original: String((trimmedOutput.isEmpty ? transcript : trimmedOutput).prefix(24)),
            corrected: String(trimmedKept.prefix(32)),
            termType: "proper_noun",
            learnable: changed && !trimmedKept.isEmpty,
            tag: "mock_mobile_edit"
        )
        return RuntimeLearningAnalysis(
            candidates: candidate.learnable ? [candidate] : [],
            changed: changed,
            source: "mock_mobile_learning"
        )
    }

    public func confirmLearning(recordingID: String, items: [RuntimeLearningCandidate]) async throws -> RuntimeLearningConfirmResult {
        RuntimeLearningConfirmResult(
            learnedCount: items.filter(\.learnable).count,
            blockedCount: 0,
            learnedTerms: items.map(\.corrected),
            status: "accepted"
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
        let status = try await runtimeStatus()
        return MobileBootstrap(
            schema: "airnote.mobile.bootstrap.v1",
            gatewayRegion: baseURL.host ?? "airnote",
            minSupportedIOSVersion: "17.0",
            minSupportedAppVersion: "0.1.0",
            features: [
                "ios_keyboard": true,
                "ios_action_button": true,
                "streaming_voice": status.activeCredentialCount > 0,
                "batch_fallback": status.activeCredentialCount > 0,
                "explicit_learning": true
            ],
            limits: MobileLimits(maxRecordingSeconds: BuildConfig.maxRecordingSeconds, maxAudioBytes: 15_728_640)
        )
    }

    public func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse {
        let endpoint = request.signup ? "v1/auth/signup" : "v1/auth/login"
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent(endpoint))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = try encoder.encode(AuthBody(email: request.email, password: request.password))

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(MobileAuthResponse.self, from: data)
    }

    public func runtimeStatus() async throws -> RuntimeStatusResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/status"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(RuntimeStatusResponse.self, from: data)
    }

    public func runtimeSettings() async throws -> RuntimeSettingsResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/settings"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(RuntimeSettingsResponse.self, from: data)
    }

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        guard let token = authTokenProvider?()?.trimmingCharacters(in: .whitespacesAndNewlines), !token.isEmpty else {
            throw URLError(.userAuthenticationRequired)
        }
        return MobileSessionResponse(
            sessionID: request.clientRequestID,
            sessionToken: token,
            expiresAt: Date().addingTimeInterval(15 * 60),
            streamingEnabled: true,
            currentVocabHash: request.vocabSnapshotHash ?? "server-runtime",
            voiceWSURL: "/v1/runtime/voice/ws?token=\(token)",
            batchURL: "/v1/runtime/voice/wav",
            maxRecordingSeconds: BuildConfig.maxRecordingSeconds
        )
    }

    public func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/voice/wav"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        let body = RuntimeVoiceWavBody(
            wavB64: audio.base64EncodedString(),
            outputLanguage: languageHint == .en ? "english" : "hinglish",
            selectedModel: style == .direct ? "fast" : "smart",
            safeVocabTerms: [],
            clientRunID: sessionID,
            deviceID: deviceID,
            platform: "ios",
            appVersion: "0.1.0"
        )
        urlRequest.httpBody = try encoder.encode(body)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        let decoded = try decoder.decode(RuntimeVoiceWavResponse.self, from: data)
        return MobileDictationResponse(
            requestID: decoded.runID,
            sessionID: sessionID,
            transcript: decoded.transcript,
            polished: decoded.output,
            language: languageHint == .auto ? .hinglish : languageHint,
            style: style,
            latencyMS: decoded.latencyMS.total,
            mock: false
        )
    }

    public func sendEvent(_ event: MobileEvent) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/client-events"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(event)

        let (_, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
    }

    public func listHistory(limit: Int) async throws -> [RuntimeHistoryItem] {
        let clamped = max(1, min(limit, 200))
        var components = URLComponents(url: baseURL.appendingPathComponent("v1/runtime/history"), resolvingAgainstBaseURL: false)
        components?.queryItems = [URLQueryItem(name: "limit", value: String(clamped))]
        guard let url = components?.url else {
            throw URLError(.badURL)
        }
        var urlRequest = URLRequest(url: url)
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode([RuntimeHistoryItem].self, from: data)
    }

    public func deleteHistory(id: String) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/history/\(id)"))
        urlRequest.httpMethod = "DELETE"
        authorize(&urlRequest)

        let (_, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
    }

    public func analyzeEdit(recordingID: String, transcript: String, aiOutput: String, userKept: String) async throws -> RuntimeLearningAnalysis {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/learning/analyze-edit"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(RuntimeLearningAnalyzeBody(
            recordingID: recordingID,
            transcript: transcript,
            aiOutput: aiOutput,
            userKept: userKept,
            candidates: []
        ))

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        return try decoder.decode(RuntimeLearningAnalysis.self, from: data)
    }

    public func confirmLearning(recordingID: String, items: [RuntimeLearningCandidate]) async throws -> RuntimeLearningConfirmResult {
        let encodedItems = items
            .filter(\.learnable)
            .map { RuntimeLearningConfirmItem(original: $0.original, corrected: $0.corrected, termType: $0.termType) }
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/learning/confirm-batch"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(RuntimeLearningConfirmBody(recordingID: recordingID, items: encodedItems))

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(response: response)
        let decoded = try decoder.decode(RuntimeLearningConfirmResponse.self, from: data)
        return RuntimeLearningConfirmResult(
            learnedCount: decoded.learnedCount,
            blockedCount: decoded.blockedCount,
            learnedTerms: decoded.learnedTerms,
            status: decoded.serverJudgment?.status ?? "unknown"
        )
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

    private struct AuthBody: Encodable {
        var email: String
        var password: String
    }

    private struct RuntimeLearningAnalyzeBody: Encodable {
        var recordingID: String
        var transcript: String
        var aiOutput: String
        var userKept: String
        var candidates: [RuntimeLearningCandidate]

        enum CodingKeys: String, CodingKey {
            case recordingID = "recording_id"
            case transcript
            case aiOutput = "ai_output"
            case userKept = "user_kept"
            case candidates
        }
    }

    private struct RuntimeLearningConfirmBody: Encodable {
        var recordingID: String
        var items: [RuntimeLearningConfirmItem]

        enum CodingKeys: String, CodingKey {
            case recordingID = "recording_id"
            case items
        }
    }

    private struct RuntimeLearningConfirmItem: Encodable {
        var original: String
        var corrected: String
        var termType: String

        enum CodingKeys: String, CodingKey {
            case original
            case corrected
            case termType = "term_type"
        }
    }

    private struct RuntimeLearningConfirmResponse: Decodable {
        var learnedCount: Int
        var blockedCount: Int
        var learnedTerms: [String]
        var serverJudgment: RuntimeLearningServerJudgment?

        enum CodingKeys: String, CodingKey {
            case learnedCount = "learned_count"
            case blockedCount = "blocked_count"
            case learnedTerms = "learned_terms"
            case serverJudgment = "server_judgment"
        }
    }

    private struct RuntimeLearningServerJudgment: Decodable {
        var status: String
    }

    private struct RuntimeVoiceWavBody: Encodable {
        var wavB64: String
        var outputLanguage: String
        var selectedModel: String
        var safeVocabTerms: [String]
        var clientRunID: String?
        var deviceID: String
        var platform: String
        var appVersion: String

        enum CodingKeys: String, CodingKey {
            case wavB64 = "wav_b64"
            case outputLanguage = "output_language"
            case selectedModel = "selected_model"
            case safeVocabTerms = "safe_vocab_terms"
            case clientRunID = "client_run_id"
            case deviceID = "device_id"
            case platform
            case appVersion = "app_version"
        }
    }

    private struct RuntimeVoiceWavResponse: Decodable {
        var runID: String
        var transcript: String
        var output: String
        var modelUsed: String
        var latencyMS: RuntimeVoiceLatency

        enum CodingKeys: String, CodingKey {
            case runID = "run_id"
            case transcript
            case output
            case modelUsed = "model_used"
            case latencyMS = "latency_ms"
        }
    }

    private struct RuntimeVoiceLatency: Decodable {
        var stt: Int
        var polish: Int
        var total: Int
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
