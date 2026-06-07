import SwiftUI
import AirNoteShared

struct HomeView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    SectionHeader(title: "Account")
                    NavigationLink(destination: AccountSignInView()) {
                        SettingsRow(
                            systemImage: environment.account == nil ? "person.crop.circle.badge.plus" : "person.crop.circle.badge.checkmark",
                            title: environment.account?.email ?? "Sign in to AirNote Mobile",
                            subtitle: environment.account == nil ? "Use the independent mobile gateway account for iPhone dictation" : "Runtime: \(environment.runtimeStatus)"
                        )
                    }

                    ReadinessPanel(
                        title: environment.lastStatusMessage,
                        sessionState: environment.sessionState
                    ) {
                        environment.sessionState = .ready
                    }

                    SectionHeader(title: "Today")
                    LastDictationPanel(records: environment.dictationStore.records)

                    SectionHeader(title: "Setup")
                    SetupChecklist(setupState: environment.setupState)

                    SectionHeader(title: "Shortcuts")
                    VStack(spacing: 10) {
                        NavigationLink(destination: LanguageStyleView()) {
                            SettingsRow(systemImage: "slider.horizontal.3", title: "Language & style", subtitle: "Auto, English, Hindi, Hinglish and Direct, Work, Casual, Email, Notes")
                        }
                        NavigationLink(destination: WelcomeView()) {
                            SettingsRow(systemImage: "checklist", title: "Run onboarding check", subtitle: "Account, privacy, mic, keyboard, and Full Access")
                        }
                        NavigationLink(destination: FirstDictationView()) {
                            SettingsRow(systemImage: "keyboard.badge.ellipsis", title: "Practice first dictation", subtitle: "Use a safe in-app field before testing host apps")
                        }
                        NavigationLink(destination: RecordingSessionView()) {
                            SettingsRow(systemImage: "waveform", title: "Live session screen", subtitle: "The screen users return to when the keyboard asks for AirNote")
                        }
                        NavigationLink(destination: HistoryView()) {
                            SettingsRow(systemImage: "clock.arrow.circlepath", title: "History", subtitle: "Copy, retry, share, or delete previous dictations")
                        }
                        NavigationLink(destination: VocabularyView()) {
                            SettingsRow(systemImage: "text.badge.plus", title: "Vocabulary", subtitle: "Add terms, aliases, and learn-spelling review")
                        }
                        NavigationLink(destination: AirNoteSettingsView()) {
                            SettingsRow(systemImage: "gearshape", title: "Settings", subtitle: "Privacy, diagnostics, account, and delete data")
                        }
                        NavigationLink(destination: DiagnosticsView()) {
                            SettingsRow(systemImage: "stethoscope", title: "Diagnostics", subtitle: "Gateway status, build, last session, and redacted export")
                        }
                    }
                }
                .padding(16)
            }
            .navigationTitle("AirNote")
            .background(Color(.systemGroupedBackground))
        }
    }
}

private struct ReadinessPanel: View {
    var title: String
    var sessionState: SessionState
    var action: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                AirNoteStatusPill(systemImage: statusIcon, text: statusText, color: statusColor)
                Spacer()
                Text("Hinglish - Work")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Text(title)
                .font(.title2.weight(.semibold))
                .fixedSize(horizontal: false, vertical: true)

            Text("Start a visible AirNote Session, switch back to your app, then use AirNote Keyboard to record, preview, insert, copy, or save.")
                .font(.subheadline)
                .foregroundStyle(.secondary)

            AirNoteActionRow(
                primaryTitle: "Start session",
                primarySystemImage: "play.fill",
                secondaryTitle: "Repair setup",
                secondarySystemImage: "wrench.and.screwdriver",
                primaryAction: action,
                secondaryAction: {}
            )
        }
        .padding(14)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
    }

    private var statusIcon: String {
        switch sessionState {
        case .ready: return "checkmark.circle.fill"
        case .recording: return "mic.fill"
        case .processing: return "bolt.horizontal.fill"
        case .retryableError, .stale: return "exclamationmark.triangle.fill"
        default: return "keyboard"
        }
    }

    private var statusText: String {
        switch sessionState {
        case .ready: return "Ready"
        case .recording: return "Listening"
        case .processing: return "Processing"
        case .insertReady: return "Insert ready"
        case .inserted: return "Inserted"
        case .savedToHistory: return "Saved"
        case .retryableError: return "Repair"
        case .stale: return "Session stale"
        default: return "Setup"
        }
    }

    private var statusColor: Color {
        switch sessionState {
        case .ready, .recording, .processing: return AirNoteDesign.accent
        case .inserted, .savedToHistory: return AirNoteDesign.success
        case .retryableError, .stale: return AirNoteDesign.warning
        default: return AirNoteDesign.teal
        }
    }
}

private struct LastDictationPanel: View {
    var records: [DictationRecord]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if records.isEmpty {
                SettingsRow(
                    systemImage: "tray",
                    title: "No dictations yet",
                    subtitle: "The first inserted, copied, or saved recovery result will appear here."
                )
            } else {
                ForEach(records.prefix(3)) { record in
                    VStack(alignment: .leading, spacing: 6) {
                        Text(record.polished)
                            .font(.body)
                            .fixedSize(horizontal: false, vertical: true)
                        AirNoteStatusPill(systemImage: "clock", text: record.outcome.rawValue, color: AirNoteDesign.success)
                    }
                    .padding(12)
                    .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
                }
            }
        }
    }
}

private struct SetupChecklist: View {
    var setupState: SetupState

    var body: some View {
        VStack(spacing: 10) {
            SetupStep(title: "Privacy consent", isDone: isAtLeastPrivacy)
            SetupStep(title: "Mic health check", isDone: isAtLeastMic)
            SetupStep(title: "Keyboard enabled", isDone: isAtLeastKeyboard)
            SetupStep(title: "Full Access verified", isDone: isReady)
        }
        .padding(12)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
    }

    private var isAtLeastPrivacy: Bool {
        switch setupState {
        case .privacyAccepted, .micReady, .keyboardReady, .fullAccessReady, .ready: return true
        default: return false
        }
    }

    private var isAtLeastMic: Bool {
        switch setupState {
        case .micReady, .keyboardReady, .fullAccessReady, .ready: return true
        default: return false
        }
    }

    private var isAtLeastKeyboard: Bool {
        switch setupState {
        case .keyboardReady, .fullAccessReady, .ready: return true
        default: return false
        }
    }

    private var isReady: Bool {
        switch setupState {
        case .fullAccessReady, .ready: return true
        default: return false
        }
    }
}

private struct SetupStep: View {
    var title: String
    var isDone: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: isDone ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(isDone ? AirNoteDesign.success : Color.secondary)
            Text(title)
                .font(.subheadline)
            Spacer()
        }
        .accessibilityElement(children: .combine)
    }
}

private struct SettingsRow: View {
    var systemImage: String
    var title: String
    var subtitle: String

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: systemImage)
                .font(.body.weight(.semibold))
                .foregroundStyle(AirNoteDesign.accent)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.primary)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
    }
}

private struct SectionHeader: View {
    var title: String

    var body: some View {
        Text(title)
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.secondary)
            .textCase(.uppercase)
    }
}
