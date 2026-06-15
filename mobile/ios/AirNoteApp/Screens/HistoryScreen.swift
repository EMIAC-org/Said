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

    var body: some View {
        AirNoteCard(padding: 14) {
            VStack(alignment: .leading, spacing: 10) {
                Text(item.displayText)
                    .font(.subheadline)
                    .foregroundStyle(AirNoteDesign.foreground)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                if !item.transcriptText.isEmpty, item.transcriptText != item.displayText {
                    Text(item.transcriptText)
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                        .lineLimit(2)
                }
                HStack(spacing: 10) {
                    Text(item.createdAt, style: .time)
                        .font(.caption2)
                        .foregroundStyle(AirNoteDesign.muted)
                    Spacer()
                    Button {
                        UIPasteboard.general.string = item.displayText
                    } label: {
                        Image(systemName: "doc.on.doc")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(AirNoteDesign.accent)
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
                    Button(role: .destructive) {
                        Task { await env.deleteHistoryItem(item) }
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(AirNoteDesign.danger)
                }
                .font(.system(size: 15, weight: .semibold))
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// Correct a dictation and teach AirNote the right spelling.
struct LearningReviewSheet: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.dismiss) private var dismiss
    @State private var draftText = ""
    @State private var selectedIDs: Set<String> = []

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

                    if !env.learningCandidates.isEmpty {
                        AirNoteCard {
                            VStack(alignment: .leading, spacing: 10) {
                                HStack {
                                    AirNoteSectionLabel(text: "Teach AirNote")
                                    Spacer()
                                    Text("\(selectedIDs.count) of \(env.learningCandidates.count)")
                                        .font(.caption2.weight(.semibold))
                                        .foregroundStyle(AirNoteDesign.muted)
                                }
                                ForEach(env.learningCandidates) { candidate in
                                    Button { toggle(candidate.id) } label: {
                                        HStack(spacing: 10) {
                                            Image(systemName: selectedIDs.contains(candidate.id) ? "checkmark.circle.fill" : "circle")
                                                .foregroundStyle(selectedIDs.contains(candidate.id) ? AirNoteDesign.accent : AirNoteDesign.muted)
                                            VStack(alignment: .leading, spacing: 2) {
                                                Text(candidate.corrected.isEmpty ? "Correction" : candidate.corrected)
                                                    .font(.subheadline.weight(.semibold))
                                                    .foregroundStyle(AirNoteDesign.foreground)
                                                if !candidate.original.isEmpty {
                                                    Text("heard as “\(candidate.original)”")
                                                        .font(.caption)
                                                        .foregroundStyle(AirNoteDesign.muted)
                                                }
                                            }
                                            Spacer()
                                            AirNoteChip(text: candidate.termType.replacingOccurrences(of: "_", with: " "))
                                        }
                                        .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                    .padding(.vertical, 3)
                                }
                            }
                        }
                    }

                    HStack(spacing: 10) {
                        Button {
                            Task { await env.analyzeLearningEdit(kept: draftText) }
                        } label: {
                            Label(env.learningWorking ? "Analyzing…" : "Analyze", systemImage: "magnifyingglass")
                        }
                        .buttonStyle(AirNoteGhostButtonStyle())
                        .disabled(env.learningWorking || draftText.trimmingCharacters(in: .whitespaces).isEmpty)

                        Button {
                            Task { await env.confirmLearning(selectedIDs: selectedIDs) }
                        } label: {
                            Label("Learn", systemImage: "checkmark.seal.fill")
                        }
                        .buttonStyle(AirNotePrimaryButtonStyle())
                        .disabled(env.learningWorking || selectedIDs.isEmpty)
                    }
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
        .onChange(of: env.learningCandidates) { _, candidates in
            // Pre-select every safe candidate; the user can deselect any.
            selectedIDs = Set(candidates.filter(\.learnable).map(\.id))
        }
    }

    private func toggle(_ id: String) {
        if selectedIDs.contains(id) { selectedIDs.remove(id) } else { selectedIDs.insert(id) }
    }
}
