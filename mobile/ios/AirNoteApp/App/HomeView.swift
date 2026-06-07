import AirNoteShared
import SwiftUI

struct HomeView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                ScrollView {
                    VStack(spacing: 14) {
                        AppHeader(email: environment.account?.email,
                                  runtime: environment.runtimeStatus,
                                  onReset: environment.resetMockSetup)

                        SessionPanel(statusText: environment.lastStatusMessage)

                        SetupSummary(setupState: environment.setupState,
                                     onReset: environment.resetMockSetup)

                        if !environment.dictationStore.records.isEmpty {
                            RecentDictations(records: environment.dictationStore.records)
                        }

                        ExploreList()
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 18)
                    .padding(.bottom, 28)
                }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
        .preferredColorScheme(.dark)
    }
}

private struct AppHeader: View {
    var email: String?
    var runtime: String
    var onReset: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            AirNoteLogoTile(size: 44)
            VStack(alignment: .leading, spacing: 2) {
                Text("AirNote")
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text(email ?? "Voice Polish Studio")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .lineLimit(1)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 6) {
                AirNoteStatusPill(systemImage: "bolt.fill", text: runtime == "Preview" ? "Preview" : "Live")
                Button(action: onReset) {
                    Text("Replay setup")
                        .font(.caption2.weight(.bold))
                }
                .foregroundStyle(AirNoteDesign.muted)
            }
        }
    }
}

private struct SessionPanel: View {
    var statusText: String

    var body: some View {
        AirNoteCard(padding: 18) {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    VStack(alignment: .leading, spacing: 5) {
                        AirNoteSectionLabel(text: "Dashboard")
                        Text("Ready to dictate")
                            .font(.system(size: 28, weight: .semibold))
                            .foregroundStyle(AirNoteDesign.foreground)
                        Text(statusText)
                            .font(.subheadline)
                            .foregroundStyle(AirNoteDesign.muted)
                    }
                    Spacer()
                    AirNoteStatusPill(systemImage: "checkmark.circle.fill",
                                      text: "Ready",
                                      color: AirNoteDesign.success)
                }

                HStack(spacing: 10) {
                    MiniStat(title: "Runtime", value: "Preview")
                    MiniStat(title: "Style", value: "Work")
                    MiniStat(title: "Lang", value: "Hinglish")
                }

                NavigationLink(destination: RecordingSessionView()) {
                    Label("Open voice session", systemImage: "mic.fill")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
            }
        }
    }
}

private struct MiniStat: View {
    var title: String
    var value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption2.weight(.bold))
                .foregroundStyle(AirNoteDesign.muted)
            Text(value)
                .font(.caption.weight(.semibold))
                .foregroundStyle(AirNoteDesign.foreground)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(AirNoteDesign.border, lineWidth: 1)
        )
    }
}

private struct SetupSummary: View {
    var setupState: SetupState
    var onReset: () -> Void

    var body: some View {
        AirNoteCard(padding: 14) {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    AirNoteSectionLabel(text: "Setup")
                    Spacer()
                    Text("4/4")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(AirNoteDesign.accent)
                }
                AirNoteSetupRow(icon: "person.crop.circle", title: "Account", subtitle: "Mobile account ready.", status: "Done")
                AirNoteSetupRow(icon: "mic.fill", title: "Microphone", subtitle: "Health check completed.", status: "Done")
                AirNoteSetupRow(icon: "keyboard", title: "Keyboard", subtitle: "Full Access completed.", status: "Done")
                Button(action: onReset) {
                    Label("Run setup flow from the beginning", systemImage: "arrow.counterclockwise")
                }
                .buttonStyle(AirNoteGhostButtonStyle())
            }
        }
    }
}

private struct RecentDictations: View {
    var records: [DictationRecord]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            AirNoteSectionLabel(text: "Recent")
            ForEach(records.prefix(2)) { record in
                AirNoteCard(padding: 14) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(record.polished)
                            .font(.subheadline.weight(.medium))
                            .foregroundStyle(AirNoteDesign.foreground)
                            .fixedSize(horizontal: false, vertical: true)
                        HStack {
                            AirNoteStatusPill(systemImage: "checkmark.circle.fill",
                                              text: record.outcome.rawValue.capitalized,
                                              color: AirNoteDesign.success)
                            Spacer()
                            Text(record.createdAt, style: .time)
                                .font(.caption)
                                .foregroundStyle(AirNoteDesign.muted)
                        }
                    }
                }
            }
        }
    }
}

private struct ExploreList: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            AirNoteSectionLabel(text: "Tools")
            VStack(spacing: 8) {
                NavRow(icon: "slider.horizontal.3", title: "Language and style", subtitle: "Auto, Hinglish, Work", destination: LanguageStyleView())
                NavRow(icon: "clock.arrow.circlepath", title: "History", subtitle: "Copy, retry, and recover", destination: HistoryView())
                NavRow(icon: "text.badge.plus", title: "Vocabulary", subtitle: "Names, aliases, and terms", destination: VocabularyView())
                NavRow(icon: "keyboard.badge.ellipsis", title: "Practice field", subtitle: "Try a safe dictation", destination: FirstDictationView())
                NavRow(icon: "gearshape.fill", title: "Settings", subtitle: "Privacy, Gateway, diagnostics", destination: AirNoteSettingsView())
            }
        }
    }
}

private struct NavRow<Destination: View>: View {
    var icon: String
    var title: String
    var subtitle: String
    var destination: Destination

    var body: some View {
        NavigationLink(destination: destination) {
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.white.opacity(0.045))
                    .frame(width: 34, height: 34)
                    .overlay(Image(systemName: icon).font(.system(size: 14, weight: .semibold)).foregroundStyle(AirNoteDesign.accent))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .strokeBorder(AirNoteDesign.border, lineWidth: 1)
                    )
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                }
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(AirNoteDesign.muted)
            }
            .padding(12)
            .background(AirNoteDesign.surface.opacity(0.92), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(AirNoteDesign.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}

#Preview("Home") {
    HomeView()
        .environmentObject(AppEnvironment())
}
