import AirNoteShared
import SwiftUI

struct DashboardScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var showDictation = false

    private var stats: DictationStats { DictationStats(items: env.history) }

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                ScrollView {
                    VStack(spacing: 16) {
                        header
                        recordCard
                        statsGrid
                        if !stats.activity.isEmpty {
                            activityCard
                        }
                        if env.permissions.keyboard != .ready {
                            keyboardNudge
                        }
                        recentCard
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                    .padding(.bottom, 28)
                }
            }
            .navigationTitle("Home")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .navigationBar)
            .task { await env.refreshHistory() }
            .refreshable { await env.refreshHistory() }
        }
        .sheet(isPresented: $showDictation) {
            NavigationStack { DictationSheet(env: env, showsDoneButton: false) }
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            AirNoteLogoTile(size: 42)
            VStack(alignment: .leading, spacing: 2) {
                Text(greeting)
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text(env.account?.email ?? "AirNote")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .lineLimit(1)
            }
            Spacer()
            AirNoteStatusPill(
                systemImage: env.dictationAvailable ? "bolt.fill" : "clock.fill",
                text: env.dictationAvailable ? "Ready" : "Setup",
                color: env.dictationAvailable ? AirNoteDesign.success : AirNoteDesign.warning
            )
        }
    }

    private var recordCard: some View {
        AirNoteCard(padding: 18) {
            VStack(alignment: .leading, spacing: 14) {
                AirNoteSectionLabel(text: "Dictate")
                Text("Speak, and AirNote writes it cleanly")
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                    .fixedSize(horizontal: false, vertical: true)
                Text(env.dictationAvailable
                     ? "English, Hindi, or Hinglish — polished on AirNote's servers."
                     : "Dictation turns on automatically once your workspace finishes setup.")
                    .font(.subheadline)
                    .foregroundStyle(AirNoteDesign.muted)
                Button {
                    showDictation = true
                } label: {
                    Label("Start dictation", systemImage: "mic.fill")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
            }
        }
    }

    private var statsGrid: some View {
        HStack(spacing: 10) {
            StatTile(value: "\(stats.streak)", label: stats.streak == 1 ? "day streak" : "day streak", systemImage: "flame.fill", tint: AirNoteDesign.warning)
            StatTile(value: stats.totalWords.compactString, label: "words", systemImage: "text.alignleft")
            StatTile(value: "\(stats.count)", label: "dictations", systemImage: "waveform")
        }
    }

    private var activityCard: some View {
        AirNoteCard(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                SectionHeader("Last 14 days")
                ActivityChart(values: stats.activity, labels: stats.activityLabels)
            }
        }
    }

    private var keyboardNudge: some View {
        NavigationLink(destination: KeyboardSetupGuide()) {
            HStack(spacing: 12) {
                Image(systemName: "keyboard.badge.ellipsis")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(AirNoteDesign.accent)
                    .frame(width: 34, height: 34)
                    .background(AirNoteDesign.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text("Finish the AirNote Keyboard")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                    Text("Dictate into any app — Messages, Mail, Slack.")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                }
                Spacer()
                Image(systemName: "chevron.right").font(.caption.weight(.bold)).foregroundStyle(AirNoteDesign.muted)
            }
            .padding(14)
            .background(AirNoteDesign.surface.opacity(0.92), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(AirNoteDesign.accent.opacity(0.3), lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private var recentCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeader("Recent")
            AirNoteCard(padding: 14) {
                if env.historyLoading && env.history.isEmpty {
                    InlineLoading(text: "Loading your dictations…")
                } else if env.history.isEmpty {
                    EmptyStateCard(
                        systemImage: "waveform",
                        title: "No dictations yet",
                        message: "Your polished dictations appear here."
                    )
                } else {
                    VStack(spacing: 12) {
                        ForEach(env.history.prefix(3)) { item in
                            DictationRow(item: item)
                            if item.id != env.history.prefix(3).last?.id {
                                Divider().overlay(AirNoteDesign.border)
                            }
                        }
                    }
                }
            }
        }
    }

    private var greeting: String {
        let hour = Calendar.current.component(.hour, from: Date())
        switch hour {
        case 5..<12: return "Good morning"
        case 12..<17: return "Good afternoon"
        case 17..<22: return "Good evening"
        default: return "Hello"
        }
    }
}

struct DictationRow: View {
    var item: RuntimeHistoryItem

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(item.displayText)
                .font(.subheadline)
                .foregroundStyle(AirNoteDesign.foreground)
                .lineLimit(3)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Text(item.createdAt, format: .relative(presentation: .named))
                    .font(.caption2)
                    .foregroundStyle(AirNoteDesign.muted)
                Spacer()
                Text("\(item.displayText.wordCount) words")
                    .font(.caption2.weight(.medium))
                    .foregroundStyle(AirNoteDesign.muted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Stats

struct DictationStats {
    let count: Int
    let totalWords: Int
    let streak: Int
    let activity: [Int]
    let activityLabels: [String]

    init(items: [RuntimeHistoryItem], days: Int = 14, calendar: Calendar = .current, now: Date = Date()) {
        count = items.count
        totalWords = items.reduce(0) { $0 + $1.displayText.wordCount }

        let today = calendar.startOfDay(for: now)
        let activeDays: Set<Int> = Set(items.compactMap { item in
            calendar.dateComponents([.day], from: calendar.startOfDay(for: item.createdAt), to: today).day
        })

        // Streak: consecutive days back from today (or yesterday) with activity.
        var streakCount = 0
        var dayOffset = activeDays.contains(0) ? 0 : (activeDays.contains(1) ? 1 : -1)
        if dayOffset >= 0 {
            while activeDays.contains(dayOffset) {
                streakCount += 1
                dayOffset += 1
            }
        }
        streak = streakCount

        // Activity: one bucket per day, oldest → newest.
        var buckets = Array(repeating: 0, count: days)
        for item in items {
            if let delta = calendar.dateComponents([.day], from: calendar.startOfDay(for: item.createdAt), to: today).day,
               delta >= 0, delta < days {
                buckets[days - 1 - delta] += 1
            }
        }
        activity = buckets

        let formatter = DateFormatter()
        formatter.dateFormat = "EEEEE"
        activityLabels = (0..<days).map { index in
            let offset = days - 1 - index
            let date = calendar.date(byAdding: .day, value: -offset, to: today) ?? today
            return formatter.string(from: date)
        }
    }
}

extension String {
    var wordCount: Int {
        split { $0 == " " || $0 == "\n" || $0 == "\t" }.filter { !$0.isEmpty }.count
    }
}

extension Int {
    /// Compact display: 1234 → "1.2k".
    var compactString: String {
        if self >= 1_000_000 { return String(format: "%.1fM", Double(self) / 1_000_000) }
        if self >= 1_000 { return String(format: "%.1fk", Double(self) / 1_000) }
        return "\(self)"
    }
}
