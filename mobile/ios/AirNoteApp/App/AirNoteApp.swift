import SwiftUI

@main
struct AirNoteApp: App {
    @StateObject private var environment = AppEnvironment()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(environment)
                .airNotePreferredAppearance()
                .task {
                    WarmDictationHost.shared.startObserving()
                    await environment.bootstrap()
                    // Wispr-style: the session turns on when the app opens (no-op
                    // unless the user's session intent is ON and the mic is granted).
                    WarmDictationHost.shared.ensureSessionActive()
                }
                .onOpenURL { url in
                    Task { await environment.handleDeepLink(url) }
                }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active {
                        WarmDictationHost.shared.startObserving()
                        environment.permissions.refreshAll()
                        environment.checkKeyboardHandoffRequest()
                        // Foreground: arm the warm session (iOS only starts the mic
                        // here) — this also creates the notch.
                        WarmDictationHost.shared.ensureSessionActive()
                    } else {
                        // Background/inactive: just reconcile the notch (never try to
                        // start the mic from the background).
                        WarmDictationHost.shared.reconcileNotch()
                    }
                }
        }
    }
}
