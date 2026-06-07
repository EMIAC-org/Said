import SwiftUI

@main
struct AirNoteApp: App {
    @StateObject private var environment = AppEnvironment()

    var body: some Scene {
        WindowGroup {
            Group {
                if case .ready = environment.setupState {
                    HomeView()
                } else {
                    SetupFlowView()
                }
            }
            .environmentObject(environment)
            .preferredColorScheme(.dark)
        }
    }
}
