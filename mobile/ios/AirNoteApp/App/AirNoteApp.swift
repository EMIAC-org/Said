import SwiftUI

@main
struct AirNoteApp: App {
    @StateObject private var environment = AppEnvironment()

    var body: some Scene {
        WindowGroup {
            HomeView()
                .environmentObject(environment)
        }
    }
}
