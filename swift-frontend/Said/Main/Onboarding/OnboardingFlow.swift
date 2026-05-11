import SwiftUI

struct OnboardingFlow: View {
    let sidecar: SidecarManager
    let onComplete: () -> Void

    @State private var step = 0
    @State private var selectedHotkey: RecordHotkey = .capsLock
    @State private var micGranted = false
    @State private var inputMonitoringGranted = false
    @State private var accessibilityGranted = false
    @State private var wantsLearning = false
    @State private var geminiKey = ""
    @State private var permTimer: Timer?

    var body: some View {
        VStack(spacing: 0) {
            stepIndicator
                .padding(.top, 24)
                .padding(.bottom, 20)

            Group {
                switch step {
                case 0: hotkeyStep
                case 1: permissionsStep
                case 2: learningStep
                default: summaryStep
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(.horizontal, 32)
            .padding(.bottom, 24)
        }
        .frame(width: 520, height: 500)
        .background(Color(nsColor: .windowBackgroundColor))
        .onAppear { refreshPermissions() }
        .onDisappear { permTimer?.invalidate() }
    }

    private var stepIndicator: some View {
        HStack(spacing: 8) {
            ForEach(0..<4, id: \.self) { i in
                Capsule()
                    .fill(i <= step ? Color.accentColor : Color.gray.opacity(0.3))
                    .frame(width: i <= step ? 20 : 6, height: 6)
                    .animation(.spring(response: 0.3), value: step)
            }
        }
    }

    // MARK: - Step 0: Pick Hotkey

    private var hotkeyStep: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "waveform")
                .font(.system(size: 44))
                .foregroundStyle(.tint)
            Text("Welcome to Said")
                .font(.largeTitle.bold())
            Text("Hold a key to speak, release to polish.\nPick your recording key:")
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)

            HStack(spacing: 12) {
                ForEach(RecordHotkey.allCases, id: \.self) { key in
                    Button {
                        selectedHotkey = key
                    } label: {
                        VStack(spacing: 6) {
                            Image(systemName: iconForKey(key))
                                .font(.system(size: 20))
                            Text(key.label)
                                .font(.system(size: 13, weight: .semibold))
                        }
                        .frame(width: 110, height: 64)
                        .background(selectedHotkey == key ? Color.accentColor.opacity(0.15) : Color.gray.opacity(0.08))
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .overlay(
                            RoundedRectangle(cornerRadius: 12)
                                .stroke(selectedHotkey == key ? Color.accentColor : Color.clear, lineWidth: 2)
                        )
                    }
                    .buttonStyle(.plain)
                }
            }

            Spacer()
            primaryButton("Continue") { step = 1 }
        }
    }

    // MARK: - Step 1: All Required Permissions

    private var permissionsStep: some View {
        VStack(spacing: 20) {
            Spacer()
            Text("Grant permissions")
                .font(.title2.bold())
            Text("Said needs these three to work:")
                .foregroundStyle(.secondary)

            VStack(spacing: 10) {
                permissionRow(
                    icon: "mic.fill",
                    title: "Microphone",
                    subtitle: "Record your voice for dictation",
                    granted: micGranted,
                    action: {
                        Task { micGranted = await PermissionHelper.requestMicrophone() }
                    }
                )
                permissionRow(
                    icon: "keyboard",
                    title: "Input Monitoring",
                    subtitle: "Detect when you hold \(selectedHotkey.label)",
                    granted: inputMonitoringGranted,
                    action: {
                        PermissionHelper.requestInputMonitoring()
                        startPolling()
                    }
                )
                permissionRow(
                    icon: "hand.raised.fill",
                    title: "Accessibility",
                    subtitle: "Paste polished text into any app",
                    granted: accessibilityGranted,
                    action: {
                        PermissionHelper.requestAccessibility()
                        startPolling()
                    }
                )
            }

            Spacer()
            HStack {
                Button("Skip for now") { step = 2 }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                    .font(.callout)
                Spacer()
                primaryButton("Continue") { step = 2 }
                    .disabled(!allCoreGranted)
            }

            if !allCoreGranted {
                Text("Grant all three to continue, or skip to set up later in Settings.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
            }
        }
    }

    // MARK: - Step 2: Optional Learning

    private var learningStep: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "brain.head.profile")
                .font(.system(size: 40))
                .foregroundStyle(.tint)
            Text("Teach Said your style?")
                .font(.title2.bold())
            Text("When you correct Said's output, it learns your preferred words and phrasing.\n\nRequires a free Gemini API key.")
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
                .frame(maxWidth: 380)

            HStack(spacing: 12) {
                Button("Maybe later") {
                    wantsLearning = false
                    step = 3
                }
                .buttonStyle(.bordered)
                .controlSize(.large)

                Button("Set up learning") {
                    wantsLearning = true
                    step = 3
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            }
            Spacer()
        }
    }

    // MARK: - Step 3: Summary

    private var summaryStep: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 48))
                .foregroundStyle(.green)
            Text("You're all set")
                .font(.title.bold())

            VStack(alignment: .leading, spacing: 10) {
                summaryRow("Recording hotkey", selectedHotkey.label, done: true)
                summaryRow("Microphone", micGranted ? "granted" : "skipped", done: micGranted)
                summaryRow("Input Monitoring", inputMonitoringGranted ? "granted" : "skipped", done: inputMonitoringGranted)
                summaryRow("Accessibility", accessibilityGranted ? "granted" : "skipped", done: accessibilityGranted)
                summaryRow("Smart learning", wantsLearning ? "on" : "off", done: wantsLearning)
            }
            .padding(16)
            .background(Color.gray.opacity(0.08))
            .clipShape(RoundedRectangle(cornerRadius: 12))

            Text("Change any of these in Settings anytime.")
                .font(.callout)
                .foregroundStyle(.secondary)

            Spacer()
            primaryButton("Start using Said →") {
                savePreferences()
                onComplete()
            }
        }
    }

    // MARK: - Components

    private func primaryButton(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.large)
    }

    private func permissionRow(icon: String, title: String, subtitle: String, granted: Bool, action: @escaping () -> Void) -> some View {
        HStack(spacing: 12) {
            Image(systemName: icon)
                .font(.system(size: 18))
                .frame(width: 36, height: 36)
                .background(granted ? Color.green.opacity(0.12) : Color.gray.opacity(0.1))
                .foregroundStyle(granted ? .green : .secondary)
                .clipShape(RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 14, weight: .semibold))
                Text(subtitle).font(.system(size: 12)).foregroundStyle(.secondary)
            }
            Spacer()
            if granted {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .font(.system(size: 18))
            } else {
                Button("Allow", action: action)
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
            }
        }
        .padding(12)
        .background(Color.gray.opacity(0.04))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }

    private func summaryRow(_ label: String, _ value: String, done: Bool) -> some View {
        HStack {
            Image(systemName: done ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(done ? .green : .secondary)
                .font(.system(size: 14))
            Text(label)
                .font(.system(size: 13, weight: .medium))
            Spacer()
            Text(value)
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
        }
    }

    private func iconForKey(_ key: RecordHotkey) -> String {
        switch key {
        case .capsLock: return "capslock"
        case .fn: return "globe"
        case .rightOption: return "option"
        }
    }

    // MARK: - Logic

    private var allCoreGranted: Bool {
        micGranted && inputMonitoringGranted && accessibilityGranted
    }

    private func refreshPermissions() {
        micGranted = PermissionHelper.microphoneGranted
        inputMonitoringGranted = PermissionHelper.inputMonitoringGranted
        accessibilityGranted = PermissionHelper.accessibilityGranted
    }

    private func startPolling() {
        permTimer?.invalidate()
        permTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
            refreshPermissions()
        }
    }

    private func savePreferences() {
        Task {
            while !sidecar.isHealthy {
                try? await Task.sleep(for: .milliseconds(200))
            }
            let client = BackendClient(sidecar: sidecar)
            var update: [String: Any] = [
                "record_hotkey": selectedHotkey.rawValue,
                "learning_enabled": wantsLearning,
            ]
            if !geminiKey.isEmpty {
                update["gemini_api_key"] = geminiKey
            }
            let _ = try? await client.patchPreferences(update)
        }
    }
}
