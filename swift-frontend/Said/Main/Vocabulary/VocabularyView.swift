import SwiftUI

struct VocabularyView: View {
    let sidecar: SidecarManager

    @State private var terms: [VocabTerm] = []
    @State private var searchText = ""
    @State private var newTerm = ""

    var filtered: [VocabTerm] {
        if searchText.isEmpty { return terms }
        return terms.filter { $0.term.localizedCaseInsensitiveContains(searchText) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Vocabulary")
                    .font(.title.bold())
                Spacer()
                HStack(spacing: 8) {
                    TextField("Add term…", text: $newTerm)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 160)
                        .onSubmit { Task { await addTerm() } }
                    Button("Add") { Task { await addTerm() } }
                        .disabled(newTerm.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                TextField("Search…", text: $searchText)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 160)
            }
            .padding(24)

            if filtered.isEmpty {
                ContentUnavailableView(
                    "No vocabulary terms",
                    systemImage: "textformat.abc",
                    description: Text("Said learns new terms as you use it, or add them manually above.")
                )
                .frame(maxHeight: .infinity)
            } else {
                List(filtered) { term in
                    HStack {
                        Button {
                            Task { await star(term.term) }
                        } label: {
                            Image(systemName: term.starred == true ? "star.fill" : "star")
                                .foregroundStyle(term.starred == true ? .yellow : .secondary)
                        }
                        .buttonStyle(.borderless)

                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(term.term).font(.system(size: 13, weight: .semibold))
                                if let type = term.term_type {
                                    Text(type)
                                        .font(.system(size: 10))
                                        .padding(.horizontal, 5)
                                        .padding(.vertical, 1)
                                        .background(Color.gray.opacity(0.2))
                                        .clipShape(Capsule())
                                }
                            }
                            if let meaning = term.meaning {
                                Text(meaning)
                                    .font(.system(size: 11))
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                        }
                        Spacer()
                        if let source = term.source {
                            Text(source)
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                        Button {
                            Task { await deleteTerm(term.term) }
                        } label: {
                            Image(systemName: "trash")
                                .foregroundStyle(.red)
                        }
                        .buttonStyle(.borderless)
                    }
                    .padding(.vertical, 2)
                }
            }
        }
        .task { await loadData() }
    }

    private func loadData() async {
        while !sidecar.isHealthy {
            try? await Task.sleep(for: .milliseconds(300))
        }
        let client = BackendClient(sidecar: sidecar)
        terms = (try? await client.getVocabulary()) ?? []
    }

    private func addTerm() async {
        let t = newTerm.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty else { return }
        let client = BackendClient(sidecar: sidecar)
        try? await client.addVocabularyTerm(t)
        newTerm = ""
        await loadData()
    }

    private func deleteTerm(_ term: String) async {
        let client = BackendClient(sidecar: sidecar)
        try? await client.deleteVocabularyTerm(term)
        terms.removeAll { $0.term == term }
    }

    private func star(_ term: String) async {
        let client = BackendClient(sidecar: sidecar)
        try? await client.starVocabularyTerm(term)
        await loadData()
    }
}
