import AirNoteShared
import SwiftUI

struct VocabularyScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var term = ""
    @State private var heardAs = ""
    @State private var adding = false
    @FocusState private var termFocused: Bool

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                ScrollView {
                    VStack(spacing: 16) {
                        statsCard
                        addCard
                        learnedCard
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                    .padding(.bottom, 28)
                }
            }
            .navigationTitle("Vocabulary")
            .navigationBarTitleDisplayMode(.large)
            .task { await env.refreshVocabulary() }
            .refreshable { await env.refreshVocabulary() }
        }
    }

    private var statsCard: some View {
        AirNoteCard(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                AirNoteSectionLabel(text: "Personal memory")
                HStack(spacing: 10) {
                    StatTile(value: "\(env.vocabTermCount)", label: "terms", systemImage: "textformat.abc")
                    StatTile(value: "\(env.vocabAliasCount)", label: "corrections", systemImage: "arrow.2.squarepath")
                }
                Text("AirNote uses these to spell your names, jargon, and brands correctly.")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
            }
        }
    }

    private var addCard: some View {
        AirNoteCard(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                AirNoteSectionLabel(text: "Add a term")
                TextField("Correct spelling (e.g. a name or brand)", text: $term)
                    .textFieldStyle(AirNoteFieldStyle())
                    .focused($termFocused)
                    .autocorrectionDisabled()
                TextField("Heard as (optional)", text: $heardAs)
                    .textFieldStyle(AirNoteFieldStyle())
                    .autocorrectionDisabled()
                if !env.vocabStatus.isEmpty {
                    Text(env.vocabStatus)
                        .font(.caption)
                        .foregroundStyle(env.vocabStatus.hasPrefix("Couldn't") ? AirNoteDesign.danger : AirNoteDesign.success)
                }
                Button {
                    let value = term
                    let alias = heardAs
                    adding = true
                    termFocused = false
                    Task {
                        let ok = await env.addVocabulary(term: value, heardAs: alias)
                        await MainActor.run {
                            adding = false
                            if ok { term = ""; heardAs = "" }
                        }
                    }
                } label: {
                    Label(adding ? "Adding…" : "Add term", systemImage: "text.badge.plus")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
                .disabled(adding || term.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
    }

    private var learnedCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeader("Recently learned")
            AirNoteCard(padding: 14) {
                let events = env.learnedEvents.filter { !$0.learnedTerms.isEmpty }
                if env.vocabLoading && events.isEmpty {
                    InlineLoading(text: "Loading learned terms…")
                } else if events.isEmpty {
                    EmptyStateCard(
                        systemImage: "sparkles",
                        title: "Nothing learned yet",
                        message: "Add a term above, or correct a dictation in History to teach AirNote."
                    )
                } else {
                    VStack(alignment: .leading, spacing: 12) {
                        ForEach(events) { event in
                            VStack(alignment: .leading, spacing: 6) {
                                FlowChips(terms: event.learnedTerms)
                                Text(event.createdAt, format: .relative(presentation: .named))
                                    .font(.caption2)
                                    .foregroundStyle(AirNoteDesign.muted)
                            }
                            if event.id != events.last?.id {
                                Divider().overlay(AirNoteDesign.border)
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Wrapping row of term chips.
private struct FlowChips: View {
    var terms: [String]

    var body: some View {
        let columns = [GridItem(.adaptive(minimum: 60, maximum: 200), spacing: 6, alignment: .leading)]
        LazyVGrid(columns: columns, alignment: .leading, spacing: 6) {
            ForEach(Array(terms.prefix(12).enumerated()), id: \.offset) { _, term in
                Text(term)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                    .lineLimit(1)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 5)
                    .background(AirNoteDesign.accent.opacity(0.12), in: Capsule())
            }
        }
    }
}

struct AirNoteFieldStyle: TextFieldStyle {
    func _body(configuration: TextField<Self._Label>) -> some View {
        configuration
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(AirNoteDesign.border, lineWidth: 1))
            .font(.subheadline)
    }
}
