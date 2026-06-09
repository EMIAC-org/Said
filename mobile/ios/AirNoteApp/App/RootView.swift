import SwiftUI

struct RootView: View {
    @EnvironmentObject private var env: AppEnvironment

    var body: some View {
        ZStack {
            switch env.phase {
            case .launching:
                LaunchView()
            case .onboarding:
                OnboardingFlow()
            case .ready:
                MainTabView()
            }
        }
        .animation(.easeInOut(duration: 0.3), value: env.phase)
        .fullScreenCover(item: $env.keyboardHandoff) { _ in
            NavigationStack {
                DictationSheet(env: env, showsDoneButton: false, handoffMode: true) { result in
                    env.deliverKeyboardDictation(result.polished)
                }
            }
        }
    }
}

struct LaunchView: View {
    var body: some View {
        ZStack {
            AirNoteBackground()
            VStack(spacing: 16) {
                AirNoteLogoTile(size: 72)
                ProgressView()
                    .controlSize(.regular)
                    .tint(AirNoteDesign.accent)
            }
        }
    }
}
