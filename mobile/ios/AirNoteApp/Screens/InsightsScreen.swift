import AirNoteShared
import SwiftUI

struct InsightsScreen: View {
    @EnvironmentObject private var env: AppEnvironment

    private var stats: DictationStats { DictationStats(items: env.history, days: 28) }

    private var avgWords: Int {
        guard stats.count > 0 else { return 0 }
        return Int((Double(stats.totalWords) / Double(stats.count)).rounded())
    }

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                ScrollView {
                    VStack(spacing: 16) {
                        if env.history.isEmpty {
                            EmptyStateCard(
                                systemImage: "chart.bar.xaxis",
                                title: "No insights yet",
                                message: "Dictate a few times and your trends show up here."
                            )
                            .padding(.top, 40)
                        } else {
                            grid
                            activityCard
                            cadenceCard
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                    .padding(.bottom, 28)
                }
            }
            .navigationTitle("Insights")
            .navigationBarTitleDisplayMode(.large)
            .task { await env.refreshHistory() }
            .refreshable { await env.refreshHistory() }
        }
    }

    private var grid: some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                StatTile(value: "\(stats.count)", label: "dictations", systemImage: "waveform")
                StatTile(value: stats.totalWords.compactString, label: "total words", systemImage: "text.alignleft")
            }
            HStack(spacing: 10) {
                StatTile(value: "\(stats.streak)", label: "day streak", systemImage: "flame.fill", tint: AirNoteDesign.warning)
                StatTile(value: "\(avgWords)", label: "avg words", systemImage: "gauge.with.dots.needle.50percent")
            }
        }
    }

    private var activityCard: some View {
        AirNoteCard(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                SectionHeader("Last 28 days")
                ActivityChart(values: stats.activity)
                Text("\(stats.activity.filter { $0 > 0 }.count) active days")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
            }
        }
    }

    private var cadenceCard: some View {
        AirNoteCard(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                SectionHeader("When you dictate")
                WeekdayBars(items: env.history)
            }
        }
    }
}

/// Distribution of dictations across days of the week.
private struct WeekdayBars: View {
    var items: [RuntimeHistoryItem]

    private var counts: [Int] {
        var buckets = Array(repeating: 0, count: 7)
        let calendar = Calendar.current
        for item in items {
            let weekday = calendar.component(.weekday, from: item.createdAt) // 1 = Sunday
            buckets[(weekday - 1) % 7] += 1
        }
        return buckets
    }

    private let labels = ["S", "M", "T", "W", "T", "F", "S"]

    var body: some View {
        let values = counts
        let maxValue = max(values.max() ?? 0, 1)
        HStack(alignment: .bottom, spacing: 8) {
            ForEach(0..<7, id: \.self) { index in
                VStack(spacing: 6) {
                    Text("\(values[index])")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(AirNoteDesign.muted)
                    RoundedRectangle(cornerRadius: 4, style: .continuous)
                        .fill(values[index] > 0 ? AirNoteDesign.accent.opacity(0.9) : AirNoteDesign.surfaceRaised)
                        .frame(height: max(6, CGFloat(values[index]) / CGFloat(maxValue) * 70))
                        .frame(maxWidth: .infinity)
                    Text(labels[index])
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(AirNoteDesign.muted)
                }
            }
        }
        .frame(height: 110)
    }
}
