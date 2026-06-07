import SwiftUI
import AirNoteShared

struct FirstDictationView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        VStack(spacing: 18) {
            Text("Try your first AirNote dictation")
                .font(.title.bold())
                .multilineTextAlignment(.center)

            Text("Use the practice field, start a visible session, and insert or save the polished result.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)

            TextEditor(text: .constant("Draft Rahul ko bhejna hai"))
                .frame(minHeight: 150)
                .padding(8)
                .overlay(RoundedRectangle(cornerRadius: AirNoteDesign.radius).stroke(.quaternary))

            Button("Mark first dictation inserted") {
                environment.dictationStore.append(
                    DictationRecord(
                        transcript: "kal jo macobs wala update hai",
                        polished: "Kal jo Macobs wala update hai, usko concise bana ke Rahul ko bhej do.",
                        outcome: .inserted
                    )
                )
                environment.markSetupReady()
            }
            .buttonStyle(.borderedProminent)
        }
        .padding()
        .navigationTitle("Practice")
    }
}
