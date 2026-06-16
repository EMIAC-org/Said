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
                        WarmDictationHost.shared.ensureSessionActive()
                    }
                }
        }
    }
}
