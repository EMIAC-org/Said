#if canImport(AppIntents)
import AppIntents

/// App Intents invoked by the Live Activity (Dynamic Island) Stop/Resume buttons.
/// They flip the persisted session intent + post a Darwin signal; the app's
/// WarmDictationHost reconciles the actual warm engine.

@available(iOS 17.0, *)
public struct StopSessionIntent: AppIntent {
    public static var title: LocalizedStringResource = "Pause AirNote session"
    /// Pausing only ends the warm engine — no need to foreground the app.
    public static var openAppWhenRun: Bool = false

    public init() {}

    public func perform() async throws -> some IntentResult {
        SharedStore.sessionEnabled = false
        DarwinSignal.shared.post(DarwinSignal.sessionControl)
        return .result()
    }
}

@available(iOS 17.0, *)
public struct ResumeSessionIntent: AppIntent {
    public static var title: LocalizedStringResource = "Resume AirNote session"
    /// Resuming must foreground the app — iOS only lets a foreground app start the
    /// mic, so we open AirNote, which re-arms the warm session on becoming active.
    public static var openAppWhenRun: Bool = true

    public init() {}

    public func perform() async throws -> some IntentResult {
        SharedStore.sessionEnabled = true
        DarwinSignal.shared.post(DarwinSignal.sessionControl)
        return .result()
    }
}
#endif
