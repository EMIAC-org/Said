import SwiftUI

struct PrivacyConsentView: View {
    @Binding var accepted: Bool

    var body: some View {
        List {
            Section {
                Text("AirNote sends your recording to AirNote servers for transcription and polish. Provider keys stay server-side.")
                    .font(.subheadline)
                Toggle("I understand cloud processing", isOn: $accepted)
            }

            Section("How AirNote protects you") {
                Label("Records only after a visible user action", systemImage: "record.circle")
                Label("Never runs in password or OTP fields", systemImage: "lock.fill")
                Label("Delete history and account data anytime", systemImage: "trash")
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("Privacy")
        .navigationBarTitleDisplayMode(.inline)
    }
}
