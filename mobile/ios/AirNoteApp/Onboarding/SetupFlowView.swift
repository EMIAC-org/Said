import AirNoteShared
import SwiftUI

enum MockSetupStep: Int, CaseIterable, Hashable {
    case welcome
    case account
    case privacy
    case microphone
    case keyboard
    case preview

    var eyebrow: String {
        switch self {
        case .welcome: return "Get started"
        case .account: return "Account"
        case .privacy: return "Privacy"
        case .microphone: return "Microphone"
        case .keyboard: return "Keyboard"
        case .preview: return "Keyboard preview"
        }
    }

    var title: String {
        switch self {
        case .welcome: return "Set up AirNote"
        case .account: return "Account"
        case .privacy: return "Privacy"
        case .microphone: return "Microphone"
        case .keyboard: return "Keyboard"
        case .preview: return "Keyboard preview"
        }
    }

    var subtitle: String {
        switch self {
        case .welcome:
            return "Account, privacy, microphone, and keyboard in one guided pass."
        case .account:
            return BuildConfig.useMockGateway ? "Use the local preview profile for this build." : "Connect your AirNote workspace before recording."
        case .privacy:
            return "Review storage and recovery defaults before recording."
        case .microphone:
            return "Confirm the recording surface is ready."
        case .keyboard:
            return "Prepare AirNote Keyboard and Full Access."
        case .preview:
            return "Run the keyboard states before leaving setup."
        }
    }
}

struct SetupFlowView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @Environment(\.openURL) private var openURL
    @State private var step: MockSetupStep
    @State private var privacyAccepted = false
    @State private var micChecked = false
    @State private var fullAccessMock = false
    @State private var keyboardPreviewState: KeyboardPreviewState = .ready
    @State private var email = ""
    @State private var password = ""
    @State private var signup = false
    @State private var authWorking = false

    init(initialStep: MockSetupStep? = nil) {
        _step = State(initialValue: initialStep ?? MockSetupStep.debugLaunchStep ?? .welcome)
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 16) {
                    header
                    progressRail

                    AirNoteCard(padding: 18) {
                        VStack(alignment: .leading, spacing: 16) {
                            AirNoteSectionLabel(text: step.eyebrow)
                            VStack(alignment: .leading, spacing: 8) {
                                Text(step.title)
                                    .font(.system(size: 28, weight: .semibold))
                                    .foregroundStyle(AirNoteDesign.foreground)
                                    .lineLimit(2)
                                    .fixedSize(horizontal: false, vertical: true)
                                Text(step.subtitle)
                                    .font(.subheadline)
                                    .foregroundStyle(AirNoteDesign.muted)
                                    .lineSpacing(2)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            stepContent
                        }
                    }

                    footerActions
                }
                .padding(.horizontal, 16)
                .padding(.top, 18)
                .padding(.bottom, 28)
            }
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            AirNoteLogoTile(size: 42)
            VStack(alignment: .leading, spacing: 2) {
                Text("AirNote")
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text("Voice Polish Studio")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 6) {
                AirNoteStatusPill(systemImage: "bolt.fill", text: BuildConfig.useMockGateway ? "Preview" : "Live")
            }
        }
    }

    private var progressRail: some View {
        HStack(spacing: 6) {
            ForEach(MockSetupStep.allCases, id: \.self) { item in
                Capsule()
                    .fill(item.rawValue <= step.rawValue ? AirNoteDesign.foreground : AirNoteDesign.surfaceHover)
                    .frame(height: 4)
            }
        }
        .accessibilityLabel("Setup step \(step.rawValue + 1) of \(MockSetupStep.allCases.count)")
    }

    @ViewBuilder
    private var stepContent: some View {
        switch step {
        case .welcome:
            VStack(spacing: 10) {
                AirNoteSetupRow(icon: "person.crop.circle.badge.checkmark", title: "Workspace", subtitle: "AirNote account, Lark identity, and mobile runtime.", status: "Ready")
                AirNoteSetupRow(icon: "mic.fill", title: "Microphone", subtitle: "Recording surface and route check.", status: "Ready")
                AirNoteSetupRow(icon: "keyboard", title: "Keyboard", subtitle: "Insert, copy, save, and recover.", status: "Ready")
            }

        case .account:
            VStack(spacing: 10) {
                AirNoteSetupRow(icon: "person.crop.circle.badge.checkmark", title: BuildConfig.useMockGateway ? "Preview workspace" : "AirNote workspace", subtitle: environment.account?.email ?? "Sign in with your AirNote or Lark workspace account.", status: environment.account == nil ? "Required" : "Signed")
                AirNoteSetupRow(icon: "server.rack", title: "Runtime Gateway", subtitle: "Same control-plane runtime contract as desktop.", status: environment.runtimeStatus)
                if !BuildConfig.useMockGateway && environment.account == nil {
                    VStack(spacing: 10) {
                        TextField("Email", text: $email)
                            .textInputAutocapitalization(.never)
                            .keyboardType(.emailAddress)
                            .textContentType(.emailAddress)
                        SecureField("Password", text: $password)
                            .textContentType(signup ? .newPassword : .password)
                        Toggle(isOn: $signup) {
                            Text("Create new account")
                                .font(.subheadline.weight(.semibold))
                                .foregroundStyle(AirNoteDesign.foreground)
                        }
                        .tint(AirNoteDesign.accent)
                        Button {
                            openURL(BuildConfig.gatewayBaseURL.appendingPathComponent("auth/lark"))
                        } label: {
                            Label("Continue with Lark", systemImage: "person.crop.circle.badge.checkmark")
                        }
                        .buttonStyle(AirNoteGhostButtonStyle())
                        .disabled(authWorking)
                        Button {
                            authWorking = true
                            Task {
                                await environment.authenticate(email: email, password: password, signup: signup)
                                await MainActor.run { authWorking = false }
                            }
                        } label: {
                            Label(authWorking ? "Connecting" : signup ? "Create account" : "Sign in", systemImage: "person.crop.circle.badge.checkmark")
                        }
                        .buttonStyle(AirNoteGhostButtonStyle())
                        .disabled(authWorking || email.isEmpty || password.count < 8)
                    }
                    .textFieldStyle(.roundedBorder)
                    .padding(12)
                    .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                }
            }

        case .privacy:
            VStack(spacing: 12) {
                AirNoteSetupRow(icon: "record.circle", title: "Visible recording only", subtitle: "The app records after a user action, never silently.", status: "Required")
                AirNoteSetupRow(icon: "doc.on.doc", title: "Secure field recovery", subtitle: "Password, OTP, and payment fields use copy-only recovery.", status: "Safe")
                AirNoteSetupRow(icon: "clock.arrow.circlepath", title: "Async learning", subtitle: "Learning never delays text insertion.", status: "0 ms")
                Toggle(isOn: $privacyAccepted) {
                    Text("I agree to these defaults")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                }
                .tint(AirNoteDesign.accent)
                .padding(12)
                .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }

        case .microphone:
            VStack(spacing: 12) {
                AirNoteCard(padding: 14) {
                    VStack(alignment: .leading, spacing: 12) {
                        HStack {
                            AirNoteStatusPill(systemImage: micChecked ? "checkmark.circle.fill" : "mic.fill",
                                              text: micChecked ? "Mic ready" : "Awaiting check",
                                              color: micChecked ? AirNoteDesign.success : AirNoteDesign.accent)
                            Spacer()
                            Text("16 kHz PCM")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(AirNoteDesign.muted)
                        }
                        AirNoteWaveform(level: micChecked ? 0.52 : 0.12, active: micChecked)
                    }
                }
                AirNoteSetupRow(icon: "waveform.path.ecg", title: "Audio route", subtitle: "Phone mic now, headset and route changes in device QA.", status: micChecked ? "OK" : "Preview")
            }

        case .keyboard:
            VStack(spacing: 10) {
                AirNoteSetupRow(icon: "keyboard", title: "Add AirNote Keyboard", subtitle: "Settings > General > Keyboard > Keyboards.", status: "Step 1")
                AirNoteSetupRow(icon: "switch.2", title: "Full Access", subtitle: "Enables the keyboard voice session.", status: fullAccessMock ? "On" : "Off")
                AirNoteSetupRow(icon: "rectangle.and.pencil.and.ellipsis", title: "Practice field", subtitle: "Notes, Messages, Slack, Gmail, or Safari.", status: "Step 3")
                Toggle(isOn: $fullAccessMock) {
                    Text("Full Access enabled")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                }
                .tint(AirNoteDesign.accent)
                .padding(12)
                .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }

        case .preview:
            VStack(spacing: 12) {
                Picker("Keyboard state", selection: $keyboardPreviewState) {
                    Text("Ready").tag(KeyboardPreviewState.ready)
                    Text("Listening").tag(KeyboardPreviewState.listening)
                    Text("Insert").tag(KeyboardPreviewState.insert)
                    Text("Copy").tag(KeyboardPreviewState.copyOnly)
                }
                .pickerStyle(.segmented)
                .tint(AirNoteDesign.accent)

                KeyboardPreviewPanel(state: keyboardPreviewState)
            }
        }
    }

    private var footerActions: some View {
        HStack(spacing: 10) {
            if step.rawValue > 0 {
                Button {
                    if let previous = MockSetupStep(rawValue: step.rawValue - 1) {
                        step = previous
                    }
                } label: {
                    Label("Back", systemImage: "arrow.left")
                }
                .buttonStyle(AirNoteGhostButtonStyle())
                .frame(maxWidth: 118)
            }

            Button(action: primaryAction) {
                Label(primaryTitle, systemImage: primaryIcon)
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
            .disabled(!canContinue)
            .opacity(canContinue ? 1 : 0.55)
        }
    }

    private var primaryTitle: String {
        switch step {
        case .welcome: return "Start setup"
        case .account: return environment.account == nil ? "Use account" : "Continue"
        case .privacy: return "Continue"
        case .microphone: return micChecked ? "Continue" : "Run mic check"
        case .keyboard: return "Preview keyboard"
        case .preview: return "Finish setup"
        }
    }

    private var primaryIcon: String {
        switch step {
        case .welcome: return "arrow.right"
        case .account: return "person.crop.circle.badge.checkmark"
        case .privacy: return "lock.shield"
        case .microphone: return micChecked ? "arrow.right" : "mic.fill"
        case .keyboard: return "keyboard"
        case .preview: return "checkmark.circle.fill"
        }
    }

    private var canContinue: Bool {
        switch step {
        case .privacy: return privacyAccepted
        case .keyboard: return fullAccessMock
        case .account: return BuildConfig.useMockGateway || environment.account != nil
        default: return true
        }
    }

    private func primaryAction() {
        switch step {
        case .welcome:
            step = .account
        case .account:
            if environment.account == nil {
                if BuildConfig.useMockGateway {
                    environment.markMockAccountReady()
                } else {
                    return
                }
            }
            step = .privacy
        case .privacy:
            environment.markPrivacyAccepted()
            step = .microphone
        case .microphone:
            if micChecked {
                environment.markMicReady()
                step = .keyboard
            } else {
                withAnimation(.easeInOut(duration: 0.25)) {
                    micChecked = true
                }
            }
        case .keyboard:
            environment.markKeyboardReady(fullAccess: fullAccessMock)
            step = .preview
        case .preview:
            if BuildConfig.useMockGateway {
                environment.dictationStore.append(
                    DictationRecord(
                        transcript: "kal ka update concise banake rahul ko bhej do",
                        polished: "Kal ka update concise bana ke Rahul ko bhej do.",
                        outcome: .inserted
                    )
                )
            }
            environment.markSetupReady()
        }
    }
}

private extension MockSetupStep {
    static var debugLaunchStep: MockSetupStep? {
        #if DEBUG
        let arguments = ProcessInfo.processInfo.arguments
        guard let index = arguments.firstIndex(of: "-AirNoteSetupStep"),
              arguments.indices.contains(index + 1)
        else {
            return nil
        }
        return Self.allCases.first { String(describing: $0) == arguments[index + 1] }
        #else
        return nil
        #endif
    }
}

enum KeyboardPreviewState: String, CaseIterable, Hashable {
    case ready
    case listening
    case insert
    case copyOnly

    var title: String {
        switch self {
        case .ready: return "AirNote ready"
        case .listening: return "Listening"
        case .insert: return "Ready to insert"
        case .copyOnly: return "Copy ready"
        }
    }

    var subtitle: String {
        switch self {
        case .ready: return "Style: Work - Hinglish"
        case .listening: return "Speak naturally. Tap stop when done."
        case .insert: return "Review, insert, copy, or save."
        case .copyOnly: return "Secure field detected. Copy polished text instead."
        }
    }

    var icon: String {
        switch self {
        case .ready: return "mic.circle.fill"
        case .listening: return "waveform.circle.fill"
        case .insert: return "text.badge.checkmark"
        case .copyOnly: return "doc.on.doc.fill"
        }
    }

    var tint: Color {
        switch self {
        case .listening: return AirNoteDesign.danger
        case .insert: return AirNoteDesign.success
        default: return AirNoteDesign.accent
        }
    }
}

struct KeyboardPreviewPanel: View {
    var state: KeyboardPreviewState

    var body: some View {
        VStack(spacing: 7) {
            voiceSurface
            ForEach(["QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"], id: \.self) { row in
                HStack(spacing: 5) {
                    ForEach(Array(row).map(String.init), id: \.self) { key in
                        keyButton(key)
                    }
                }
            }
            HStack(spacing: 6) {
                keyButton("globe", icon: "globe")
                    .frame(width: 52)
                keyButton("space")
                keyButton("delete", icon: "delete.left")
                    .frame(width: 56)
            }
        }
        .padding(8)
            .background(AirNoteDesign.keyboardWell, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder(AirNoteDesign.borderStrong, lineWidth: 1)
        )
    }

    private var voiceSurface: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 8) {
                Image(systemName: state.icon)
                    .foregroundStyle(state.tint)
                    .frame(width: 22)
                Text(state.title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Spacer()
                chip("Work")
                chip("Hinglish")
            }
            HStack(spacing: 10) {
                AirNoteWaveform(level: state == .listening ? 0.60 : 0.18,
                                active: state != .ready,
                                barCount: 8,
                                color: state.tint)
                    .frame(width: 86, height: 34)
                Text(state.subtitle)
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .lineLimit(2)
                Spacer(minLength: 0)
            }
            if state == .insert || state == .copyOnly {
                Text("Kal ka update concise bana ke Rahul ko bhej do.")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(AirNoteDesign.foreground)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .background(AirNoteDesign.surfaceRaised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            HStack(spacing: 8) {
                previewAction(state == .listening ? "Stop" : state == .copyOnly ? "Copy" : state == .insert ? "Insert" : "Start",
                              icon: state == .listening ? "stop.fill" : state == .copyOnly ? "doc.on.doc" : state == .insert ? "text.insert" : "mic.fill",
                              primary: true)
                if state == .insert || state == .copyOnly {
                    previewAction("Save", icon: "tray.and.arrow.down", primary: false)
                }
            }
        }
        .padding(10)
        .background(AirNoteDesign.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(AirNoteDesign.borderStrong, lineWidth: 1)
        )
    }

    private func chip(_ text: String) -> some View {
        Text(text)
            .font(.caption2.weight(.bold))
            .foregroundStyle(AirNoteDesign.accent)
            .padding(.horizontal, 7)
            .padding(.vertical, 4)
            .background(AirNoteDesign.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }

    private func previewAction(_ text: String, icon: String, primary: Bool) -> some View {
        Label(text, systemImage: icon)
            .font(.caption.weight(.semibold))
            .foregroundStyle(primary ? AirNoteDesign.primaryButtonForeground : AirNoteDesign.foreground)
            .frame(maxWidth: .infinity)
            .frame(height: 34)
            .background(primary ? AirNoteDesign.primaryButtonFill.opacity(0.96) : AirNoteDesign.surfaceRaised,
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(primary ? AirNoteDesign.borderStrong : AirNoteDesign.border, lineWidth: 1)
            )
    }

    private func keyButton(_ title: String, icon: String? = nil) -> some View {
        Group {
            if let icon {
                Image(systemName: icon)
                    .font(.caption.weight(.semibold))
            } else {
                Text(title == "space" ? "space" : title)
                    .font(.caption.weight(.semibold))
            }
        }
        .foregroundStyle(AirNoteDesign.foreground)
        .frame(maxWidth: .infinity)
        .frame(height: 34)
        .background(AirNoteDesign.surfaceRaised, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(AirNoteDesign.border, lineWidth: 1)
        )
    }
}

#Preview("Setup Flow") {
    SetupFlowView()
        .environmentObject(AppEnvironment())
}
