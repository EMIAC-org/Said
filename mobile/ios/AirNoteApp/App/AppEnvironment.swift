import AirNoteShared
import Combine
import Foundation
import UIKit

/// Identifies one keyboard→app dictation handoff. A new value each time forces
/// the dictation sheet to re-present fresh.
struct KeyboardHandoff: Identifiable {
    let id = UUID()
}

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
    /// Non-nil while the keyboard handed off a dictation (airnote://dictate). A
    /// fresh token each time forces a clean dictation sheet to re-present.
    @Published var keyboardHandoff: KeyboardHandoff?

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

    // MARK: Provider credentials (BYOK — user's own Deepgram/Groq keys, server-vaulted)

    @Published private(set) var credentials: [RuntimeCredential] = []
    @Published private(set) var credentialStatus = ""
    @Published private(set) var credentialWorking = false

    /// Providers the runtime needs for end-to-end dictation.
    static let requiredProviders = ["deepgram", "groq"]

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
        guard account != nil else { return false }
        return SharedStore.onboardingComplete || UserDefaults.standard.bool(forKey: onboardingCompleteKey)
    }

    func completeOnboarding() {
        SharedStore.onboardingComplete = true
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

    /// Routes incoming `airnote://` deep links. `airnote://dictate` is the
    /// keyboard handoff (record here, then the keyboard inserts on return).
    func handleDeepLink(_ url: URL) async {
        guard url.scheme == "airnote" else { return }
        switch url.host {
        case "dictate":
            // Only run a handoff if signed in; the keyboard guards this too.
            if account != nil { keyboardHandoff = KeyboardHandoff() }
        case "auth":
            await handleAuthCallback(url)
        default:
            break   // airnote://open just foregrounds the app
        }
    }

    /// When the app comes to the foreground, check whether the keyboard just
    /// asked to dictate (it can't open the app itself on iOS, so the user opens
    /// it). If so, auto-start the dictation handoff. Called on scenePhase .active.
    func checkKeyboardHandoffRequest() {
        guard account != nil, keyboardHandoff == nil,
              let requestedAt = SharedStore.keyboardDictationRequestedAt,
              Date().timeIntervalSince(requestedAt) < 120
        else { return }
        SharedStore.keyboardDictationRequestedAt = nil   // consume
        keyboardHandoff = KeyboardHandoff()
    }

    /// Persists a finished dictation for the keyboard to insert when the user
    /// swipes back to their app.
    func deliverKeyboardDictation(_ text: String) {
        let clean = HinglishScript.enforceRomanHinglish(text)
        guard !clean.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        SharedStore.putPendingKeyboardText(clean, at: Date())
        // Keep the mic warm (while still foreground) so the NEXT keyboard dictation
        // happens in-place with no app switch.
        WarmDictationHost.shared.warmUp()
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

    /// Manual-token sign-in fallback (enterprise): the user pastes a session
    /// token (e.g. from the server's web sign-in) when the Lark deep-link flow
    /// can't complete. Mirrors the desktop connect form's "paste token" option.
    @discardableResult
    func signInWithToken(_ rawToken: String) async -> Bool {
        let token = rawToken.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else { return false }
        isAuthenticating = true
        authError = nil
        defer { isAuthenticating = false }
        do {
            let response = try await gateway.restoreSession(token: token)
            authTokens.persist(accessToken: response.token, account: response.account)
            account = response.account
            await loadWorkspaceState()
            phase = isOnboardingComplete ? .ready : .onboarding
            return true
        } catch {
            authError = "That sign-in token wasn't accepted. Check it and try again."
            return false
        }
    }

    func signOut() {
        signOutLocally()
        phase = .onboarding
    }

    private func signOutLocally() {
        authTokens.clear()
        // Clear onboarding so the next account completes it fresh (and is prompted
        // for its own provider keys). Device-level steps auto-pass.
        SharedStore.onboardingComplete = false
        UserDefaults.standard.removeObject(forKey: onboardingCompleteKey)
        account = nil
        dictationAvailable = false
        runtimeStatusLabel = "Signed out"
        settingsLoaded = false
        settingsVersion = 0
        history = []
        learnedEvents = []
        credentials = []
        vocabTermCount = 0
        vocabAliasCount = 0
        cancelLearningReview()
    }

    /// Inspects a thrown error; if the session is invalid, signs out so the user
    /// is routed back to authentication. Returns true if it was an auth failure.
    @discardableResult
    private func handleUnauthorized(_ error: Error) -> Bool {
        if let gatewayError = error as? GatewayError, gatewayError.isUnauthorized {
            // Only the first 401 signs out; ignore stale/concurrent 401s that
            // arrive after the user has already signed out or re-authenticated.
            guard account != nil else { return true }
            signOut()
            authError = "Your session expired. Please sign in again."
            return true
        }
        return false
    }

    // MARK: Workspace bootstrap (after auth)

    private func loadWorkspaceState() async {
        // Fresh account → discard any cached settings-version watermark so the new
        // account's (possibly lower-versioned) settings actually apply.
        settingsVersion = 0
        async let status: Void = refreshRuntimeStatus()
        async let settings: Void = loadSettings()
        async let history: Void = refreshHistory()
        async let credentials: Void = refreshCredentials()
        async let vocabulary: Void = refreshVocabulary()
        _ = await (status, settings, history, credentials, vocabulary)
    }

    func refreshRuntimeStatus() async {
        do {
            let status = try await gateway.runtimeStatus()
            // dictationAvailable is owned by refreshCredentials — it needs BOTH
            // required providers (deepgram + groq), not just any active credential
            // (an optional Gemini key alone must not flip it on).
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
        selectedModel = "smart"   // model picker removed — always Smart, matching desktop
        tonePreset = settings.tonePreset
        learningEnabled = settings.learningEnabled
        SharedStore.outputLanguage = settings.outputLanguage
        SharedStore.selectedModel = "smart"   // model picker removed — always Smart
        SharedStore.tonePreset = settings.tonePreset
    }

    func setOutputLanguage(_ value: String) async { await patch(.init(outputLanguage: value)) { self.outputLanguage = value; SharedStore.outputLanguage = value } }
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
            cacheSafeVocabTerms()
            vocabStatus = ""
        } catch {
            if handleUnauthorized(error) { return }
            vocabStatus = "Couldn't load learned terms"
        }
    }

    /// Mirror the most-recent learned terms into the App Group so every dictation
    /// (app + keyboard warm path) sends them as `safe_vocab_terms`. Newest first,
    /// deduped, capped — this is what makes taught names survive polish today
    /// without any server change.
    private func cacheSafeVocabTerms() {
        var seen = Set<String>()
        var terms: [String] = []
        for event in learnedEvents {
            for term in event.learnedTerms {
                let key = term.lowercased()
                if !key.isEmpty, seen.insert(key).inserted {
                    terms.append(term)
                }
            }
        }
        SharedStore.safeVocabTerms = terms
    }

    @discardableResult
    func addVocabulary(term: String, heardAs: String) async -> Bool {
        let trimmedTerm = term.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedTerm.isEmpty else { return false }
        let alias = heardAs.trimmingCharacters(in: .whitespacesAndNewlines)
        let aliases: [(heard: String, correct: String)] = alias.isEmpty ? [] : [(alias, trimmedTerm)]
        do {
            let result = try await gateway.addVocabulary(terms: [trimmedTerm], aliases: aliases)
            if result.learnedCount > 0 {
                if !alias.isEmpty { SharedStore.addLearnedAlias(heard: alias, correct: trimmedTerm) }
                vocabStatus = "Added \(trimmedTerm)"
                await refreshVocabulary()
                return true
            } else if result.blockedCount > 0 {
                vocabStatus = "“\(trimmedTerm)” is too common to add as a custom term."
                return false
            } else {
                vocabStatus = "Couldn't add that term"
                return false
            }
        } catch {
            if handleUnauthorized(error) { return false }
            vocabStatus = "Couldn't add that term"
            return false
        }
    }

    // MARK: Provider credentials (BYOK)

    func hasCredential(_ provider: String) -> Bool {
        credentials.contains { $0.provider.lowercased() == provider.lowercased() && $0.status.lowercased() != "revoked" }
    }

    /// Required providers (deepgram, groq) that the user hasn't added yet.
    var missingRequiredProviders: [String] {
        Self.requiredProviders.filter { !hasCredential($0) }
    }

    func refreshCredentials() async {
        guard account != nil else { credentials = []; return }
        do {
            credentials = try await gateway.listCredentials()
            // Dictation needs BOTH required providers (deepgram + groq); an
            // optional Gemini key alone must not turn this on.
            dictationAvailable = missingRequiredProviders.isEmpty
        } catch {
            _ = handleUnauthorized(error)
        }
    }

    @discardableResult
    func saveProviderKey(provider: String, secret: String) async -> Bool {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count >= 8 else {
            credentialStatus = "That key looks too short."
            return false
        }
        credentialWorking = true
        defer { credentialWorking = false }
        do {
            _ = try await gateway.saveCredential(provider: provider, secret: trimmed)
            credentialStatus = "\(provider.capitalized) key saved"
            await refreshCredentials()
            await refreshRuntimeStatus()   // flips dictationAvailable once both keys exist
            return true
        } catch let error as GatewayError {
            if error.isUnauthorized { signOut(); return false }
            credentialStatus = "Couldn't save the \(provider.capitalized) key."
            return false
        } catch {
            credentialStatus = "Couldn't save the \(provider.capitalized) key."
            return false
        }
    }

    func deleteCredential(_ credential: RuntimeCredential) async {
        let snapshot = credentials
        credentials.removeAll { $0.id == credential.id }
        do {
            try await gateway.deleteCredential(id: credential.id)
            dictationAvailable = missingRequiredProviders.isEmpty
            await refreshRuntimeStatus()
        } catch {
            if handleUnauthorized(error) { return }
            credentials = snapshot
            credentialStatus = "Couldn't remove that key."
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

    func analyzeLearningEdit(kept rawKept: String) async {
        guard let item = learningItem else { return }
        let kept = rawKept.trimmingCharacters(in: .whitespacesAndNewlines)
        learningDraftText = kept
        guard !kept.isEmpty else {
            learningStatus = "The corrected text can't be empty"
            return
        }
        learningWorking = true
        learningCandidates = []
        defer { learningWorking = false }
        do {
            let analysis = try await gateway.analyzeEdit(
                recordingID: item.learningRecordingID,
                transcript: item.transcriptText,
                aiOutput: item.learningAIOutput,
                userKept: kept
            )
            learningCandidates = analysis.candidates.filter(\.learnable)
            // Base the message on the actual candidates — the server's `changed`
            // flag means "candidates were refined", not "an edit was detected",
            // so candidates can come back with changed=false.
            if !learningCandidates.isEmpty {
                learningStatus = "\(learningCandidates.count) correction\(learningCandidates.count == 1 ? "" : "s") ready to learn"
            } else if analysis.changed {
                learningStatus = "No safe corrections found in that edit"
            } else {
                learningStatus = "No change detected — edit the kept text, then analyze"
            }
        } catch {
            if handleUnauthorized(error) { return }
            learningStatus = "Could not analyze this correction"
        }
    }

    func confirmLearning(selectedIDs: Set<String>) async {
        guard let item = learningItem else { return }
        let items = learningCandidates.filter { $0.learnable && selectedIDs.contains($0.id) }
        guard !items.isEmpty else {
            learningStatus = "Select at least one correction to learn"
            return
        }
        learningWorking = true
        defer { learningWorking = false }
        do {
            let result = try await gateway.confirmLearning(recordingID: item.learningRecordingID, items: items)
            if result.learnedCount > 0 {
                // Cache the heard→meant mapping locally so the client applies it
                // to future dictation output (the server's streaming path won't).
                for candidate in items {
                    SharedStore.addLearnedAlias(heard: candidate.original, correct: candidate.corrected)
                }
                learningStatus = "✓ Learned \(result.learnedTerms.isEmpty ? "\(result.learnedCount) correction\(result.learnedCount == 1 ? "" : "s")" : result.learnedTerms.joined(separator: ", "))"
                learningCandidates = []
                await refreshVocabulary()
                // Show the success briefly, then close the sheet (if still open).
                let reviewedID = item.id
                try? await Task.sleep(nanoseconds: 1_100_000_000)
                if learningItem?.id == reviewedID { cancelLearningReview() }
            } else if result.blockedCount > 0 {
                // The server blocks corrections that are too common or unsafe.
                learningStatus = "That's too common to learn as a custom term."
                learningCandidates = []
            } else {
                learningStatus = "Nothing new to learn here"
                learningCandidates = []
            }
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
            safeVocabTerms: SharedStore.safeVocabTerms,
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
