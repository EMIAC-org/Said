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

    // MARK: Dictation preferences (so the keyboard can request the right model/language)

    public static var outputLanguage: String {
        get { string(Key.outputLanguage) ?? "hinglish" }
        set { set(Key.outputLanguage, newValue) }
    }

    public static var selectedModel: String {
        get { string(Key.selectedModel) ?? "fast" }
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
