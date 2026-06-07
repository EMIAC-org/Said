import SwiftUI

struct PrivacyConsentView: View {
    @Binding var accepted: Bool

    var body: some View {
        List {
            Section {
                Text("AirNote sends your recording to AirNote servers for STT and polish. Provider keys stay server-side.")
                Toggle("I understand cloud processing", isOn: $accepted)
            }

            Section("Rules") {
                Label("Records only after a visible user action", systemImage: "record.circle")
                Label("Does not run in password fields", systemImage: "lock")
                Label("Delete history and account data from Settings", systemImage: "trash")
            }
        }
        .navigationTitle("Privacy")
    }
}
