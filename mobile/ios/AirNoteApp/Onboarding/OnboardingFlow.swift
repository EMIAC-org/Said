import AirNoteShared
import SwiftUI

struct OnboardingFlow: View {
    @EnvironmentObject private var env: AppEnvironment

    enum Step: Int, CaseIterable {
        case welcome, account, privacy, microphone, keyboard, voiceKeys, firstDictation, personalize
    }

    @State private var step: Step = .welcome

    var body: some View {
        ZStack {
            AirNoteBackground()
            VStack(spacing: 0) {
                if step != .welcome {
                    ProgressRail(current: step.rawValue, total: Step.allCases.count)
                        .padding(.horizontal, 20)
                        .padding(.top, 12)
                }
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .transition(.asymmetric(
                        insertion: .move(edge: .trailing).combined(with: .opacity),
                        removal: .move(edge: .leading).combined(with: .opacity)
                    ))
                    .id(step)
            }
        }
        .animation(.easeInOut(duration: 0.3), value: step)
        .onAppear {
            // A restored session (e.g. after reinstall) is already signed in —
            // don't make them re-enter the account step.
            if env.account != nil, step.rawValue <= Step.account.rawValue {
                step = .privacy
            }
        }
        .onChange(of: env.account?.id) { _, id in
            // Advance off the account screen as soon as sign-in succeeds.
            if id != nil, step == .account { advance() }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch step {
        case .welcome:
            WelcomeStep(onContinue: advance)
        case .account:
            AccountStep()
        case .privacy:
            PrivacyStep(onContinue: advance)
        case .microphone:
            MicrophoneStep(onContinue: advance)
        case .keyboard:
            KeyboardStep(onContinue: advance)
        case .voiceKeys:
            VoiceKeysStep(onContinue: advance)
        case .firstDictation:
            FirstDictationStep(onContinue: advance)
        case .personalize:
            PersonalizeStep(onFinish: { env.completeOnboarding() })
        }
    }

    private func advance() {
        if let next = Step(rawValue: step.rawValue + 1) {
            step = next
        } else {
            env.completeOnboarding()
        }
    }
}

// MARK: - Progress rail

private struct ProgressRail: View {
    var current: Int
    var total: Int

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0..<total, id: \.self) { index in
                Capsule()
                    .fill(index <= current ? AirNoteDesign.accent : AirNoteDesign.surfaceHover)
                    .frame(height: 4)
            }
        }
        .accessibilityLabel("Step \(current + 1) of \(total)")
    }
}

// MARK: - Reusable step scaffold

private struct OnboardingScaffold<Content: View, Footer: View>: View {
    var eyebrow: String
    var title: String
    var subtitle: String
    @ViewBuilder var content: () -> Content
    @ViewBuilder var footer: () -> Footer

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(eyebrow.uppercased())
                            .font(.caption2.weight(.bold))
                            .tracking(0.9)
                            .foregroundStyle(AirNoteDesign.accent)
                        Text(title)
                            .font(.system(size: 30, weight: .bold, design: .rounded))
                            .foregroundStyle(AirNoteDesign.foreground)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(subtitle)
                            .font(.subheadline)
                            .foregroundStyle(AirNoteDesign.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    content()
                }
                .padding(20)
            }
            VStack(spacing: 10) { footer() }
                .padding(.horizontal, 20)
                .padding(.bottom, 20)
                .padding(.top, 8)
        }
    }
}

// MARK: - Welcome

private struct WelcomeStep: View {
    var onContinue: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            VStack(spacing: 18) {
                AirNoteLogoTile(size: 96)
                VStack(spacing: 10) {
                    Text("Speak naturally.\nAirNote writes it clearly.")
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                        .multilineTextAlignment(.center)
                        .foregroundStyle(AirNoteDesign.foreground)
                        .fixedSize(horizontal: false, vertical: true)
                    Text("A voice keyboard for English, Hindi, and Hinglish — polished in real time.")
                        .font(.subheadline)
                        .foregroundStyle(AirNoteDesign.muted)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 24)
                }
                VStack(spacing: 10) {
                    FeatureRow(icon: "mic.fill", title: "Speak, don't type", subtitle: "Hold a thought and say it out loud.")
                    FeatureRow(icon: "sparkles", title: "Polished instantly", subtitle: "Clean, readable text in your style.")
                    FeatureRow(icon: "lock.shield.fill", title: "Private by design", subtitle: "Recordings are never stored.")
                }
                .padding(.horizontal, 20)
                .padding(.top, 6)
            }
            Spacer()
            Button(action: onContinue) {
                Label("Get started", systemImage: "arrow.right")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
            .padding(.horizontal, 20)
            .padding(.bottom, 24)
        }
    }
}

private struct FeatureRow: View {
    var icon: String
    var title: String
    var subtitle: String

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: icon)
                .font(.headline)
                .foregroundStyle(AirNoteDesign.accent)
                .frame(width: 44, height: 44)
                .background(AirNoteDesign.accent.opacity(0.14), in: Circle())
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.subheadline.weight(.semibold)).foregroundStyle(AirNoteDesign.foreground)
                Text(subtitle).font(.caption).foregroundStyle(AirNoteDesign.muted)
            }
            Spacer(minLength: 0)
        }
    }
}

// MARK: - Account

private struct AccountStep: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.openURL) private var openURL
    @State private var email = ""
    @State private var password = ""
    @State private var isSignup = true
    @FocusState private var focus: Field?

    enum Field { case email, password }

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Account",
            title: isSignup ? "Create your account" : "Welcome back",
            subtitle: isSignup ? "Sign up with email — it takes seconds." : "Sign in to pick up where you left off."
        ) {
            VStack(spacing: 12) {
                TextField("you@company.com", text: $email)
                    .textFieldStyle(AirNoteFieldStyle())
                    .textContentType(.emailAddress)
                    .keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .email)
                SecureField("Password (8+ characters)", text: $password)
                    .textFieldStyle(AirNoteFieldStyle())
                    .textContentType(isSignup ? .newPassword : .password)
                    .focused($focus, equals: .password)
                    .submitLabel(.go)
                    .onSubmit { submit() }

                if let error = env.authError {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.danger)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }

                Button {
                    isSignup.toggle()
                    env.authError = nil
                } label: {
                    Text(isSignup ? "Already have an account? Sign in" : "New here? Create an account")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.accent)
                }
                .frame(maxWidth: .infinity, alignment: .trailing)

                HStack(spacing: 10) {
                    Rectangle().fill(AirNoteDesign.border).frame(height: 1)
                    Text("or").font(.caption2).foregroundStyle(AirNoteDesign.muted)
                    Rectangle().fill(AirNoteDesign.border).frame(height: 1)
                }
                .padding(.vertical, 2)

                Button {
                    openURL(BuildConfig.gatewayBaseURL.appendingPathComponent("auth/lark"))
                } label: {
                    Label("Continue with Lark", systemImage: "person.crop.circle")
                }
                .buttonStyle(AirNoteGhostButtonStyle())
            }
        } footer: {
            Button(action: submit) {
                Label(env.isAuthenticating ? "Please wait…" : (isSignup ? "Create account" : "Sign in"),
                      systemImage: env.isAuthenticating ? "hourglass" : "arrow.right")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
            .disabled(env.isAuthenticating || email.trimmingCharacters(in: .whitespaces).isEmpty || password.count < 8)

            Text("Free to start · no credit card")
                .font(.caption2)
                .foregroundStyle(AirNoteDesign.muted)
                .frame(maxWidth: .infinity)
        }
    }

    private func submit() {
        focus = nil
        Task { _ = await env.authenticate(email: email, password: password, signup: isSignup) }
    }
}

// MARK: - Privacy

private struct PrivacyStep: View {
    var onContinue: () -> Void
    @State private var accepted = false

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Privacy",
            title: "Your words stay yours",
            subtitle: "AirNote sends your speech to its servers to transcribe and polish it — then the audio is discarded."
        ) {
            VStack(spacing: 10) {
                AirNoteSetupRow(icon: "waveform.slash", title: "Recordings are never stored", subtitle: "Audio streams to transcription and is dropped immediately.", status: "Always")
                AirNoteSetupRow(icon: "lock.fill", title: "Secure fields are skipped", subtitle: "AirNote never records into password or OTP fields.", status: "Safe")
                AirNoteSetupRow(icon: "key.fill", title: "Provider keys stay server-side", subtitle: "Your account never holds raw API keys.", status: "Secure")
                Toggle(isOn: $accepted) {
                    Text("I understand how AirNote handles my dictation")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(AirNoteDesign.foreground)
                }
                .tint(AirNoteDesign.accent)
                .padding(12)
                .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        } footer: {
            Button(action: onContinue) {
                Label("Continue", systemImage: "arrow.right")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
            .disabled(!accepted)
            .opacity(accepted ? 1 : 0.5)
        }
    }
}

// MARK: - Microphone

private struct MicrophoneStep: View {
    @EnvironmentObject private var env: AppEnvironment
    var onContinue: () -> Void
    @State private var requesting = false

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Microphone",
            title: "Let AirNote hear you",
            subtitle: "AirNote uses the microphone only while you're dictating — never in the background."
        ) {
            VStack(spacing: 12) {
                ZStack {
                    Circle().fill(AirNoteDesign.accent.opacity(0.12)).frame(width: 120, height: 120)
                    Image(systemName: env.permissions.micPermission == .granted ? "checkmark" : "mic.fill")
                        .font(.system(size: 44, weight: .bold))
                        .foregroundStyle(env.permissions.micPermission == .granted ? AirNoteDesign.success : AirNoteDesign.accent)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)

                if env.permissions.micPermission == .denied {
                    Text("Microphone access is off. Turn it on in Settings to dictate.")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.danger)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity)
                }
            }
        } footer: {
            if env.permissions.micPermission == .granted {
                Button(action: onContinue) {
                    Label("Continue", systemImage: "arrow.right")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
            } else if env.permissions.micPermission == .denied {
                Button { env.permissions.openSettings() } label: {
                    Label("Open Settings", systemImage: "gearshape")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
                Button("Continue anyway", action: onContinue)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.muted)
            } else {
                Button {
                    requesting = true
                    Task {
                        let granted = await env.permissions.requestMic()
                        await MainActor.run {
                            requesting = false
                            if granted { onContinue() }
                        }
                    }
                } label: {
                    Label(requesting ? "Requesting…" : "Allow microphone", systemImage: "mic.fill")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
                .disabled(requesting)
            }
        }
    }
}

// MARK: - Keyboard

private struct KeyboardStep: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.scenePhase) private var scenePhase
    var onContinue: () -> Void

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Keyboard",
            title: "Dictate in any app",
            subtitle: "Add the AirNote Keyboard so you can speak into Messages, Mail, Slack — anywhere you type."
        ) {
            VStack(alignment: .leading, spacing: 14) {
                StepRow(number: 1, title: "Open Settings", detail: "Tap the button below.")
                StepRow(number: 2, title: "Keyboards → AirNote Keyboard", detail: "Turn it on.")
                StepRow(number: 3, title: "Allow Full Access", detail: "Required so AirNote can send your speech to its servers for transcription. Audio is handled securely and is not stored.")
                if env.permissions.keyboard == .ready {
                    Label("AirNote Keyboard is ready", systemImage: "checkmark.circle.fill")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.success)
                        .padding(.top, 4)
                }
            }
        } footer: {
            Button { env.permissions.openSettings() } label: {
                Label("Open Settings", systemImage: "gearshape")
            }
            .buttonStyle(AirNoteGhostButtonStyle())
            Button(action: onContinue) {
                Label(env.permissions.keyboard == .ready ? "Continue" : "I'll do this later", systemImage: "arrow.right")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { env.permissions.refreshKeyboard() }
        }
    }
}

// MARK: - Voice keys (BYOK)

private struct VoiceKeysStep: View {
    @EnvironmentObject private var env: AppEnvironment
    var onContinue: () -> Void

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Voice keys",
            title: "Connect your voice",
            subtitle: "Add your own Deepgram and Groq keys so AirNote can transcribe and polish. Both have free tiers — stored encrypted on AirNote's servers under your account."
        ) {
            VStack(spacing: 12) {
                ProviderKeyCard(
                    provider: "deepgram",
                    name: "Deepgram",
                    role: "Speech-to-text",
                    badge: "D",
                    color: Color(red: 0.04, green: 0.55, blue: 0.54),
                    getKeyURL: URL(string: "https://console.deepgram.com/signup")!,
                    placeholder: "Paste your Deepgram key"
                )
                ProviderKeyCard(
                    provider: "groq",
                    name: "Groq",
                    role: "AI polish",
                    badge: "G",
                    color: Color(red: 0.96, green: 0.31, blue: 0.21),
                    getKeyURL: URL(string: "https://console.groq.com/keys")!,
                    placeholder: "gsk_…"
                )
                if !env.credentialStatus.isEmpty {
                    Text(env.credentialStatus)
                        .font(.caption)
                        .foregroundStyle(env.credentialStatus.hasPrefix("Couldn't") || env.credentialStatus.contains("too short") ? AirNoteDesign.danger : AirNoteDesign.success)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        } footer: {
            if env.dictationAvailable {
                Label("Dictation is ready", systemImage: "checkmark.circle.fill")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.success)
                    .frame(maxWidth: .infinity)
            }
            Button(action: onContinue) {
                Label(env.dictationAvailable ? "Continue" : "I'll add keys later", systemImage: "arrow.right")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
        }
        .task { await env.refreshCredentials() }
    }
}

// MARK: - First dictation

private struct FirstDictationStep: View {
    @EnvironmentObject private var env: AppEnvironment
    var onContinue: () -> Void
    @State private var showSheet = false
    @State private var didDictate = false

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Try it",
            title: didDictate ? "Nice — that's it!" : "Your first dictation",
            subtitle: didDictate
                ? "That's all there is to it. Speak, and AirNote writes it cleanly."
                : (env.dictationAvailable
                   ? "Tap the mic, say a sentence, and watch AirNote polish it."
                   : "Dictation turns on automatically once your workspace finishes setup — you're all set to continue.")
        ) {
            VStack(spacing: 14) {
                ZStack {
                    Circle().fill((didDictate ? AirNoteDesign.success : AirNoteDesign.accent).opacity(0.12)).frame(width: 120, height: 120)
                    Image(systemName: didDictate ? "checkmark.seal.fill" : "waveform")
                        .font(.system(size: 44, weight: .bold))
                        .foregroundStyle(didDictate ? AirNoteDesign.success : AirNoteDesign.accent)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)
            }
        } footer: {
            if env.dictationAvailable && !didDictate {
                Button { showSheet = true } label: {
                    Label("Try a dictation", systemImage: "mic.fill")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
                Button("Skip for now", action: onContinue)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.muted)
            } else {
                Button(action: onContinue) {
                    Label("Continue", systemImage: "arrow.right")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())
            }
        }
        .sheet(isPresented: $showSheet) {
            NavigationStack {
                DictationSheet(env: env, showsDoneButton: true) { _ in
                    didDictate = true
                    showSheet = false
                }
            }
        }
    }
}

// MARK: - Personalize

private struct PersonalizeStep: View {
    @EnvironmentObject private var env: AppEnvironment
    var onFinish: () -> Void
    @State private var language = SharedStore.outputLanguage
    // Coerce any legacy/blank stored value to a canonical tone key so a fresh install
    // shows a real tone pre-selected (the option list uses AirNoteTone's canonical keys).
    @State private var tone = AirNoteTone.coerced(SharedStore.tonePreset)
    @State private var saving = false

    var body: some View {
        OnboardingScaffold(
            eyebrow: "Make it yours",
            title: "Sound like you",
            subtitle: "Pick how AirNote should write. You can change this any time in Settings."
        ) {
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 8) {
                    AirNoteSectionLabel(text: "Language")
                    Picker("Language", selection: $language) {
                        Text("Hinglish").tag("hinglish")
                        Text("English").tag("english")
                    }
                    .pickerStyle(.segmented)
                }
                VStack(alignment: .leading, spacing: 8) {
                    AirNoteSectionLabel(text: "Default tone")
                    ForEach(AirNoteTone.all) { option in
                        Button { tone = option.key } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.label).font(.subheadline.weight(.semibold)).foregroundStyle(AirNoteDesign.foreground)
                                    Text(option.detail).font(.caption).foregroundStyle(AirNoteDesign.muted)
                                }
                                Spacer()
                                Image(systemName: tone == option.key ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(tone == option.key ? AirNoteDesign.accent : AirNoteDesign.muted)
                            }
                            .padding(12)
                            .background(AirNoteDesign.surfaceRaised.opacity(tone == option.key ? 0.7 : 0.4), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(tone == option.key ? AirNoteDesign.accent.opacity(0.4) : AirNoteDesign.border, lineWidth: 1))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        } footer: {
            Button {
                saving = true
                Task {
                    await env.setOutputLanguage(language)
                    await env.setTonePreset(tone)
                    await MainActor.run { onFinish() }
                }
            } label: {
                Label(saving ? "Finishing…" : "Start using AirNote", systemImage: "checkmark.circle.fill")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
            .disabled(saving)
        }
    }
}
