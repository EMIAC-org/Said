import AirNoteShared
import Combine
import Foundation
import UIKit

/// Single source of truth for the AirNote app: authentication, runtime/dictation
/// availability, cross-device settings, history, and personal vocabulary — all
/// against the real server backend. There is no mock/preview path in the app.
@MainActor
final class AppEnvironment: ObservableObject {
    enum Phase: Equatable {
        case launching
        case onboarding
        case ready
    }

    // MARK: Top-level routing

    @Published private(set) var phase: Phase = .launching

    // MARK: Account / auth

    @Published private(set) var account: MobileAccount?
    @Published var authError: String?
    @Published private(set) var isAuthenticating = false

    // MARK: Runtime / dictation availability

    /// True once the server has provider credentials provisioned, so live
    /// dictation will succeed. Until then the UI shows a calm "being set up"
    /// state instead of a hard error.
    @Published private(set) var dictationAvailable = false
    @Published private(set) var runtimeStatusLabel = "Checking workspace…"

    // MARK: Settings (server-backed, cross-device)

    @Published private(set) var outputLanguage = SharedStore.outputLanguage   // "hinglish" | "english"
    @Published private(set) var selectedModel = SharedStore.selectedModel     // "fast" | "smart"
    @Published private(set) var tonePreset = SharedStore.tonePreset           // "work" | "casual" | "email" | "notes"
    @Published private(set) var learningEnabled = true
    @Published private(set) var settingsLoaded = false
    /// Highest settings version applied so far. Guards against an older, slower
    /// PATCH response overwriting a newer one when the user changes settings fast.
    private var settingsVersion = 0

    // MARK: History

    @Published private(set) var history: [RuntimeHistoryItem] = []
    @Published private(set) var historyLoading = false
    @Published private(set) var historyStatus = ""

    // MARK: Vocabulary / personal memory

    @Published private(set) var vocabTermCount = 0
    @Published private(set) var vocabAliasCount = 0
    @Published private(set) var learnedEvents: [RuntimeLearningEvent] = []
    @Published private(set) var vocabStatus = ""
    @Published private(set) var vocabLoading = false

    // MARK: Learning review (correct a saved dictation)

    @Published var learningDraftText = ""
    @Published private(set) var learningItem: RuntimeHistoryItem?
    @Published private(set) var learningCandidates: [RuntimeLearningCandidate] = []
    @Published private(set) var learningStatus = "Pick a saved dictation to review an edit"
    @Published private(set) var learningWorking = false

    // MARK: Collaborators

    let permissions = PermissionManager()
    let authTokens: GatewayAuthTokenBox
    let gateway: any MobileGatewayClient

    private let onboardingCompleteKey = "airnote.onboarding.complete"
    private var didBootstrap = false

    init(gateway: (any MobileGatewayClient)? = nil, authTokens: GatewayAuthTokenBox = GatewayAuthTokenBox()) {
        self.authTokens = authTokens
        self.gateway = gateway ?? GatewayEnvironment.makeClient(authTokenProvider: { authTokens.accessToken })
        self.account = authTokens.account
    }

    // MARK: Lifecycle

    /// Called once at launch. Restores a saved session (validating the token),
    /// then routes to onboarding or the main app.
    func bootstrap() async {
        guard !didBootstrap else { return }
        didBootstrap = true
        permissions.refreshAll()
        if let token = authTokens.accessToken, !token.isEmpty {
            do {
                let response = try await gateway.restoreSession(token: token)
                authTokens.persist(accessToken: response.token, account: response.account)
                account = response.account
                await loadWorkspaceState()
                phase = isOnboardingComplete ? .ready : .onboarding
                return
            } catch let error as GatewayError where error.isUnauthorized {
                signOutLocally()
            } catch {
                // Network hiccup — keep the cached account and proceed; state
                // refreshes when connectivity returns.
                if account != nil {
                    phase = isOnboardingComplete ? .ready : .onboarding
                    return
                }
            }
        }
        phase = .onboarding
    }

    var isOnboardingComplete: Bool {
        account != nil && UserDefaults.standard.bool(forKey: onboardingCompleteKey)
    }

    func completeOnboarding() {
        UserDefaults.standard.set(true, forKey: onboardingCompleteKey)
        phase = .ready
    }

    // MARK: Authentication

    func authenticate(email: String, password: String, signup: Bool) async -> Bool {
        let trimmed = email.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, password.count >= 8 else {
            authError = "Enter a valid email and an 8+ character password."
            return false
        }
        isAuthenticating = true
        authError = nil
        defer { isAuthenticating = false }
        do {
            let response = try await gateway.authenticate(
                MobileAuthRequest(email: trimmed, password: password, signup: signup)
            )
            authTokens.persist(accessToken: response.token, account: response.account)
            account = response.account
            await loadWorkspaceState()
            return true
        } catch let error as GatewayError {
            authError = signup
                ? authSignupMessage(for: error)
                : authLoginMessage(for: error)
            return false
        } catch {
            authError = signup ? "Could not create your account." : "Could not sign you in."
            return false
        }
    }

    private func authLoginMessage(for error: GatewayError) -> String {
        switch error {
        case .unauthorized: return "Email or password is incorrect."
        case .network: return "No internet connection. Check your network and try again."
        default: return error.userMessage
        }
    }

    private func authSignupMessage(for error: GatewayError) -> String {
        switch error {
        case let .server(status, _, message) where status == 409:
            return message ?? "That email is already registered. Try signing in."
        case let .server(_, _, message):
            return message ?? "Could not create your account."
        case .network: return "No internet connection. Check your network and try again."
        default: return error.userMessage
        }
    }

    /// Handles the `airnote://auth/callback?token=…` deep link from Lark sign-in.
    func handleAuthCallback(_ url: URL) async {
        guard
            url.scheme == "airnote",
            url.host == "auth",
            let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
            let token = components.queryItems?.first(where: { $0.name == "token" })?.value?
                .trimmingCharacters(in: .whitespacesAndNewlines),
            !token.isEmpty
        else { return }

        do {
            let response = try await gateway.restoreSession(token: token)
            authTokens.persist(accessToken: response.token, account: response.account)
            account = response.account
            authError = nil
            await loadWorkspaceState()
        } catch {
            authError = "Could not finish Lark sign-in. Try again."
        }
    }

    func signOut() {
        signOutLocally()
        phase = .onboarding
    }

    private func signOutLocally() {
        authTokens.clear()
        account = nil
        dictationAvailable = false
        runtimeStatusLabel = "Signed out"
        settingsLoaded = false
        history = []
        learnedEvents = []
        vocabTermCount = 0
        vocabAliasCount = 0
        cancelLearningReview()
    }

    /// Inspects a thrown error; if the session is invalid, signs out so the user
    /// is routed back to authentication. Returns true if it was an auth failure.
    @discardableResult
    private func handleUnauthorized(_ error: Error) -> Bool {
        if let gatewayError = error as? GatewayError, gatewayError.isUnauthorized {
            signOut()
            authError = "Your session expired. Please sign in again."
            return true
        }
        return false
    }

    // MARK: Workspace bootstrap (after auth)

    private func loadWorkspaceState() async {
        async let status: Void = refreshRuntimeStatus()
        async let settings: Void = loadSettings()
        async let history: Void = refreshHistory()
        _ = await (status, settings, history)
    }

    func refreshRuntimeStatus() async {
        do {
            let status = try await gateway.runtimeStatus()
            dictationAvailable = status.activeCredentialCount > 0
            runtimeStatusLabel = status.activeCredentialCount > 0
                ? (status.serverMemoryReady ? "Personalized" : "Ready")
                : "Setting up dictation"
            vocabTermCount = status.personalVocabCount
            vocabAliasCount = status.personalAliasCount
        } catch {
            if handleUnauthorized(error) { return }
            runtimeStatusLabel = "Offline"
        }
    }

    // MARK: Settings

    func loadSettings() async {
        do {
            let settings = try await gateway.runtimeSettings()
            applySettings(settings)
            settingsLoaded = true
        } catch {
            _ = handleUnauthorized(error)
        }
    }

    private func applySettings(_ settings: RuntimeSettingsResponse) {
        // Ignore a response that's older than what we've already applied.
        guard settings.version >= settingsVersion else { return }
        settingsVersion = settings.version
        outputLanguage = settings.outputLanguage
        selectedModel = settings.selectedModel
        tonePreset = settings.tonePreset
        learningEnabled = settings.learningEnabled
        SharedStore.outputLanguage = settings.outputLanguage
        SharedStore.selectedModel = settings.selectedModel
        SharedStore.tonePreset = settings.tonePreset
    }

    func setOutputLanguage(_ value: String) async { await patch(.init(outputLanguage: value)) { self.outputLanguage = value; SharedStore.outputLanguage = value } }
    func setSelectedModel(_ value: String) async { await patch(.init(selectedModel: value)) { self.selectedModel = value; SharedStore.selectedModel = value } }
    func setTonePreset(_ value: String) async { await patch(.init(tonePreset: value)) { self.tonePreset = value; SharedStore.tonePreset = value } }
    func setLearningEnabled(_ value: Bool) async { await patch(.init(learningEnabled: value)) { self.learningEnabled = value } }

    private func patch(_ patch: RuntimeSettingsPatch, optimistic: () -> Void) async {
        optimistic()
        do {
            let updated = try await gateway.updateSettings(patch)
            applySettings(updated)
        } catch {
            _ = handleUnauthorized(error)
            // Re-sync from server so the UI doesn't drift on failure.
            await loadSettings()
        }
    }

    // MARK: History

    func refreshHistory() async {
        guard account != nil else {
            history = []
            historyStatus = "Sign in to sync your dictations"
            return
        }
        historyLoading = true
        defer { historyLoading = false }
        do {
            let rows = try await gateway.listHistory(limit: 100)
            history = rows
            historyStatus = rows.isEmpty ? "No dictations yet" : ""
        } catch {
            if handleUnauthorized(error) { return }
            historyStatus = "Couldn't load history"
        }
    }

    func deleteHistoryItem(_ item: RuntimeHistoryItem) async {
        let snapshot = history
        history.removeAll { $0.id == item.id }
        if learningItem?.id == item.id { cancelLearningReview() }
        do {
            try await gateway.deleteHistory(id: item.id)
        } catch {
            if handleUnauthorized(error) { return }
            history = snapshot   // rollback
            historyStatus = "Couldn't delete that dictation"
        }
    }

    // MARK: Vocabulary

    func refreshVocabulary() async {
        guard account != nil else { return }
        vocabLoading = true
        defer { vocabLoading = false }
        await refreshRuntimeStatus()
        do {
            learnedEvents = try await gateway.listLearningEvents(limit: 50)
            vocabStatus = ""
        } catch {
            if handleUnauthorized(error) { return }
            vocabStatus = "Couldn't load learned terms"
        }
    }

    @discardableResult
    func addVocabulary(term: String, heardAs: String) async -> Bool {
        let trimmedTerm = term.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedTerm.isEmpty else { return false }
        let alias = heardAs.trimmingCharacters(in: .whitespacesAndNewlines)
        let aliases: [(heard: String, correct: String)] = alias.isEmpty ? [] : [(alias, trimmedTerm)]
        do {
            let result = try await gateway.addVocabulary(terms: [trimmedTerm], aliases: aliases)
            vocabStatus = result.learnedCount > 0 ? "Added \(trimmedTerm)" : "Couldn't add that term"
            await refreshVocabulary()
            return result.learnedCount > 0
        } catch {
            if handleUnauthorized(error) { return false }
            vocabStatus = "Couldn't add that term"
            return false
        }
    }

    // MARK: Learning review

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
        defer { learningWorking = false }
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
            if handleUnauthorized(error) { return }
            learningStatus = "Could not analyze this correction"
        }
    }

    func confirmLearning() async {
        guard let item = learningItem else { return }
        let items = learningCandidates.filter(\.learnable)
        guard !items.isEmpty else {
            learningStatus = "Analyze a correction before confirming"
            return
        }
        learningWorking = true
        defer { learningWorking = false }
        do {
            let result = try await gateway.confirmLearning(recordingID: item.learningRecordingID, items: items)
            learningStatus = result.learnedCount > 0
                ? "Learned \(result.learnedCount) correction\(result.learnedCount == 1 ? "" : "s")"
                : "Nothing new to learn here"
            learningCandidates = []
            await refreshVocabulary()
        } catch {
            if handleUnauthorized(error) { return }
            learningStatus = "Could not confirm learning"
        }
    }

    // MARK: Dictation config

    /// Builds the streaming config from current settings + a fresh run id.
    func dictationConfig(runID: String) -> VoiceStreamConfig {
        VoiceStreamConfig(
            runID: runID,
            selectedModel: selectedModel,
            outputLanguage: outputLanguage,
            safeVocabTerms: [],
            screenContext: nil,
            platform: "ios",
            appVersion: AppInfo.version
        )
    }

    // MARK: Telemetry (best effort, never blocks UX)

    func track(_ type: MobileEventType, latencyMS: Int? = nil) {
        let event = MobileEvent(
            deviceID: AppInfo.deviceID,
            eventType: type,
            redactedContext: RedactedContext(networkType: nil, latencyMS: latencyMS)
        )
        Task { [gateway] in try? await gateway.sendEvent(event) }
    }
}

private extension RuntimeHistoryItem {
    var learningRecordingID: String { clientRunID ?? runID ?? id }

    var learningAIOutput: String {
        let final = finalText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !final.isEmpty { return final }
        let polished = polishedOutput?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !polished.isEmpty { return polished }
        return displayText
    }
}

/// Small app metadata helpers.
enum AppInfo {
    static var version: String {
        let short = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
        return short
    }

    static var build: String {
        Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
    }

    static var deviceID: String {
        UIDevice.current.identifierForVendor?.uuidString ?? "ios-\(UUID().uuidString)"
    }
}
