import AirNoteShared
import Combine
import Foundation

@MainActor
final class AppEnvironment: ObservableObject {
    @Published var setupState: SetupState = .notStarted
    @Published var sessionState: SessionState = .idle
    @Published var languageHint: LanguageHint = .hinglish
    @Published var style: DictationStyle = .work
    @Published var lastStatusMessage: String = "AirNote is ready"
    @Published private(set) var account: MobileAccount?
    @Published private(set) var runtimeStatus: String = BuildConfig.useMockGateway ? "mock_pipeline" : "unknown"

    let dictationStore = DictationStore()
    let eventQueue = EventQueue()
    let authTokens: GatewayAuthTokenBox
    let gateway: any MobileGatewayClient

    init(gateway: (any MobileGatewayClient)? = nil, authTokens: GatewayAuthTokenBox = GatewayAuthTokenBox()) {
        self.authTokens = authTokens
        self.gateway = gateway ?? GatewayEnvironment.makeClient(authTokenProvider: { authTokens.accessToken })
    }

    func markSetupReady() {
        setupState = .ready
        lastStatusMessage = "Keyboard, mic, and Gateway are ready"
    }

    func authenticate(email: String, password: String, signup: Bool) async {
        do {
            let response = try await gateway.authenticate(MobileAuthRequest(email: email, password: password, signup: signup))
            authTokens.accessToken = response.token
            authTokens.refreshToken = response.refreshToken
            account = response.account
            setupState = .accountReady
            lastStatusMessage = "Account and Gateway are ready"
            runtimeStatus = response.policy.streamingEnabled ? "streaming_ready" : "batch_ready"
        } catch {
            setupState = .blocked("Could not sign in. Check your email, password, or gateway.")
        }
    }

    func refreshRuntimeConfig() async {
        do {
            let config = try await gateway.runtimeConfig()
            runtimeStatus = config.runtime.status
            lastStatusMessage = config.runtime.streamingEnabled ? "Gateway streaming is ready" : "Gateway batch fallback is ready"
        } catch {
            runtimeStatus = "unreachable"
        }
    }
}

enum SetupState: Equatable {
    case notStarted
    case accountReady
    case privacyAccepted
    case micReady
    case keyboardReady
    case fullAccessReady
    case ready
    case blocked(String)
}
