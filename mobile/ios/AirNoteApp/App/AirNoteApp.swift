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
                .task { await environment.bootstrap() }
                .onOpenURL { url in
                    Task { await environment.handleAuthCallback(url) }
                }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active { environment.permissions.refreshAll() }
                }
        }
    }
}
