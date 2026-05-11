import SwiftUI

struct HistoryView: View {
    let sidecar: SidecarManager

    @State private var recordings: [Recording] = []
    @State private var searchText = ""

    var filtered: [Recording] {
        if searchText.isEmpty { return recordings }
        return recordings.filter {
            ($0.polished ?? "").localizedCaseInsensitiveContains(searchText) ||
            ($0.transcript ?? "").localizedCaseInsensitiveContains(searchText)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("History")
                    .font(.title.bold())
                Spacer()
                TextField("Search…", text: $searchText)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 200)
            }
            .padding(24)

            if filtered.isEmpty {
                ContentUnavailableView.search(text: searchText)
                    .frame(maxHeight: .infinity)
            } else {
                List(filtered) { rec in
                    HStack {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(rec.polished ?? rec.transcript ?? "—")
                                .lineLimit(2)
                            HStack(spacing: 8) {
                                if let app = rec.target_app {
                                    Text(app).font(.caption).foregroundStyle(.secondary)
                                }
                                Text(rec.formattedDate)
                                    .font(.caption).foregroundStyle(.tertiary)
                                if let source = rec.source {
                                    Text(source)
                                        .font(.caption2)
                                        .padding(.horizontal, 4)
                                        .padding(.vertical, 1)
                                        .background(.quaternary)
                                        .clipShape(Capsule())
                                }
                            }
                        }
                        Spacer()
                        Button {
                            if let text = rec.polished ?? rec.transcript {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(text, forType: .string)
                            }
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        .buttonStyle(.borderless)
                        .help("Copy to clipboard")

                        Button {
                            Task { await deleteRecording(rec.id) }
                        } label: {
                            Image(systemName: "trash")
                                .foregroundStyle(.red)
                        }
                        .buttonStyle(.borderless)
                        .help("Delete")
                    }
                    .padding(.vertical, 4)
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
        do {
            recordings = try await client.getHistory(limit: 200)
        } catch {
            print("[history] load failed: \(error)")
        }
    }

    private func deleteRecording(_ id: String) async {
        let client = BackendClient(sidecar: sidecar)
        try? await client.deleteRecording(id)
        recordings.removeAll { $0.id == id }
    }
}
