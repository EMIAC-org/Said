import AirNoteShared
import SwiftUI

struct SettingsScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @AppStorage("airnotePreferredAppearance") private var appearance = AirNoteAppearance.system.rawValue
    @State private var confirmSignOut = false
    @State private var sessionMinutes = SharedStore.sessionDurationMinutes

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                Form {
                    accountSection
                    if !env.personalMode { enterpriseSection }
                    workspaceSection
                    dictationSection
                    permissionsSection
                    appearanceSection
                    privacySection
                    helpSection
                    aboutSection
                }
                .scrollContentBackground(.hidden)
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.large)
            .tint(AirNoteDesign.accent)
            .task {
                env.permissions.refreshAll()
                sessionMinutes = SharedStore.sessionDurationMinutes
                if !env.settingsLoaded { await env.loadSettings() }
            }
        }
    }

    // MARK: Account

    private var accountSection: some View {
        Section {
            HStack(spacing: 12) {
                AccountAvatar(email: env.account?.email ?? "A", size: 44)
                VStack(alignment: .leading, spacing: 2) {
                    Text(env.account?.email ?? "Not signed in")
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(1)
                    Text((env.account?.licenseTier ?? "free").capitalized + " plan")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                }
            }
            .padding(.vertical, 4)
            Button(role: .destructive) { confirmSignOut = true } label: {
                Label("Sign out", systemImage: "rectangle.portrait.and.arrow.right")
            }
            .confirmationDialog("Sign out of AirNote?", isPresented: $confirmSignOut, titleVisibility: .visible) {
                Button("Sign out", role: .destructive) { env.signOut() }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("You'll need to sign in again to dictate. Your server history stays safe.")
            }
        } header: {
            Text("Account")
        }
    }

    // MARK: Workspace

    private var workspaceSection: some View {
        Section {
            NavigationLink(destination: WorkspaceSwitcherView()) {
                HStack {
                    Label("Active workspace", systemImage: "building.2")
                    Spacer()
                    Text(env.personalMode ? "Personal" : (env.activeOrg?.name ?? "Workspace"))
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                        .lineLimit(1).truncationMode(.tail)
                }
            }
            NavigationLink(destination: ServerConnectionView()) {
                HStack {
                    Label("Workspace server", systemImage: "server.rack")
                    Spacer()
                    Text(BuildConfig.gatewayBaseURL.host ?? "AirNote")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                        .lineLimit(1).truncationMode(.middle)
                }
            }
        } header: {
            Text("Workspace")
        } footer: {
            Text("Switch between your personal account and any enterprise workspace you belong to. The server is the control-plane the app and keyboard connect to.")
        }
    }

    // MARK: Enterprise

    private var enterpriseSection: some View {
        Section {
            NavigationLink(destination: MeetingsScreen()) {
                Label("Meetings", systemImage: "person.2.wave.2")
            }
            NavigationLink(destination: DivoScreen()) {
                Label("Divo — AI chat", systemImage: "sparkles")
            }
        } header: {
            Text("Enterprise")
        } footer: {
            Text("Meetings and Divo for \(env.activeOrg?.name ?? "your workspace").")
        }
    }

    // MARK: Help & sharing

    private var helpSection: some View {
        Section {
            if let inviteURL = URL(string: "https://airnote.emiactech.com") {
                ShareLink(item: inviteURL) {
                    Label("Invite a friend", systemImage: "person.badge.plus")
                }
            }
            if let bugURL = URL(string: BuildConfig.gatewayBaseURL.absoluteString + "/report-bug") {
                Link(destination: bugURL) {
                    Label("Report a bug", systemImage: "ladybug")
                }
            }
        } header: {
            Text("Help")
        }
    }

    // MARK: Dictation

    private var dictationSection: some View {
        Section {
            NavigationLink(destination: ProviderKeysView()) {
                HStack {
                    Label("Voice keys", systemImage: "key")
                    Spacer()
                    statusChip(env.dictationAvailable ? "On" : "Add keys",
                               color: env.dictationAvailable ? AirNoteDesign.success : AirNoteDesign.warning)
                }
            }
            Picker("Language", selection: languageBinding) {
                Text("Hinglish").tag("hinglish")
                Text("English").tag("english")
            }
            Picker("Tone", selection: toneBinding) {
                ForEach(AirNoteTone.all, id: \.key) { tone in
                    Text(tone.label).tag(tone.key)
                }
            }
            Toggle("Personalize from my corrections", isOn: learningBinding)
            Picker("Keyboard session", selection: $sessionMinutes) {
                Text("5 minutes").tag(5)
                Text("15 minutes").tag(15)
                Text("1 hour").tag(60)
                Text("Until I stop it").tag(-1)
            }
            .onChange(of: sessionMinutes) { _, value in
                SharedStore.sessionDurationMinutes = value
            }
        } header: {
            Text("Dictation")
        } footer: {
            Text("AirNote applies the names and corrections you teach it (in History → review an edit, or Vocabulary) to future dictations. The keyboard session is how long the mic stays warm in the background after you dictate — longer means fewer “Start session” taps, at no extra server cost (the mic only streams while you actually speak). Settings sync across your devices.")
        }
    }

    private var languageBinding: Binding<String> {
        Binding(get: { env.outputLanguage }, set: { value in Task { await env.setOutputLanguage(value) } })
    }
    private var toneBinding: Binding<String> {
        Binding(get: { env.tonePreset }, set: { value in Task { await env.setTonePreset(value) } })
    }
    private var learningBinding: Binding<Bool> {
        Binding(get: { env.learningEnabled }, set: { value in Task { await env.setLearningEnabled(value) } })
    }

    // MARK: Permissions

    private var permissionsSection: some View {
        Section {
            HStack {
                Label("Microphone", systemImage: "mic.fill")
                Spacer()
                switch env.permissions.micPermission {
                case .granted:
                    statusChip("On", color: AirNoteDesign.success)
                case .undetermined:
                    Button("Allow") { Task { await env.permissions.requestMic() } }
                        .font(.subheadline.weight(.semibold))
                case .denied:
                    Button("Open Settings") { env.permissions.openSettings() }
                        .font(.subheadline.weight(.semibold))
                }
            }
            NavigationLink(destination: KeyboardSetupGuide()) {
                HStack {
                    Label("AirNote Keyboard", systemImage: "keyboard")
                    Spacer()
                    switch env.permissions.keyboard {
                    case .ready: statusChip("Ready", color: AirNoteDesign.success)
                    case .needsFullAccess: statusChip("Full Access off", color: AirNoteDesign.warning)
                    case .unknown: statusChip("Set up", color: AirNoteDesign.muted)
                    }
                }
            }
        } header: {
            Text("Permissions")
        }
    }

    private func statusChip(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.caption.weight(.bold))
            .foregroundStyle(color)
    }

    // MARK: Appearance

    private var appearanceSection: some View {
        Section {
            AirNoteAppearancePicker()
        } header: {
            Text("Appearance")
        }
    }

    // MARK: Privacy

    private var privacySection: some View {
        Section {
            Label("Recordings are never stored", systemImage: "waveform.slash")
            Label("AirNote skips password & OTP fields", systemImage: "lock.fill")
            if let privacyURL = URL(string: "https://airnote.emiactech.com/privacy") {
                Link(destination: privacyURL) {
                    Label("Privacy Policy", systemImage: "hand.raised")
                }
            }
        } header: {
            Text("Privacy")
        } footer: {
            Text("Audio streams straight to transcription and is discarded — it is never saved on AirNote's servers.")
        }
    }

    // MARK: About

    private var aboutSection: some View {
        Section {
            LabeledContent("Version", value: "\(AppInfo.version) (\(AppInfo.build))")
            LabeledContent("Runtime", value: env.runtimeStatusLabel)
        } header: {
            Text("About")
        }
    }
}

/// Step-by-step guide to enable the AirNote keyboard + Full Access. Used from
/// the Dashboard nudge, Settings, and onboarding.
struct KeyboardSetupGuide: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 16) {
                    statusBanner
                    AirNoteCard {
                        VStack(alignment: .leading, spacing: 14) {
                            AirNoteSectionLabel(text: "Enable AirNote Keyboard")
                            StepRow(number: 1, title: "Open Settings", detail: "Tap the button below to jump to AirNote's settings.")
                            StepRow(number: 2, title: "Tap Keyboards", detail: "Go to Keyboards, then turn on “AirNote Keyboard”.")
                            StepRow(number: 3, title: "Allow Full Access", detail: "Turn on Allow Full Access — this lets AirNote send your speech to its servers for transcription. Audio is handled securely and is never stored.")
                            StepRow(number: 4, title: "Switch to it", detail: "In any app, tap the 🌐 globe key until you see AirNote.")
                        }
                    }
                    Button { env.permissions.openSettings() } label: {
                        Label("Open Settings", systemImage: "gearshape")
                    }
                    .buttonStyle(AirNotePrimaryButtonStyle())

                    Text("On iOS 26.4 and later, the first time you dictate from the keyboard you may be asked to swipe back to the app to start the microphone — that's a normal iOS step.")
                        .font(.caption2)
                        .foregroundStyle(AirNoteDesign.muted)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 8)
                }
                .padding(18)
            }
        }
        .navigationTitle("Keyboard")
        .navigationBarTitleDisplayMode(.inline)
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { env.permissions.refreshKeyboard() }
        }
    }

    @ViewBuilder
    private var statusBanner: some View {
        switch env.permissions.keyboard {
        case .ready:
            banner(icon: "checkmark.circle.fill", text: "AirNote Keyboard is enabled with Full Access.", color: AirNoteDesign.success)
        case .needsFullAccess:
            banner(icon: "exclamationmark.triangle.fill", text: "AirNote Keyboard is added — turn on Allow Full Access to dictate.", color: AirNoteDesign.warning)
        case .unknown:
            banner(icon: "keyboard", text: "Add AirNote Keyboard to dictate into any app.", color: AirNoteDesign.accent)
        }
    }

    private func banner(icon: String, text: String, color: Color) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon).foregroundStyle(color)
            Text(text).font(.subheadline).foregroundStyle(AirNoteDesign.foreground)
            Spacer(minLength: 0)
        }
        .padding(14)
        .background(color.opacity(0.12), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(color.opacity(0.3), lineWidth: 1))
    }
}

struct StepRow: View {
    var number: Int
    var title: String
    var detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(number)")
                .font(.subheadline.weight(.bold))
                .foregroundStyle(AirNoteDesign.accent)
                .frame(width: 26, height: 26)
                .background(AirNoteDesign.accent.opacity(0.12), in: Circle())
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
