import Foundation

/// App-Group-shared, non-secret runtime state that both the main app and the
/// keyboard extension read/write. The access token lives here (in addition to
/// the Keychain) specifically so the keyboard extension — which cannot read the
/// app's private Keychain — can authenticate its own streaming connection.
///
/// The App Group container is sandboxed to AirNote's app + extensions, so a
/// 30-day session token stored here is acceptable for v1. Nothing in this store
/// is a provider secret; provider keys never leave the server.
public enum SharedStore {
    private static var defaults: UserDefaults? {
        UserDefaults(suiteName: BuildConfig.appGroupIdentifier)
    }

    private enum Key {
        static let accessToken = "airnote.shared.access_token"
        static let accountEmail = "airnote.shared.account_email"
        static let accountJSON = "airnote.shared.account_json"
        static let outputLanguage = "airnote.shared.output_language"
        static let selectedModel = "airnote.shared.selected_model"
        static let tonePreset = "airnote.shared.tone_preset"
        static let keyboardHasFullAccess = "airnote.shared.keyboard_full_access"
        static let keyboardHealthAt = "airnote.shared.keyboard_health_at"
        static let onboardingComplete = "airnote.shared.onboarding_complete"
        static let kbdDictationRequestedAt = "airnote.shared.kbd_dictation_requested_at"
        static let pendingKbdText = "airnote.shared.pending_kbd_text"
        static let pendingKbdTextAt = "airnote.shared.pending_kbd_text_at"
        static let sessionWarmUntil = "airnote.shared.session_warm_until"
        static let sessionDurationMinutes = "airnote.shared.session_duration_minutes"
        static let safeVocabTerms = "airnote.shared.safe_vocab_terms"
        static let keyboardLivePartial = "airnote.shared.kbd_live_partial"
        static let keyboardLiveLevel = "airnote.shared.kbd_live_level"
        static let learnedAliases = "airnote.shared.learned_aliases"
        static let customGatewayURL = "airnote.shared.custom_gateway_url"
        static let recentGatewayURLs = "airnote.shared.recent_gateway_urls"
        static let activeOrgID = "airnote.shared.active_org_id"
        static let keyboardKeysCollapsed = "airnote.shared.keyboard_keys_collapsed"
        static let workspaceChosenPersonal = "airnote.shared.workspace_chosen_personal"
        static let profileDisplayName = "airnote.shared.profile_display_name"
        static let profileAccentIndex = "airnote.shared.profile_accent_index"
    }

    /// Locally cached learned corrections (heard -> meant), captured whenever the
    /// user teaches one. The client applies these to dictation output so taught
    /// names actually get fixed even though the server's streaming path doesn't
    /// merge learned aliases. Newest first, capped.
    public static var learnedAliases: [LearnedAliasPair] {
        get {
            guard let data = defaults?.data(forKey: Key.learnedAliases),
                  let pairs = try? JSONDecoder().decode([LearnedAliasPair].self, from: data)
            else { return [] }
            return pairs
        }
        set {
            let capped = Array(newValue.prefix(120))
            defaults?.set(try? JSONEncoder().encode(capped), forKey: Key.learnedAliases)
        }
    }

    /// Record a taught correction (newest first, deduped, only if it's a real
    /// heard≠meant pair that passes the resolver's STORE-time safety gate — which
    /// rejects homophone/word-swaps that would corrupt ordinary dictation once
    /// auto-applied).
    @discardableResult
    public static func addLearnedAlias(heard: String, correct: String) -> Bool {
        // Strip surrounding whitespace AND sentence punctuation so a diff that
        // captured "jai," stores the bare word "jai" and fires on later plain
        // occurrences (internal punctuation like node.js / n8n is untouched).
        let strip = CharacterSet.whitespacesAndNewlines.union(CharacterSet(charactersIn: ".,;:!?\"'…"))
        let h = heard.trimmingCharacters(in: strip)
        let c = correct.trimmingCharacters(in: strip)
        guard LearnedAliasResolver.isSafeToLearn(heard: h, correct: c) else { return false }
        let pair = LearnedAliasPair(heard: h, correct: c)
        var current = learnedAliases.filter {
            !($0.heard.caseInsensitiveCompare(h) == .orderedSame && $0.correct == c)
        }
        current.insert(pair, at: 0)
        learnedAliases = current
        return true
    }

    /// The latest live (already romanized) partial transcript during a warm
    /// keyboard dictation, written by the app and read by the keyboard so it can
    /// show words as the user speaks. Empty string = nothing to show / cleared.
    public static var keyboardLivePartial: String {
        get { defaults?.string(forKey: Key.keyboardLivePartial) ?? "" }
        set { defaults?.set(newValue, forKey: Key.keyboardLivePartial) }
    }

    /// Latest mic level (0...1) the warm app is capturing, so the keyboard can drive
    /// its live waveform from the real voice (it can't capture audio in-process).
    public static var keyboardLiveLevel: Double {
        get { defaults?.double(forKey: Key.keyboardLiveLevel) ?? 0 }
        set { defaults?.set(newValue, forKey: Key.keyboardLiveLevel) }
    }

    /// The user's learned vocabulary (names/brands they've taught), cached by the
    /// app so every dictation — including the keyboard's warm path — can send them
    /// as `safe_vocab_terms`. The server polish prompt uses these as "SAFE LOCAL
    /// VOCAB HINTS", so taught terms (e.g. "Anugra") start surviving polish even
    /// without the server-side learned-memory merge. Capped at 30 (server limit).
    public static var safeVocabTerms: [String] {
        get { (defaults?.array(forKey: Key.safeVocabTerms) as? [String]) ?? [] }
        set {
            let cleaned = newValue
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
            defaults?.set(Array(cleaned.prefix(30)), forKey: Key.safeVocabTerms)
        }
    }

    /// How long a keyboard session stays warm after the last dictation, in
    /// minutes. -1 means "until I stop it / the app is killed". Default 5, like
    /// Wispr's shortest option.
    public static var sessionDurationMinutes: Int {
        get { (defaults?.object(forKey: Key.sessionDurationMinutes) as? Int) ?? 5 }
        set { defaults?.set(newValue, forKey: Key.sessionDurationMinutes) }
    }

    /// Until when the app is holding the mic warm in the background. While this is
    /// in the future, the keyboard can dictate IN-PLACE (no app switch) by
    /// signalling the warm app over Darwin notifications.
    public static var sessionWarmUntil: Date? {
        get {
            let ts = defaults?.double(forKey: Key.sessionWarmUntil) ?? 0
            return ts > 0 ? Date(timeIntervalSince1970: ts) : nil
        }
        set { defaults?.set(newValue?.timeIntervalSince1970 ?? 0, forKey: Key.sessionWarmUntil) }
    }

    public static var isSessionWarm: Bool {
        guard let until = sessionWarmUntil else { return false }
        return until > Date()
    }

    /// Onboarding-complete flag, in the App Group so it survives a reinstall
    /// together with the restored session.
    public static var onboardingComplete: Bool {
        get { defaults?.bool(forKey: Key.onboardingComplete) ?? false }
        set { defaults?.set(newValue, forKey: Key.onboardingComplete) }
    }

    // MARK: Auth (read by the keyboard extension to stream directly)

    public static var accessToken: String? {
        get { string(Key.accessToken) }
        set { set(Key.accessToken, newValue) }
    }

    public static var accountEmail: String? {
        get { string(Key.accountEmail) }
        set { set(Key.accountEmail, newValue) }
    }

    /// Full account (id/email/tier) as JSON. The App Group container survives an
    /// app reinstall on unsigned simulator builds (unlike the Keychain), so this
    /// lets the session restore even when the Keychain token was dropped.
    public static var accountJSON: String? {
        get { string(Key.accountJSON) }
        set { set(Key.accountJSON, newValue) }
    }

    // MARK: Server connection (enterprise / self-hosted override)

    /// Enterprise/self-hosted override for the control-plane server URL. When set,
    /// BuildConfig.gatewayBaseURL returns this so BOTH the app and the keyboard
    /// extension talk to the chosen server. nil = use the built-in default.
    public static var customGatewayURL: String? {
        get { string(Key.customGatewayURL) }
        set { set(Key.customGatewayURL, newValue) }
    }

    /// Recently-used control-plane URLs (newest first, max 5) so the enterprise
    /// connect screen can offer one-tap reconnect — mirrors the desktop's recent
    /// workspaces list.
    public static var recentGatewayURLs: [String] {
        get { (defaults?.array(forKey: Key.recentGatewayURLs) as? [String]) ?? [] }
        set { defaults?.set(Array(newValue.prefix(5)), forKey: Key.recentGatewayURLs) }
    }

    /// Push a URL to the front of the recents list (deduped, capped).
    public static func rememberGatewayURL(_ url: String) {
        let u = url.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !u.isEmpty else { return }
        var list = recentGatewayURLs.filter { $0.caseInsensitiveCompare(u) != .orderedSame }
        list.insert(u, at: 0)
        recentGatewayURLs = list
    }

    /// The active workspace (org) id, or nil for personal mode. Read by the
    /// gateway client to send the `X-AirNote-Org-Id` header on org-scoped
    /// endpoints (meetings, divo). Lives in the App Group so the keyboard's
    /// client carries the same workspace.
    public static var activeOrgID: String? {
        get { string(Key.activeOrgID) }
        set { set(Key.activeOrgID, newValue) }
    }

    /// Whether the keyboard's typing keys are collapsed (compact voice mode): the
    /// keyboard shrinks to the voice surface + a handle; tap to bring keys back.
    public static var keyboardKeysCollapsed: Bool {
        get { defaults?.bool(forKey: Key.keyboardKeysCollapsed) ?? false }
        set { defaults?.set(newValue, forKey: Key.keyboardKeysCollapsed) }
    }

    /// True once the user explicitly chose Personal mode, so we stop auto-selecting
    /// their workspace on launch.
    public static var workspaceChosenPersonal: Bool {
        get { defaults?.bool(forKey: Key.workspaceChosenPersonal) ?? false }
        set { defaults?.set(newValue, forKey: Key.workspaceChosenPersonal) }
    }

    /// User-chosen display name for the profile/avatar. Empty falls back to email.
    public static var profileDisplayName: String {
        get { string(Key.profileDisplayName) ?? "" }
        set { set(Key.profileDisplayName, newValue.isEmpty ? nil : newValue) }
    }

    /// Index into the profile accent palette (0 = app default blue). Tints the
    /// avatar across the app — purely cosmetic personalization.
    public static var profileAccentIndex: Int {
        get { defaults?.integer(forKey: Key.profileAccentIndex) ?? 0 }
        set { defaults?.set(newValue, forKey: Key.profileAccentIndex) }
    }

    // MARK: Dictation preferences (so the keyboard can request the right model/language)

    public static var outputLanguage: String {
        get { string(Key.outputLanguage) ?? "hinglish" }
        set { set(Key.outputLanguage, newValue) }
    }

    /// The desktop removed its model picker and always uses the higher-quality
    /// model; iOS matches that — always "smart". (Setter kept so settings-sync
    /// writes don't error; the getter intentionally ignores the stored value.)
    public static var selectedModel: String {
        get { "smart" }
        set { set(Key.selectedModel, newValue) }
    }

    public static var tonePreset: String {
        get { string(Key.tonePreset) ?? "work" }
        set { set(Key.tonePreset, newValue) }
    }

    // MARK: Keyboard → app health handshake

    /// Written by the keyboard extension on every load so the main app can show
    /// an accurate "keyboard enabled + Full Access" status without guessing.
    public static var keyboardHasFullAccess: Bool {
        get { defaults?.bool(forKey: Key.keyboardHasFullAccess) ?? false }
        set { defaults?.set(newValue, forKey: Key.keyboardHasFullAccess) }
    }

    /// Timestamp of the last keyboard load. If recent, the keyboard extension has
    /// been instantiated at least once (i.e. it is installed and was opened).
    public static var keyboardLastSeen: Date? {
        get {
            let ts = defaults?.double(forKey: Key.keyboardHealthAt) ?? 0
            return ts > 0 ? Date(timeIntervalSince1970: ts) : nil
        }
        set { defaults?.set(newValue?.timeIntervalSince1970 ?? 0, forKey: Key.keyboardHealthAt) }
    }

    /// Called by the keyboard extension whenever it loads, recording whether it
    /// currently has Full Access (network) granted.
    public static func recordKeyboardHealth(hasFullAccess: Bool, at date: Date) {
        keyboardHasFullAccess = hasFullAccess
        keyboardLastSeen = date
    }

    // MARK: Keyboard ⇄ app dictation handoff
    //
    // iOS forbids microphone capture inside a keyboard extension. So the keyboard
    // asks the main app to record (via the airnote://dictate deep link); the app
    // records + polishes, drops the result here, and the keyboard inserts it when
    // the user swipes back.

    /// When the keyboard last asked the app to dictate.
    public static var keyboardDictationRequestedAt: Date? {
        get {
            let ts = defaults?.double(forKey: Key.kbdDictationRequestedAt) ?? 0
            return ts > 0 ? Date(timeIntervalSince1970: ts) : nil
        }
        set { defaults?.set(newValue?.timeIntervalSince1970 ?? 0, forKey: Key.kbdDictationRequestedAt) }
    }

    /// The polished text the app produced for the keyboard to insert.
    public static var pendingKeyboardText: String? {
        get { string(Key.pendingKbdText) }
        set { set(Key.pendingKbdText, newValue) }
    }

    /// When `pendingKeyboardText` was written (so the keyboard only inserts a
    /// result that is newer than its request).
    public static var pendingKeyboardTextAt: Date? {
        get {
            let ts = defaults?.double(forKey: Key.pendingKbdTextAt) ?? 0
            return ts > 0 ? Date(timeIntervalSince1970: ts) : nil
        }
        set { defaults?.set(newValue?.timeIntervalSince1970 ?? 0, forKey: Key.pendingKbdTextAt) }
    }

    public static func putPendingKeyboardText(_ text: String, at date: Date) {
        pendingKeyboardText = text
        pendingKeyboardTextAt = date
    }

    public static func clearKeyboardDictation() {
        pendingKeyboardText = nil
        set(Key.pendingKbdTextAt, nil)
        set(Key.kbdDictationRequestedAt, nil)
    }

    public static func clearAuth() {
        set(Key.accessToken, nil)
        set(Key.accountEmail, nil)
        set(Key.accountJSON, nil)
    }

    // MARK: Helpers

    private static func string(_ key: String) -> String? {
        guard let value = defaults?.string(forKey: key)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !value.isEmpty
        else { return nil }
        return value
    }

    private static func set(_ key: String, _ value: String?) {
        if let value, !value.isEmpty {
            defaults?.set(value, forKey: key)
        } else {
            defaults?.removeObject(forKey: key)
        }
    }
}
