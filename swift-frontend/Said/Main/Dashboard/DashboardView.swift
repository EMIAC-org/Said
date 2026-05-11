import SwiftUI

struct DashboardView: View {
    let sidecar: SidecarManager

    @State private var recordings: [Recording] = []
    @State private var totalCount = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text("Dashboard")
                    .font(.title.bold())

                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 16) {
                    statCard("Recordings", "\(totalCount)", icon: "waveform")
                    statCard("Words polished", "\(recordings.reduce(0) { $0 + ($1.word_count ?? 0) })", icon: "text.word.spacing")
                }

                Text("Recent recordings")
                    .font(.headline)

                if recordings.isEmpty {
                    ContentUnavailableView(
                        "No recordings yet",
                        systemImage: "waveform.slash",
                        description: Text("Hold your recording key and start speaking.")
                    )
                } else {
                    ForEach(recordings.prefix(10)) { rec in
                        recordingRow(rec)
                    }
                }
            }
            .padding(24)
        }
        .task { await loadData() }
    }

    private func statCard(_ title: String, _ value: String, icon: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: icon)
                    .foregroundStyle(.tint)
                Text(title)
                    .foregroundStyle(.secondary)
            }
            .font(.callout)
            Text(value)
                .font(.system(size: 28, weight: .bold, design: .rounded))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private func recordingRow(_ rec: Recording) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 4) {
                Text(rec.polished ?? rec.transcript ?? "—")
                    .lineLimit(1)
                    .font(.system(size: 13))
                HStack(spacing: 6) {
                    Text(rec.formattedDate)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                    if let app = rec.target_app {
                        Text(app)
                            .font(.system(size: 11))
                            .foregroundStyle(.tertiary)
                    }
                }
            }
            Spacer()
            if let wc = rec.word_count {
                Text("\(wc) words")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 6)
    }

    private func loadData() async {
        while !sidecar.isHealthy {
            try? await Task.sleep(for: .milliseconds(300))
        }
        let client = BackendClient(sidecar: sidecar)
        do {
            let recs = try await client.getHistory(limit: 20)
            recordings = recs
            totalCount = recs.count
        } catch {
            print("[dashboard] load failed: \(error)")
        }
    }
}
