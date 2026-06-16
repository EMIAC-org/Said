import AirNoteShared
import SwiftUI
import UIKit

struct HistoryScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var search = ""

    private var grouped: [(day: Date, items: [RuntimeHistoryItem])] {
        let calendar = Calendar.current
        let filtered = env.history.filter { item in
            search.isEmpty || item.displayText.localizedCaseInsensitiveContains(search)
                || item.transcriptText.localizedCaseInsensitiveContains(search)
        }
        let dict = Dictionary(grouping: filtered) { calendar.startOfDay(for: $0.createdAt) }
        return dict.keys.sorted(by: >).map { ($0, (dict[$0] ?? []).sorted { $0.createdAt > $1.createdAt }) }
    }

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                content
            }
            .navigationTitle("History")
            .navigationBarTitleDisplayMode(.large)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { Task { await env.refreshHistory() } } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .disabled(env.historyLoading)
                }
            }
            .task { await env.refreshHistory() }
        }
        .searchable(text: $search, prompt: "Search dictations")
        .sheet(item: Binding(get: { env.learningItem }, set: { if $0 == nil { env.cancelLearningReview() } })) { _ in
            NavigationStack { LearningReviewSheet() }
        }
    }

    @ViewBuilder
    private var content: some View {
        if env.historyLoading && env.history.isEmpty {
            InlineLoading(text: "Loading your dictations…")
        } else if env.history.isEmpty {
            ScrollView {
                EmptyStateCard(
                    systemImage: "clock.arrow.circlepath",
                    title: "No dictations yet",
                    message: "Everything you dictate — in the app or the keyboard — shows up here."
                )
                .padding(.top, 60)
            }
        } else if grouped.isEmpty {
            ScrollView {
                EmptyStateCard(systemImage: "magnifyingglass", title: "No matches", message: "Try a different search.")
                    .padding(.top, 60)
            }
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 18) {
                    ForEach(grouped, id: \.day) { group in
                        VStack(alignment: .leading, spacing: 10) {
                            Text(dayLabel(group.day))
                                .font(.caption.weight(.bold))
                                .foregroundStyle(AirNoteDesign.muted)
                                .textCase(.uppercase)
                                .padding(.horizontal, 4)
                            ForEach(group.items) { item in
                                HistoryCard(item: item)
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 28)
            }
            .refreshable { await env.refreshHistory() }
        }
    }

    private func dayLabel(_ date: Date) -> String {
        let calendar = Calendar.current
        if calendar.isDateInToday(date) { return "Today" }
        if calendar.isDateInYesterday(date) { return "Yesterday" }
        let formatter = DateFormatter()
        formatter.dateFormat = "EEEE, MMM d"
        return formatter.string(from: date)
    }
}

private struct HistoryCard: View {
    @EnvironmentObject private var env: AppEnvironment
    var item: RuntimeHistoryItem
    @State private var expanded = false
    @State private var copied = false

    /// Long Hinglish dictations are collapsed behind "Read more" so the timeline
    /// stays scannable — matches the desktop's 50-word truncation.
    private static let truncateWords = 50

    private var wordCount: Int {
        item.displayText.split { $0 == " " || $0 == "\t" || $0 == "\n" }.count
    }

    private var isLong: Bool { wordCount > Self.truncateWords }

    private var shownText: String {
        guard isLong, !expanded else { return item.displayText }
        // Cut at the end of the Nth word in the ORIGINAL string so newlines/tabs
        // and runs of spaces survive, then strip trailing separators/punctuation
        // before the ellipsis so we never render "word,…" or a doubled "…".
        let s = item.displayText
        var seen = 0, inWord = false
        var cut = s.endIndex
        for idx in s.indices {
            let isSep = s[idx] == " " || s[idx] == "\t" || s[idx] == "\n"
            if !isSep, !inWord {
                inWord = true; seen += 1
            } else if isSep, inWord {
                inWord = false
                if seen >= Self.truncateWords { cut = idx; break }
            }
        }
        var prefix = String(s[s.startIndex..<cut])
        while let last = prefix.last, last.isWhitespace || last.isPunctuation { prefix.removeLast() }
        return prefix + " …"
    }

    private var hasHeard: Bool {
        !item.transcriptText.isEmpty && item.transcriptText != item.displayText
    }

    var body: some View {
        AirNoteCard(padding: 14) {
            VStack(alignment: .leading, spacing: 10) {
                Text(shownText)
                    .font(.subheadline)
                    .foregroundStyle(AirNoteDesign.foreground)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                if isLong {
                    Button(expanded ? "Show less" : "Read more") { expanded.toggle() }
                        .font(.caption.weight(.bold))
                        .buttonStyle(.plain)
                        .foregroundStyle(AirNoteDesign.accent)
                }
                if hasHeard {
                    Text("Heard: \(item.transcriptText)")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                        .lineLimit(expanded ? nil : 2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                HStack(spacing: 10) {
                    Text(item.createdAt, style: .time)
                        .font(.caption2)
                        .foregroundStyle(AirNoteDesign.muted)
                    Text("·").font(.caption2).foregroundStyle(AirNoteDesign.muted)
                    Text("\(wordCount) word\(wordCount == 1 ? "" : "s")")
                        .font(.caption2)
                        .foregroundStyle(AirNoteDesign.muted)
                        .monospacedDigit()
                    Spacer()
                    Button {
                        UIPasteboard.general.string = item.displayText
                        copied = true
                        Task { try? await Task.sleep(nanoseconds: 1_500_000_000); copied = false }
                    } label: {
                        Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(copied ? AirNoteDesign.success : AirNoteDesign.accent)
                    .accessibilityLabel("Copy text")
                    Button {
                        env.startLearningReview(item)
                    } label: {
                        Label("Teach", systemImage: "checkmark.seal")
                            .labelStyle(.titleAndIcon)
                            .font(.caption.weight(.bold))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(AirNoteDesign.accent)
                    .accessibilityLabel("Teach a correction")
                    .accessibilityHint("Fix this dictation to teach AirNote the right spelling")
                    Menu {
                        ShareLink(item: item.displayText) {
                            Label("Share text", systemImage: "square.and.arrow.up")
                        }
                        Button {
                            UIPasteboard.general.string = item.displayText
                        } label: {
                            Label("Copy text", systemImage: "doc.on.doc")
                        }
                        if hasHeard {
                            Button {
                                UIPasteboard.general.string = item.transcriptText
                            } label: {
                                Label("Copy heard (STT)", systemImage: "waveform")
                            }
                        }
                        Divider()
                        Button(role: .destructive) {
                            Task { await env.deleteHistoryItem(item) }
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .menuStyle(.borderlessButton)
                    .foregroundStyle(AirNoteDesign.muted)
                    .accessibilityLabel("More options")
                }
                .font(.system(size: 15, weight: .semibold))
            }
        }
    }
}

/// Correct a dictation and teach AirNote the right spelling.
struct LearningReviewSheet: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.dismiss) private var dismiss
    @State private var draftText = ""

    private var learningStatusColor: Color {
        let status = env.learningStatus
        if status.hasPrefix("✓") { return AirNoteDesign.success }
        if status.hasPrefix("Could") || status.contains("cannot") || status.contains("too common") {
            return AirNoteDesign.warning
        }
        return AirNoteDesign.muted
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 16) {
                    AirNoteCard {
                        VStack(alignment: .leading, spacing: 12) {
                            if let item = env.learningItem {
                                Text("AirNote wrote")
                                    .font(.caption2.weight(.bold)).tracking(0.9)
                                    .foregroundStyle(AirNoteDesign.muted)
                                Text(item.displayText)
                                    .font(.subheadline)
                                    .foregroundStyle(AirNoteDesign.muted)
                                    .fixedSize(horizontal: false, vertical: true)
                                Divider().overlay(AirNoteDesign.border)
                            }
                            AirNoteSectionLabel(text: "Fix it — what should it have said?")
                            TextEditor(text: $draftText)
                                .frame(minHeight: 90)
                                .scrollContentBackground(.hidden)
                                .padding(8)
                                .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(AirNoteDesign.border, lineWidth: 1))
                            if let item = env.learningItem, !item.transcriptText.isEmpty, item.transcriptText != item.displayText {
                                Text("Heard: \(item.transcriptText)")
                                    .font(.caption)
                                    .foregroundStyle(AirNoteDesign.muted)
                            }
                            Text(env.learningStatus)
                                .font(.caption)
                                .foregroundStyle(learningStatusColor)
                        }
                    }

                    Button {
                        Task { await env.learnFromHistory(kept: draftText) }
                    } label: {
                        Label(env.learningWorking ? "Learning…" : "Learn this fix", systemImage: "checkmark.seal.fill")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(AirNotePrimaryButtonStyle())
                    .disabled(env.learningWorking || draftText.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding(18)
            }
        }
        .navigationTitle("Review edit")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Close") { dismiss() }
            }
        }
        .onAppear { draftText = env.learningDraftText }
    }
}
