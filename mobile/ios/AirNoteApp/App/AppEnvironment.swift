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
    @Published private(set) var runtimeStatus: String = BuildConfig.useMockGateway ? "Preview" : "Unknown"
    @Published private(set) var serverHistory: [RuntimeHistoryItem] = []
    @Published private(set) var historyStatus: String = BuildConfig.useMockGateway ? "Preview history" : "History not loaded"
    @Published var learningDraftText: String = ""
    @Published private(set) var learningItem: RuntimeHistoryItem?
    @Published private(set) var learningCandidates: [RuntimeLearningCandidate] = []
    @Published private(set) var learningStatus: String = "Pick a saved dictation to review an edit"
    @Published private(set) var learningWorking: Bool = false

    let dictationStore = DictationStore()
    let eventQueue = EventQueue()
    let authTokens: GatewayAuthTokenBox
    let gateway: any MobileGatewayClient

    init(gateway: (any MobileGatewayClient)? = nil, authTokens: GatewayAuthTokenBox = GatewayAuthTokenBox()) {
        self.authTokens = authTokens
        self.gateway = gateway ?? GatewayEnvironment.makeClient(authTokenProvider: { authTokens.accessToken })
        self.account = authTokens.account
        if authTokens.account != nil {
            setupState = .accountReady
            lastStatusMessage = "Account restored from this device"
        }
    }

    func markSetupReady() {
        setupState = .ready
        lastStatusMessage = "Keyboard, mic, and Gateway are ready"
    }

    func markMockAccountReady() {
        account = MobileAccount(id: "preview-account", email: "anugra@airnote.preview", licenseTier: "test")
        setupState = .accountReady
        runtimeStatus = "Preview"
        lastStatusMessage = "Account and Gateway are ready"
    }

    func markPrivacyAccepted() {
        setupState = .privacyAccepted
        lastStatusMessage = "Privacy reviewed"
    }

    func markMicReady() {
        setupState = .micReady
        lastStatusMessage = "Microphone check passed"
    }

    func markKeyboardReady(fullAccess: Bool = false) {
        setupState = fullAccess ? .fullAccessReady : .keyboardReady
        lastStatusMessage = fullAccess ? "Keyboard and Full Access are ready" : "Keyboard setup previewed"
    }

    func resetMockSetup() {
        authTokens.clear()
        account = nil
        setupState = .notStarted
        runtimeStatus = BuildConfig.useMockGateway ? "Preview" : "Unknown"
        serverHistory = []
        historyStatus = BuildConfig.useMockGateway ? "Preview history" : "Sign in to sync server history"
        cancelLearningReview()
        lastStatusMessage = "AirNote is ready"
        dictationStore.clear()
    }

    func authenticate(email: String, password: String, signup: Bool) async {
        do {
            let response = try await gateway.authenticate(MobileAuthRequest(email: email, password: password, signup: signup))
            authTokens.persist(accessToken: response.token, account: response.account)
            account = response.account
            setupState = .accountReady
            lastStatusMessage = "Account and Gateway are ready"
            if let policy = response.policy {
                runtimeStatus = policy.streamingEnabled ? "streaming_ready" : "batch_ready"
            } else {
                await refreshRuntimeConfig()
            }
            await refreshHistory()
        } catch {
            setupState = .blocked("Could not sign in. Check your email, password, or gateway.")
        }
    }

    func handleAuthCallback(_ url: URL) async {
        guard
            url.scheme == "airnote",
            url.host == "auth",
            url.path == "/callback",
            let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
            let token = components.queryItems?.first(where: { $0.name == "token" })?.value?.trimmingCharacters(in: .whitespacesAndNewlines),
            !token.isEmpty
        else {
            return
        }

        do {
            let response = try await gateway.restoreSession(token: token)
            authTokens.persist(accessToken: response.token, account: response.account)
            account = response.account
            setupState = .accountReady
            lastStatusMessage = "Lark workspace connected"
            await refreshRuntimeConfig()
            await refreshHistory()
        } catch {
            setupState = .blocked("Could not finish Lark sign-in. Try again from setup.")
        }
    }

    func refreshRuntimeConfig() async {
        do {
            let status = try await gateway.runtimeStatus()
            runtimeStatus = status.readinessLabel
            lastStatusMessage = status.activeCredentialCount > 0 ? "Gateway streaming is ready" : "Gateway needs runtime credentials"
        } catch {
            runtimeStatus = "unreachable"
        }
    }

    func refreshHistory() async {
        if !BuildConfig.useMockGateway && authTokens.accessToken == nil {
            serverHistory = []
            historyStatus = "Sign in to sync server history"
            return
        }

        do {
            let rows = try await gateway.listHistory(limit: 50)
            serverHistory = rows
            historyStatus = rows.isEmpty ? "No server history yet" : "Server history synced"
        } catch {
            historyStatus = "Could not load server history"
        }
    }

    func deleteHistoryItem(_ item: RuntimeHistoryItem) async {
        do {
            try await gateway.deleteHistory(id: item.id)
            serverHistory.removeAll { $0.id == item.id }
            if learningItem?.id == item.id {
                cancelLearningReview()
            }
            historyStatus = "Deleted from server history"
        } catch {
            historyStatus = "Could not delete history item"
        }
    }

    func startLearningReview(_ item: RuntimeHistoryItem) {
        learningItem = item
        learningDraftText = item.displayText
        learningCandidates = []
        learningStatus = "Edit the kept text, then analyze the correction"
        learningWorking = false
    }

    func cancelLearningReview() {
        learningItem = nil
        learningDraftText = ""
        learningCandidates = []
        learningStatus = "Pick a saved dictation to review an edit"
        learningWorking = false
    }

    func analyzeLearningEdit() async {
        guard let item = learningItem else { return }
        let kept = learningDraftText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !kept.isEmpty else {
            learningStatus = "Kept text cannot be empty"
            return
        }

        learningWorking = true
        learningCandidates = []
        do {
            let analysis = try await gateway.analyzeEdit(
                recordingID: item.learningRecordingID,
                transcript: item.transcript,
                aiOutput: item.learningAIOutput,
                userKept: kept
            )
            learningCandidates = analysis.candidates.filter(\.learnable)
            if !analysis.changed {
                learningStatus = "No edit detected"
            } else if learningCandidates.isEmpty {
                learningStatus = "No safe learning candidates found"
            } else {
                learningStatus = "\(learningCandidates.count) learning candidate\(learningCandidates.count == 1 ? "" : "s") ready"
            }
        } catch {
            learningStatus = "Could not analyze this correction"
        }
        learningWorking = false
    }

    func confirmLearning() async {
        guard let item = learningItem else { return }
        let items = learningCandidates.filter(\.learnable)
        guard !items.isEmpty else {
            learningStatus = "Analyze a correction before confirming"
            return
        }

        learningWorking = true
        do {
            let result = try await gateway.confirmLearning(recordingID: item.learningRecordingID, items: items)
            learningStatus = "Learned \(result.learnedCount), blocked \(result.blockedCount). \(result.status)"
            learningCandidates = []
            await refreshHistory()
        } catch {
            learningStatus = "Could not confirm learning"
        }
        learningWorking = false
    }
}

private extension RuntimeHistoryItem {
    var learningRecordingID: String {
        clientRunID ?? runID ?? id
    }

    var learningAIOutput: String {
        let final = finalText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !final.isEmpty { return final }
        let polished = polishedOutput?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !polished.isEmpty { return polished }
        return displayText
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
