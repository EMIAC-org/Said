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
        // Fall back to the App Group (survives reinstall when the Keychain doesn't).
        if self.accessToken == nil {
            self.accessToken = SharedStore.accessToken
        }
        if self.account == nil,
           let json = SharedStore.accountJSON,
           let data = json.data(using: .utf8) {
            self.account = try? JSONDecoder().decode(MobileAccount.self, from: data)
        }
    }

    public func persist(accessToken: String, account: MobileAccount) {
        self.accessToken = accessToken
        self.account = account
        try? store.write(Data(accessToken.utf8), for: Key.accessToken)
        if let data = try? encoder.encode(account) {
            try? store.write(data, for: Key.account)
        }
        // Mirror into the App Group so the keyboard extension can stream directly,
        // and so the session survives a reinstall when the Keychain is dropped.
        SharedStore.accessToken = accessToken
        SharedStore.accountEmail = account.email
        SharedStore.accountJSON = (try? encoder.encode(account)).flatMap { String(data: $0, encoding: .utf8) }
    }

    public func clear() {
        accessToken = nil
        account = nil
        try? store.delete(Key.accessToken)
        try? store.delete(Key.account)
        SharedStore.clearAuth()
    }

    public static func savedAccessToken(store: SecureStore = KeychainSecureStore()) -> String? {
        readString(Key.accessToken, from: store) ?? SharedStore.accessToken
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

    public init(
        sessionID: String,
        sessionToken: String,
        expiresAt: Date,
        streamingEnabled: Bool,
        currentVocabHash: String,
        voiceWSURL: String? = nil,
        batchURL: String? = nil,
        maxRecordingSeconds: Int? = nil
    ) {
        self.sessionID = sessionID
        self.sessionToken = sessionToken
        self.expiresAt = expiresAt
        self.streamingEnabled = streamingEnabled
        self.currentVocabHash = currentVocabHash
        self.voiceWSURL = voiceWSURL
        self.batchURL = batchURL
        self.maxRecordingSeconds = maxRecordingSeconds
    }

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
    // Server-nullable: the runtime may persist a record without a raw transcript.
    public var transcript: String?
    public var polishedOutput: String?
    public var finalText: String?
    public var source: String
    public var platform: String?
    public var createdAt: Date

    /// Raw transcript text, never nil for callers.
    public var transcriptText: String { transcript ?? "" }

    public var displayText: String {
        // Final/polished output is guaranteed Roman Hinglish; the raw transcript
        // (transcriptText) is left untouched and shown separately as "Heard".
        let final = finalText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !final.isEmpty { return HinglishScript.enforceRomanHinglish(final) }
        let polished = polishedOutput?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !polished.isEmpty { return HinglishScript.enforceRomanHinglish(polished) }
        return HinglishScript.enforceRomanHinglish(transcriptText)
    }

    public init(
        id: String,
        runID: String? = nil,
        clientRunID: String? = nil,
        transcript: String? = nil,
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

/// Partial update for runtime settings. Only non-nil fields are sent (PATCH semantics).
public struct RuntimeSettingsPatch: Codable, Equatable {
    public var selectedModel: String?
    public var outputLanguage: String?
    public var tonePreset: String?
    public var autoPaste: Bool?
    public var editCapture: Bool?
    public var learningEnabled: Bool?
    public var messagePolishMode: Bool?

    public init(
        selectedModel: String? = nil,
        outputLanguage: String? = nil,
        tonePreset: String? = nil,
        autoPaste: Bool? = nil,
        editCapture: Bool? = nil,
        learningEnabled: Bool? = nil,
        messagePolishMode: Bool? = nil
    ) {
        self.selectedModel = selectedModel
        self.outputLanguage = outputLanguage
        self.tonePreset = tonePreset
        self.autoPaste = autoPaste
        self.editCapture = editCapture
        self.learningEnabled = learningEnabled
        self.messagePolishMode = messagePolishMode
    }

    enum CodingKeys: String, CodingKey {
        case selectedModel = "selected_model"
        case outputLanguage = "output_language"
        case tonePreset = "tone_preset"
        case autoPaste = "auto_paste"
        case editCapture = "edit_capture"
        case learningEnabled = "learning_enabled"
        case messagePolishMode = "message_polish_mode"
    }
}

/// A learned-memory event surfaced on the Vocabulary screen (from GET /v1/runtime/learning-events).
public struct RuntimeLearningEvent: Codable, Equatable, Identifiable {
    public var id: String
    public var eventType: String
    public var classification: String?
    public var createdAt: Date
    /// Terms learned by this event, extracted from the payload where present.
    public var learnedTerms: [String]

    public init(id: String, eventType: String, classification: String?, createdAt: Date, learnedTerms: [String]) {
        self.id = id
        self.eventType = eventType
        self.classification = classification
        self.createdAt = createdAt
        self.learnedTerms = learnedTerms
    }
}

/// A stored provider credential (BYOK) — the server returns only metadata and
/// the last 4 chars, never the secret.
public struct RuntimeCredential: Codable, Equatable, Identifiable {
    public var id: String
    public var provider: String
    public var scope: String
    public var displayName: String
    public var secretLast4: String
    public var status: String

    public init(id: String, provider: String, scope: String, displayName: String, secretLast4: String, status: String) {
        self.id = id
        self.provider = provider
        self.scope = scope
        self.displayName = displayName
        self.secretLast4 = secretLast4
        self.status = status
    }

    enum CodingKeys: String, CodingKey {
        case id
        case provider
        case scope
        case displayName = "display_name"
        case secretLast4 = "secret_last4"
        case status
    }
}

public protocol MobileGatewayClient {
    func bootstrap() async throws -> MobileBootstrap
    func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse
    func restoreSession(token: String) async throws -> MobileAuthResponse
    func runtimeStatus() async throws -> RuntimeStatusResponse
    func runtimeSettings() async throws -> RuntimeSettingsResponse
    func updateSettings(_ patch: RuntimeSettingsPatch) async throws -> RuntimeSettingsResponse
    func listCredentials() async throws -> [RuntimeCredential]
    func saveCredential(provider: String, secret: String) async throws -> RuntimeCredential
    func deleteCredential(id: String) async throws
    func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse
    func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse
    func listHistory(limit: Int) async throws -> [RuntimeHistoryItem]
    func deleteHistory(id: String) async throws
    func syncHistory(clientRunID: String, transcript: String, polished: String, source: String) async throws
    func listLearningEvents(limit: Int) async throws -> [RuntimeLearningEvent]
    func addVocabulary(terms: [String], aliases: [(heard: String, correct: String)]) async throws -> RuntimeLearningConfirmResult
    func analyzeEdit(recordingID: String, transcript: String, aiOutput: String, userKept: String) async throws -> RuntimeLearningAnalysis
    func confirmLearning(recordingID: String, items: [RuntimeLearningCandidate]) async throws -> RuntimeLearningConfirmResult
    func sendEvent(_ event: MobileEvent) async throws
    func listOrgs() async throws -> OrgsResponse
    func activateOrg(id: String) async throws -> OrgActivateResponse
    func deactivateOrg() async throws -> OrgActivateResponse
    func listMeetings(status: String?) async throws -> [Meeting]
    func meetingDetail(id: String) async throws -> MeetingDetail
    func createMeeting(title: String, agenda: String?, participantIDs: [String], durationMinutes: Int?) async throws -> Meeting
    func startMeeting(id: String) async throws
    func endMeeting(id: String) async throws
    func meetingGuestLink(id: String) async throws -> GuestLinkResponse
    func listOrgMembers(orgID: String) async throws -> [OrgMember]
    func divoListThreads() async throws -> [DivoThreadSummary]
    func divoThread(id: String) async throws -> DivoThread
    func divoChat(message: String, threadID: String?) async throws -> DivoChatResult
}

public typealias GatewayAuthTokenProvider = () -> String?

public enum GatewayEnvironment {
    /// Always returns the live HTTP gateway client. The app talks only to the
    /// real server backend — there is no mock path in shipping builds.
    public static func makeClient(authTokenProvider: GatewayAuthTokenProvider? = nil) -> any MobileGatewayClient {
        HTTPMobileGatewayClient(
            baseURL: BuildConfig.gatewayBaseURL,
            authTokenProvider: authTokenProvider ?? { GatewayAuthTokenBox.savedAccessToken() ?? SharedStore.accessToken }
        )
    }
}

#if DEBUG
/// SwiftUI `#Preview`-only client. Returns small, neutral placeholder values so
/// canvas previews render without a network. Never used in a running app build.
public struct PreviewMobileGatewayClient: MobileGatewayClient {
    public init() {}

    public func bootstrap() async throws -> MobileBootstrap {
        MobileBootstrap(
            schema: "airnote.mobile.bootstrap.v1",
            gatewayRegion: "preview",
            minSupportedIOSVersion: "17.0",
            minSupportedAppVersion: "0.1.0",
            features: ["ios_keyboard": true, "streaming_voice": true],
            limits: MobileLimits(maxRecordingSeconds: BuildConfig.maxRecordingSeconds, maxAudioBytes: 15_728_640)
        )
    }

    public func authenticate(_ request: MobileAuthRequest) async throws -> MobileAuthResponse {
        MobileAuthResponse(token: "preview", account: MobileAccount(id: "preview", email: request.email, licenseTier: "free"), refreshToken: nil, policy: nil)
    }

    public func restoreSession(token: String) async throws -> MobileAuthResponse {
        MobileAuthResponse(token: token, account: MobileAccount(id: "preview", email: "you@example.com", licenseTier: "free"), refreshToken: nil, policy: nil)
    }

    public func runtimeStatus() async throws -> RuntimeStatusResponse {
        RuntimeStatusResponse(credentialEncryptionConfigured: true, activeCredentialCount: 1, runtimeSessionCount: 4, learningEventCount: 2, personalReplacementCount: 1, personalVocabCount: 3, personalAliasCount: 1, activeEditPolicyCount: 0, serverMemoryReady: true)
    }

    public func runtimeSettings() async throws -> RuntimeSettingsResponse {
        RuntimeSettingsResponse(selectedModel: "fast", outputLanguage: "hinglish", tonePreset: "work", autoPaste: true, editCapture: true, learningEnabled: true, serverRuntimeEnabled: true, serverAudioRuntimeEnabled: true, messagePolishMode: true, version: 1)
    }

    public func updateSettings(_ patch: RuntimeSettingsPatch) async throws -> RuntimeSettingsResponse {
        try await runtimeSettings()
    }

    public func listCredentials() async throws -> [RuntimeCredential] { [] }
    public func saveCredential(provider: String, secret: String) async throws -> RuntimeCredential {
        RuntimeCredential(id: "preview", provider: provider, scope: "user", displayName: provider.capitalized, secretLast4: "1234", status: "active")
    }
    public func deleteCredential(id: String) async throws {}

    public func createSession(_ request: MobileSessionRequest) async throws -> MobileSessionResponse {
        MobileSessionResponse(sessionID: request.clientRequestID, sessionToken: "preview", expiresAt: Date().addingTimeInterval(1800), streamingEnabled: true, currentVocabHash: "preview", voiceWSURL: nil, batchURL: nil, maxRecordingSeconds: BuildConfig.maxRecordingSeconds)
    }

    public func dictateBatch(audio: Data, sessionID: String?, deviceID: String, languageHint: LanguageHint, style: DictationStyle) async throws -> MobileDictationResponse {
        MobileDictationResponse(requestID: RequestId.make(), sessionID: sessionID, transcript: "", polished: "", language: languageHint, style: style, latencyMS: 0, mock: false)
    }

    public func listHistory(limit: Int) async throws -> [RuntimeHistoryItem] { [] }
    public func deleteHistory(id: String) async throws {}
    public func syncHistory(clientRunID: String, transcript: String, polished: String, source: String) async throws {}
    public func listLearningEvents(limit: Int) async throws -> [RuntimeLearningEvent] { [] }
    public func addVocabulary(terms: [String], aliases: [(heard: String, correct: String)]) async throws -> RuntimeLearningConfirmResult {
        RuntimeLearningConfirmResult(learnedCount: terms.count, blockedCount: 0, learnedTerms: terms, status: "accepted")
    }
    public func analyzeEdit(recordingID: String, transcript: String, aiOutput: String, userKept: String) async throws -> RuntimeLearningAnalysis {
        RuntimeLearningAnalysis(candidates: [], changed: false, source: "preview")
    }
    public func confirmLearning(recordingID: String, items: [RuntimeLearningCandidate]) async throws -> RuntimeLearningConfirmResult {
        RuntimeLearningConfirmResult(learnedCount: items.count, blockedCount: 0, learnedTerms: items.map(\.corrected), status: "accepted")
    }
    public func sendEvent(_ event: MobileEvent) async throws {}
    public func listOrgs() async throws -> OrgsResponse { OrgsResponse(orgs: [], activeOrgID: nil, personalMode: true) }
    public func activateOrg(id: String) async throws -> OrgActivateResponse { OrgActivateResponse(activeOrgID: id, personalMode: false) }
    public func deactivateOrg() async throws -> OrgActivateResponse { OrgActivateResponse(activeOrgID: nil, personalMode: true) }
    public func listMeetings(status: String?) async throws -> [Meeting] { [] }
    public func meetingDetail(id: String) async throws -> MeetingDetail {
        MeetingDetail(meeting: Meeting(id: id, title: "Preview", agenda: nil, status: "ended", createdBy: nil, startedAt: nil, endedAt: nil, createdAt: nil, scheduledAt: nil, durationMinutes: 30), participants: [], summary: nil, tasks: [], decisions: [], transcript: [])
    }
    public func createMeeting(title: String, agenda: String?, participantIDs: [String], durationMinutes: Int?) async throws -> Meeting {
        Meeting(id: "preview", title: title, agenda: agenda, status: "scheduled", createdBy: nil, startedAt: nil, endedAt: nil, createdAt: nil, scheduledAt: nil, durationMinutes: durationMinutes)
    }
    public func startMeeting(id: String) async throws {}
    public func endMeeting(id: String) async throws {}
    public func meetingGuestLink(id: String) async throws -> GuestLinkResponse { GuestLinkResponse(token: "preview", inviteURL: nil, guestLink: nil, expiresAt: nil) }
    public func listOrgMembers(orgID: String) async throws -> [OrgMember] { [] }
    public func divoListThreads() async throws -> [DivoThreadSummary] { [] }
    public func divoThread(id: String) async throws -> DivoThread { DivoThread(id: id, title: "Preview", messages: []) }
    public func divoChat(message: String, threadID: String?) async throws -> DivoChatResult { DivoChatResult(content: "Preview answer.", threadID: threadID ?? "preview") }
}
#endif

public final class HTTPMobileGatewayClient: MobileGatewayClient {
    /// Read fresh each request so switching servers (Settings -> Server) takes
    /// effect immediately, without an app relaunch.
    private var baseURL: URL { BuildConfig.gatewayBaseURL }
    private let session: URLSession
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private let authTokenProvider: GatewayAuthTokenProvider?

    public init(baseURL _: URL = BuildConfig.gatewayBaseURL, session: URLSession = .shared, authTokenProvider: GatewayAuthTokenProvider? = nil) {
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
        try Self.validate(data, response: response)
        return try decoder.decode(MobileAuthResponse.self, from: data)
    }

    public func restoreSession(token: String) async throws -> MobileAuthResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/auth/me"))
        urlRequest.httpMethod = "GET"
        urlRequest.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        let decoded = try decoder.decode(MeResponse.self, from: data)
        return MobileAuthResponse(
            token: token,
            account: MobileAccount(
                id: decoded.account.id,
                email: decoded.account.email,
                licenseTier: decoded.license.tier
            ),
            refreshToken: nil,
            policy: nil
        )
    }

    public func runtimeStatus() async throws -> RuntimeStatusResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/status"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(RuntimeStatusResponse.self, from: data)
    }

    public func runtimeSettings() async throws -> RuntimeSettingsResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/settings"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(RuntimeSettingsResponse.self, from: data)
    }

    public func listOrgs() async throws -> OrgsResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/orgs"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(OrgsResponse.self, from: data)
    }

    public func activateOrg(id: String) async throws -> OrgActivateResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/orgs/\(id)/activate"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = Data("{}".utf8)
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(OrgActivateResponse.self, from: data)
    }

    public func deactivateOrg() async throws -> OrgActivateResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/orgs/deactivate"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = Data("{}".utf8)
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(OrgActivateResponse.self, from: data)
    }

    // MARK: Meetings

    public func listMeetings(status: String?) async throws -> [Meeting] {
        let metURL = baseURL.appendingPathComponent("v1/meetings")
        var url = metURL
        if let status, !status.isEmpty, var c = URLComponents(url: metURL, resolvingAgainstBaseURL: false) {
            c.queryItems = [URLQueryItem(name: "status", value: status)]
            url = c.url ?? metURL
        }
        var urlRequest = URLRequest(url: url)
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(MeetingsListResponse.self, from: data).meetings
    }

    public func meetingDetail(id: String) async throws -> MeetingDetail {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/meetings/\(id)"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(MeetingDetail.self, from: data)
    }

    public func createMeeting(title: String, agenda: String?, participantIDs: [String], durationMinutes: Int?) async throws -> Meeting {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/meetings"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = try encoder.encode(CreateMeetingBody(
            title: title, agenda: agenda, participantIds: participantIDs,
            scheduledAt: nil, durationMinutes: durationMinutes
        ))
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(MeetingResponse.self, from: data).meeting
    }

    public func startMeeting(id: String) async throws { try await postMeetingAction(id: id, action: "start") }
    public func endMeeting(id: String) async throws { try await postMeetingAction(id: id, action: "end") }

    private func postMeetingAction(id: String, action: String) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/meetings/\(id)/\(action)"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = Data("{}".utf8)
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
    }

    public func meetingGuestLink(id: String) async throws -> GuestLinkResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/meetings/\(id)/guest-link"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        urlRequest.httpBody = Data("{}".utf8)
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(GuestLinkResponse.self, from: data)
    }

    public func listOrgMembers(orgID: String) async throws -> [OrgMember] {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/orgs/\(orgID)/members"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(OrgMembersResponse.self, from: data).members
    }

    // MARK: Divo (SSE chat; server-gated to approved accounts)

    public func divoListThreads() async throws -> [DivoThreadSummary] {
        let url = baseURL.appendingPathComponent("v1/divo/threads")
        var c = URLComponents(url: url, resolvingAgainstBaseURL: false)
        c?.queryItems = [URLQueryItem(name: "page", value: "1"), URLQueryItem(name: "pageSize", value: "30")]
        var urlRequest = URLRequest(url: c?.url ?? url)
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(DivoThreadsEnvelope.self, from: data).data.threads
    }

    public func divoThread(id: String) async throws -> DivoThread {
        let url = baseURL.appendingPathComponent("v1/divo/threads/\(id)")
        var c = URLComponents(url: url, resolvingAgainstBaseURL: false)
        c?.queryItems = [URLQueryItem(name: "page", value: "1"), URLQueryItem(name: "pageSize", value: "50")]
        var urlRequest = URLRequest(url: c?.url ?? url)
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(DivoThreadEnvelope.self, from: data).data
    }

    public func divoChat(message: String, threadID: String?) async throws -> DivoChatResult {
        var req = URLRequest(url: baseURL.appendingPathComponent("v1/divo/chat"))
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 120
        req.httpBody = try encoder.encode(DivoChatBody(requestId: RequestId.make(), message: message, threadId: threadID, mode: "high"))
        authorize(&req)
        let (bytes, response) = try await session.bytes(for: req)
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw GatewayError.from(status: http.statusCode, code: nil,
                                    message: http.statusCode == 403 ? "Divo is limited to approved accounts, and needs Lark sign-in." : nil)
        }
        var event = ""
        var dataLines: [String] = []
        var content: String?
        var thread = threadID
        for try await line in bytes.lines {
            if line.isEmpty {
                let dataStr = dataLines.joined(separator: "\n")
                if !dataStr.isEmpty {
                    let obj = (try? JSONSerialization.jsonObject(with: Data(dataStr.utf8))) as? [String: Any]
                    switch event {
                    case "error":
                        throw GatewayError.server(status: 0, code: nil, message: (obj?["message"] as? String) ?? "Divo error")
                    case "done":
                        if let msg = obj?["message"] as? [String: Any] {
                            content = msg["content"] as? String
                            thread = (msg["threadId"] as? String) ?? thread
                        }
                    case "meta":
                        thread = (obj?["threadId"] as? String) ?? thread
                    default:
                        break
                    }
                }
                if event == "done" || event == "error" { break }
                event = ""
                dataLines = []
                continue
            }
            if line.hasPrefix(":") { continue }
            if line.hasPrefix("event:") {
                event = String(line.dropFirst("event:".count)).trimmingCharacters(in: .whitespaces)
            } else if line.hasPrefix("data:") {
                dataLines.append(String(line.dropFirst("data:".count)).trimmingCharacters(in: .whitespaces))
            }
        }
        guard let content, !content.isEmpty else { throw GatewayError.invalidResponse }
        return DivoChatResult(content: content, threadID: thread)
    }

    private struct DivoThreadsEnvelope: Decodable {
        let data: ThreadsData
        struct ThreadsData: Decodable { let threads: [DivoThreadSummary] }
    }
    private struct DivoThreadEnvelope: Decodable { let data: DivoThread }
    private struct DivoChatBody: Encodable {
        var requestId: String
        var message: String
        var threadId: String?
        var mode: String
    }

    private struct CreateMeetingBody: Encodable {
        var title: String
        var agenda: String?
        var participantIds: [String]
        var scheduledAt: String?
        var durationMinutes: Int?
        enum CodingKeys: String, CodingKey {
            case title, agenda
            case participantIds = "participant_ids"
            case scheduledAt = "scheduled_at"
            case durationMinutes = "duration_minutes"
        }
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
        try Self.validate(data, response: response)
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

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
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
        try Self.validate(data, response: response)
        return try decoder.decode([RuntimeHistoryItem].self, from: data)
    }

    public func deleteHistory(id: String) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/history/\(id)"))
        urlRequest.httpMethod = "DELETE"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
    }

    /// Persist a finished dictation to history. The WS voice path doesn't write
    /// history server-side, so the client syncs it here (the same endpoint the
    /// desktop uses), making dictations show in History + reviewable for learning.
    public func syncHistory(clientRunID: String, transcript: String, polished: String, source: String = "ios") async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/history/sync"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(HistorySyncBody(items: [
            HistorySyncBody.Item(
                clientRunID: clientRunID,
                source: source,
                platform: "ios",
                transcript: transcript,
                polishedOutput: polished,
                finalText: polished
            )
        ]))
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
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
        try Self.validate(data, response: response)
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
        try Self.validate(data, response: response)
        let decoded = try decoder.decode(RuntimeLearningConfirmResponse.self, from: data)
        return RuntimeLearningConfirmResult(
            learnedCount: decoded.learnedCount,
            blockedCount: decoded.blockedCount,
            learnedTerms: decoded.learnedTerms,
            status: decoded.serverJudgment?.status ?? "unknown"
        )
    }

    public func updateSettings(_ patch: RuntimeSettingsPatch) async throws -> RuntimeSettingsResponse {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/settings"))
        urlRequest.httpMethod = "PATCH"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(patch)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(RuntimeSettingsResponse.self, from: data)
    }

    public func listCredentials() async throws -> [RuntimeCredential] {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/credentials"))
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode([RuntimeCredential].self, from: data)
    }

    public func saveCredential(provider: String, secret: String) async throws -> RuntimeCredential {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/credentials"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(SaveCredentialBody(
            provider: provider.lowercased(),
            secret: secret.trimmingCharacters(in: .whitespacesAndNewlines),
            scope: "user"
        ))
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        return try decoder.decode(RuntimeCredential.self, from: data)
    }

    public func deleteCredential(id: String) async throws {
        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/credentials/\(id)"))
        urlRequest.httpMethod = "DELETE"
        authorize(&urlRequest)
        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
    }

    public func listLearningEvents(limit: Int) async throws -> [RuntimeLearningEvent] {
        let clamped = max(1, min(limit, 200))
        var components = URLComponents(url: baseURL.appendingPathComponent("v1/runtime/learning-events"), resolvingAgainstBaseURL: false)
        components?.queryItems = [URLQueryItem(name: "limit", value: String(clamped))]
        guard let url = components?.url else { throw GatewayError.invalidResponse }
        var urlRequest = URLRequest(url: url)
        urlRequest.httpMethod = "GET"
        authorize(&urlRequest)

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        let rows = try decoder.decode([LearningEventRow].self, from: data)
        return rows.map { $0.toEvent() }
    }

    public func addVocabulary(terms: [String], aliases: [(heard: String, correct: String)]) async throws -> RuntimeLearningConfirmResult {
        let vocabItems = terms
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .map { MemoryVocabItem(term: $0, termType: "proper_noun", weight: 1.0) }
        let aliasItems = aliases
            .map { (heard: $0.heard.trimmingCharacters(in: .whitespacesAndNewlines), correct: $0.correct.trimmingCharacters(in: .whitespacesAndNewlines)) }
            .filter { !$0.heard.isEmpty && !$0.correct.isEmpty }
            .map { MemoryAliasItem(transcriptForm: $0.heard, correctForm: $0.correct, editType: "replace") }

        guard !vocabItems.isEmpty || !aliasItems.isEmpty else {
            return RuntimeLearningConfirmResult(learnedCount: 0, blockedCount: 0, learnedTerms: [], status: "empty")
        }

        var urlRequest = URLRequest(url: baseURL.appendingPathComponent("v1/runtime/memory/sync"))
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&urlRequest)
        urlRequest.httpBody = try encoder.encode(MemorySyncBody(vocabTerms: vocabItems, sttReplacements: aliasItems))

        let (data, response) = try await session.data(for: urlRequest)
        try Self.validate(data, response: response)
        let decoded = try decoder.decode(MemorySyncResponseBody.self, from: data)
        let learned = decoded.acceptedVocab + decoded.acceptedAliases
        let blocked = decoded.blockedVocab + decoded.blockedAliases
        let learnedTerms = vocabItems.map(\.term) + aliasItems.map(\.correctForm)
        return RuntimeLearningConfirmResult(
            learnedCount: learned,
            blockedCount: blocked,
            learnedTerms: learnedTerms,
            status: learned > 0 ? "accepted" : "blocked"
        )
    }

    private func authorize(_ request: inout URLRequest) {
        if let token = authTokenProvider?()?.trimmingCharacters(in: .whitespacesAndNewlines), !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        // Scope org-aware endpoints (meetings, divo, org settings) to the active
        // workspace. Only sent when an org is active — personal mode sends no
        // header, so the personal dictation runtime is unaffected.
        if let org = SharedStore.activeOrgID, !org.isEmpty {
            request.setValue(org, forHTTPHeaderField: "X-AirNote-Org-Id")
        }
    }

    private struct AuthBody: Encodable {
        var email: String
        var password: String
    }

    private struct HistorySyncBody: Encodable {
        let items: [Item]
        struct Item: Encodable {
            var clientRunID: String
            var source: String
            var platform: String
            var transcript: String
            var polishedOutput: String
            var finalText: String
            enum CodingKeys: String, CodingKey {
                case clientRunID = "client_run_id"
                case source, platform, transcript
                case polishedOutput = "polished_output"
                case finalText = "final_text"
            }
        }
    }

    private struct MeResponse: Decodable {
        var account: MeAccount
        var license: MeLicense
    }

    private struct MeAccount: Decodable {
        var id: String
        var email: String
    }

    private struct MeLicense: Decodable {
        var tier: String
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

    private struct SaveCredentialBody: Encodable {
        var provider: String
        var secret: String
        var scope: String
    }

    private struct MemoryVocabItem: Encodable {
        var term: String
        var termType: String
        var weight: Double
        enum CodingKeys: String, CodingKey {
            case term
            case termType = "term_type"
            case weight
        }
    }

    private struct MemoryAliasItem: Encodable {
        var transcriptForm: String
        var correctForm: String
        var editType: String
        enum CodingKeys: String, CodingKey {
            case transcriptForm = "transcript_form"
            case correctForm = "correct_form"
            case editType = "edit_type"
        }
    }

    private struct MemorySyncBody: Encodable {
        var vocabTerms: [MemoryVocabItem]
        var sttReplacements: [MemoryAliasItem]
        enum CodingKeys: String, CodingKey {
            case vocabTerms = "vocab_terms"
            case sttReplacements = "stt_replacements"
        }
    }

    private struct MemorySyncResponseBody: Decodable {
        var acceptedVocab: Int
        var acceptedAliases: Int
        var blockedVocab: Int
        var blockedAliases: Int
        enum CodingKeys: String, CodingKey {
            case acceptedVocab = "accepted_vocab"
            case acceptedAliases = "accepted_aliases"
            case blockedVocab = "blocked_vocab"
            case blockedAliases = "blocked_aliases"
        }
    }

    /// Raw learning-event row. We only surface the learned terms (from the
    /// confirm payload's `memory` block) plus light metadata.
    private struct LearningEventRow: Decodable {
        var id: String
        var eventType: String
        var classification: String?
        var createdAt: Date
        var payloadJson: PayloadBlock?

        enum CodingKeys: String, CodingKey {
            case id
            case eventType = "event_type"
            case classification
            case createdAt = "created_at"
            case payloadJson = "payload_json"
        }

        struct PayloadBlock: Decodable {
            var memory: MemoryBlock?
            struct MemoryBlock: Decodable {
                var acceptedTerms: [TermEntry]?
                var acceptedAliases: [AliasEntry]?
                enum CodingKeys: String, CodingKey {
                    case acceptedTerms = "accepted_terms"
                    case acceptedAliases = "accepted_aliases"
                }
            }
            struct TermEntry: Decodable { var term: String? }
            struct AliasEntry: Decodable {
                var correctForm: String?
                enum CodingKeys: String, CodingKey { case correctForm = "correct_form" }
            }
        }

        func toEvent() -> RuntimeLearningEvent {
            var terms: [String] = []
            if let memory = payloadJson?.memory {
                terms.append(contentsOf: (memory.acceptedTerms ?? []).compactMap { $0.term })
                terms.append(contentsOf: (memory.acceptedAliases ?? []).compactMap { $0.correctForm })
            }
            // De-dupe while preserving order.
            var seen = Set<String>()
            let unique = terms.filter { seen.insert($0.lowercased()).inserted && !$0.isEmpty }
            return RuntimeLearningEvent(
                id: id,
                eventType: eventType,
                classification: classification,
                createdAt: createdAt,
                learnedTerms: unique
            )
        }
    }

    private static func validate(_ data: Data, response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else {
            throw GatewayError.invalidResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            let (code, message) = parseError(data)
            throw GatewayError.from(status: http.statusCode, code: code, message: message)
        }
    }

    private static func parseError(_ data: Data) -> (code: String?, message: String?) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return (nil, nil)
        }
        let code = (object["code"] as? String) ?? (object["error_code"] as? String)
        let message = (object["error"] as? String) ?? (object["message"] as? String)
        return (code, message)
    }
}

private extension Data {
    mutating func appendString(_ string: String) {
        append(Data(string.utf8))
    }
}
