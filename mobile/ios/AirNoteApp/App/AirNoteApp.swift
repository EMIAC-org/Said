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
                }
                .onOpenURL { url in
                    Task { await environment.handleDeepLink(url) }
                }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active {
                        WarmDictationHost.shared.startObserving()
                        environment.permissions.refreshAll()
                        environment.checkKeyboardHandoffRequest()
                    }
                }
        }
    }
}
