import SwiftUI
import AirNoteShared

struct FirstDictationView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var draft = "Draft Rahul ko bhejna hai"

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 18) {
                    VStack(spacing: 8) {
                        Text("Try your first dictation")
                            .font(.system(.title, design: .rounded).weight(.bold))
                            .multilineTextAlignment(.center)
                        Text("Type or dictate into this safe practice field, then mark it inserted.")
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    }
                    .padding(.top, 12)

                    AirNoteCard {
                        VStack(alignment: .leading, spacing: 10) {
                            AirNoteSectionLabel(text: "Practice field")
                            TextEditor(text: $draft)
                                .frame(minHeight: 140)
                                .scrollContentBackground(.hidden)
                                .font(.body)
                        }
                    }

                    Button {
                        environment.dictationStore.append(
                            DictationRecord(
                                transcript: "kal jo macobs wala update hai",
                                polished: "Kal jo Macobs wala update hai, usko concise bana ke Rahul ko bhej do.",
                                outcome: .inserted
                            )
                        )
                        environment.markSetupReady()
                    } label: {
                        Label("Mark first dictation inserted", systemImage: "checkmark.circle.fill")
                    }
                    .buttonStyle(AirNotePrimaryButtonStyle())
                }
                .padding(18)
            }
        }
        .navigationTitle("Practice")
        .navigationBarTitleDisplayMode(.inline)
    }
}

#Preview("Practice") {
    NavigationStack {
        FirstDictationView()
            .environmentObject(AppEnvironment())
    }
}
