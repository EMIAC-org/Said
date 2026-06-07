import SwiftUI
import AirNoteShared

struct HomeView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                ScrollView {
                    VStack(spacing: 20) {
                        HeroHeader(
                            email: environment.account?.email,
                            runtime: environment.runtimeStatus
                        )

                        StartCard(statusText: environment.lastStatusMessage)

                        if !environment.dictationStore.records.isEmpty {
                            LastDictationCard(records: environment.dictationStore.records)
                        }

                        if !isReady {
                            SetupCard(setupState: environment.setupState)
                        }

                        ExploreGrid()
                    }
                    .padding(18)
                    .padding(.bottom, 24)
                }
            }
            .navigationTitle("")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    private var isReady: Bool {
        if case .ready = environment.setupState { return true }
        return false
    }
}

// MARK: - Hero header

private struct HeroHeader: View {
    var email: String?
    var runtime: String

    var body: some View {
        HStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(AirNoteDesign.accentGradient)
                .frame(width: 46, height: 46)
                .overlay(
                    Image(systemName: "waveform")
                        .font(.system(size: 20, weight: .bold))
                        .foregroundStyle(.white)
                )
                .shadow(color: AirNoteDesign.accent.opacity(0.4), radius: 12, x: 0, y: 6)

            VStack(alignment: .leading, spacing: 2) {
                Text("AirNote")
                    .font(.system(.title2, design: .rounded).weight(.bold))
                Text(email ?? "Voice, polished — anywhere you type")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            NavigationLink(destination: AccountSignInView()) {
                Image(systemName: email == nil ? "person.crop.circle.badge.plus" : "person.crop.circle.fill")
                    .font(.title2)
                    .foregroundStyle(AirNoteDesign.accent)
            }
        }
        .padding(.top, 8)
    }
}

// MARK: - Start (hero CTA)

private struct StartCard: View {
    var statusText: String

    var body: some View {
        NavigationLink(destination: RecordingSessionView()) {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    AirNoteStatusPill(systemImage: "checkmark.seal.fill", text: "Ready", color: .white)
                        .environment(\.colorScheme, .dark)
                    Spacer()
                    Text("Hinglish · Work")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.white.opacity(0.85))
                }

                VStack(alignment: .leading, spacing: 6) {
                    Text("Start dictating")
                        .font(.system(.title, design: .rounded).weight(.bold))
                        .foregroundStyle(.white)
                    Text("Speak naturally — AirNote writes it clearly.")
                        .font(.subheadline)
                        .foregroundStyle(.white.opacity(0.9))
                }

                HStack(spacing: 10) {
                    Image(systemName: "mic.fill")
                        .font(.headline)
                    Text("Open the voice session")
                        .font(.headline)
                    Spacer()
                    Image(systemName: "arrow.right")
                        .font(.headline)
                }
                .foregroundStyle(.white)
                .padding(.vertical, 14)
                .padding(.horizontal, 16)
                .background(Color.white.opacity(0.18), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(AirNoteDesign.accentGradient, in: RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous))
            .shadow(color: AirNoteDesign.accent.opacity(0.35), radius: 22, x: 0, y: 14)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Start dictating. \(statusText)")
    }
}

// MARK: - Last dictation

private struct LastDictationCard: View {
    var records: [DictationRecord]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            AirNoteSectionLabel(text: "Recent")
            ForEach(records.prefix(2)) { record in
                AirNoteCard(padding: 16) {
                    VStack(alignment: .leading, spacing: 10) {
                        Text(record.polished)
                            .font(.body)
                            .fixedSize(horizontal: false, vertical: true)
                        HStack {
                            AirNoteStatusPill(systemImage: "checkmark.circle.fill",
                                              text: record.outcome.rawValue.capitalized,
                                              color: AirNoteDesign.success)
                            Spacer()
                            Text(record.createdAt, style: .time)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Setup progress

private struct SetupCard: View {
    var setupState: SetupState

    var body: some View {
        NavigationLink(destination: WelcomeView()) {
            AirNoteCard(padding: 16) {
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        AirNoteSectionLabel(text: "Finish setup")
                        Spacer()
                        Text("\(doneCount)/4")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(AirNoteDesign.accent)
                    }
                    ProgressView(value: Double(doneCount), total: 4)
                        .tint(AirNoteDesign.accent)
                    Text(nextStep)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .buttonStyle(.plain)
    }

    private var doneCount: Int {
        switch setupState {
        case .notStarted, .blocked: return 0
        case .accountReady: return 1
        case .privacyAccepted: return 2
        case .micReady: return 3
        case .keyboardReady, .fullAccessReady: return 3
        case .ready: return 4
        }
    }

    private var nextStep: String {
        switch setupState {
        case .notStarted, .blocked: return "Create your account to begin"
        case .accountReady: return "Review privacy & cloud processing"
        case .privacyAccepted: return "Run the microphone health check"
        case .micReady: return "Enable the AirNote keyboard"
        case .keyboardReady: return "Turn on Full Access"
        case .fullAccessReady, .ready: return "You're ready to dictate"
        }
    }
}

// MARK: - Explore grid

private struct ExploreGrid: View {
    private let columns = [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            AirNoteSectionLabel(text: "Explore")
            LazyVGrid(columns: columns, spacing: 12) {
                NavTile(icon: "slider.horizontal.3", tint: AirNoteDesign.accent,
                        title: "Language & style", subtitle: "Auto · Hinglish · Work") { LanguageStyleView() }
                NavTile(icon: "clock.arrow.circlepath", tint: AirNoteDesign.teal,
                        title: "History", subtitle: "Copy, retry, share") { HistoryView() }
                NavTile(icon: "text.badge.plus", tint: AirNoteDesign.success,
                        title: "Vocabulary", subtitle: "Names & terms") { VocabularyView() }
                NavTile(icon: "keyboard.badge.ellipsis", tint: AirNoteDesign.accent2,
                        title: "Practice", subtitle: "Try a safe field") { FirstDictationView() }
                NavTile(icon: "gearshape.fill", tint: .secondary,
                        title: "Settings", subtitle: "Privacy & data") { AirNoteSettingsView() }
                NavTile(icon: "stethoscope", tint: AirNoteDesign.warning,
                        title: "Diagnostics", subtitle: "Gateway status") { DiagnosticsView() }
            }
        }
    }
}

private struct NavTile<Destination: View>: View {
    var icon: String
    var tint: Color
    var title: String
    var subtitle: String
    @ViewBuilder var destination: () -> Destination

    var body: some View {
        NavigationLink(destination: destination()) {
            VStack(alignment: .leading, spacing: 10) {
                ZStack {
                    Circle().fill(tint.opacity(0.15)).frame(width: 40, height: 40)
                    Image(systemName: icon)
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(tint)
                }
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.primary)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, minHeight: 108, alignment: .topLeading)
            .padding(16)
            .background(
                RoundedRectangle(cornerRadius: AirNoteDesign.tileRadius, style: .continuous)
                    .fill(Color(.secondarySystemBackground))
            )
            .overlay(
                RoundedRectangle(cornerRadius: AirNoteDesign.tileRadius, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.05), lineWidth: 1)
            )
            .shadow(color: AirNoteDesign.cardShadow, radius: 12, x: 0, y: 6)
        }
        .buttonStyle(.plain)
    }
}

#Preview("Home") {
    HomeView()
        .environmentObject(AppEnvironment())
}
