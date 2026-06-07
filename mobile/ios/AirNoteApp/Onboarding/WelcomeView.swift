import SwiftUI

struct WelcomeView: View {
    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Speak naturally. AirNote writes clearly.")
                        .font(.title.bold())
                    Text("AirNote uses a native keyboard, a visible main-app recording session, and a hosted Gateway for transcription and polish.")
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 8)
            }

            Section("What AirNote needs") {
                Label("Microphone in the main app", systemImage: "mic")
                Label("Full Access for cloud dictation", systemImage: "keyboard")
                Label("No recording in secure fields", systemImage: "lock.shield")
            }
        }
        .navigationTitle("Welcome")
    }
}
