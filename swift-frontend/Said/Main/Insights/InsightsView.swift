import SwiftUI

struct InsightsView: View {
    let sidecar: SidecarManager

    @State private var recordings: [Recording] = []

    private var totalWords: Int { recordings.reduce(0) { $0 + ($1.word_count ?? 0) } }
    private var totalSeconds: Double { recordings.reduce(0) { $0 + ($1.recording_seconds ?? 0) } }
    private var avgWPM: Int {
        guard totalSeconds > 0 else { return 0 }
        return Int(Double(totalWords) / (totalSeconds / 60))
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text("Insights")
                    .font(.title.bold())

                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 16) {
                    metricCard("Total Words", "\(totalWords)", icon: "text.word.spacing")
                    metricCard("Avg WPM", "\(avgWPM)", icon: "gauge.medium")
                    metricCard("Recordings", "\(recordings.count)", icon: "waveform")
                    metricCard("Time Saved", timeSaved, icon: "clock.arrow.circlepath")
                    metricCard("Recording Time", recordingTime, icon: "timer")
                }
            }
            .padding(24)
        }
        .task { await loadData() }
    }

    private var timeSaved: String {
        let minutes = Int(totalSeconds * 2.5 / 60)
        return "\(minutes) min"
    }

    private var recordingTime: String {
        let minutes = Int(totalSeconds / 60)
        let seconds = Int(totalSeconds) % 60
        return "\(minutes)m \(seconds)s"
    }

    private func metricCard(_ title: String, _ value: String, icon: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Image(systemName: icon).foregroundStyle(.tint)
                Text(title).foregroundStyle(.secondary)
            }
            .font(.callout)
            Text(value)
                .font(.system(size: 24, weight: .bold, design: .rounded))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private func loadData() async {
        while !sidecar.isHealthy {
            try? await Task.sleep(for: .milliseconds(300))
        }
        let client = BackendClient(sidecar: sidecar)
        recordings = (try? await client.getHistory(limit: 500)) ?? []
    }
}
