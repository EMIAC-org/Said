#if canImport(AppIntents)
import AppIntents
#if canImport(ActivityKit)
import ActivityKit
#endif

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
        // End the Live Activity HERE, not via the app. The notch is only visible
        // while the app is backgrounded, and tapping Stop background-launches this
        // intent into a process where the app's Darwin observer was never registered
        // (startObserving() runs on a scene becoming active, which a background
        // launch doesn't trigger) — so the sessionControl signal gets dropped and the
        // notch never clears, which is exactly why Stop looked dead. Ending the
        // Activity directly is allowed in the background and clears the notch
        // instantly; the app still tears the warm engine down when it next runs.
        #if canImport(ActivityKit)
        for activity in Activity<DictationSessionAttributes>.activities {
            await activity.end(nil, dismissalPolicy: .immediate)
        }
        #endif
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
