import AppKit
import SwiftUI

enum SettingsTab: String, CaseIterable, Identifiable {
    case writing = "Writing"
    case permissions = "Permissions"
    case apiKeys = "API Keys"
    case account = "Account"
    case debug = "Debug"
    case about = "About"

    var id: String { rawValue }

    var icon: String {
        switch self {
        case .writing: return "textformat"
        case .permissions: return "lock.shield"
        case .apiKeys: return "key"
        case .account: return "person.circle"
        case .debug: return "ladybug"
        case .about: return "info.circle"
        }
    }
}

struct SettingsView: View {
    let sidecar: SidecarManager
    let engine: DictationEngine
    let updateManager: SoftwareUpdateManager

    @State private var selectedTab: SettingsTab = .writing
    @State private var prefs: Preferences?

    var body: some View {
        HSplitView {
            List(SettingsTab.allCases, selection: $selectedTab) { tab in
                Label(tab.rawValue, systemImage: tab.icon)
                    .tag(tab)
            }
            .listStyle(.sidebar)
            .frame(width: 160)

            Group {
                if let prefs = prefs {
                    switch selectedTab {
                    case .writing:
                        WritingTab(prefs: prefs, onSave: savePrefs)
                    case .permissions:
                        PermissionsTab(engine: engine)
                    case .apiKeys:
                        APIKeysTab(prefs: prefs, onSave: savePrefs)
                    case .account:
                        AccountTab(sidecar: sidecar)
                    case .debug:
                        DebugTab(sidecar: sidecar)
                    case .about:
                        AboutTab(updateManager: updateManager)
                    }
                } else {
                    ProgressView("Loading settings…")
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(24)
        }
        .task { await loadPrefs() }
    }

    private func loadPrefs() async {
        guard sidecar.isHealthy else {
            try? await Task.sleep(for: .seconds(1))
            await loadPrefs()
            return
        }
        let client = BackendClient(sidecar: sidecar)
        prefs = try? await client.getPreferences()
    }

    private func savePrefs(_ update: [String: Any]) {
        Task {
            let client = BackendClient(sidecar: sidecar)
            if let updated = try? await client.patchPreferences(update) {
                await MainActor.run { prefs = updated }
            }
        }
    }
}

// MARK: - Writing Tab

struct WritingTab: View {
    let prefs: Preferences
    let onSave: ([String: Any]) -> Void

    private let tones = ["neutral", "professional", "casual", "assertive", "concise", "custom"]

    var body: some View {
        Form {
            Section("Tone Preset") {
                Picker("Tone", selection: Binding(
                    get: { prefs.tone_preset },
                    set: { onSave(["tone_preset": $0]) }
                )) {
                    ForEach(tones, id: \.self) { Text($0.capitalized).tag($0) }
                }
                .pickerStyle(.segmented)
            }

            Section("Output Language") {
                Picker("Language", selection: Binding(
                    get: { prefs.output_language },
                    set: { onSave(["output_language": $0]) }
                )) {
                    ForEach(OutputLanguage.allCases, id: \.self) { Text($0.label).tag($0.rawValue) }
                }
                .pickerStyle(.segmented)
            }

            Section("Recording Hotkey") {
                Picker("Key", selection: Binding(
                    get: { prefs.record_hotkey },
                    set: { onSave(["record_hotkey": $0]) }
                )) {
                    ForEach(RecordHotkey.allCases, id: \.self) { Text($0.label).tag($0.rawValue) }
                }
                .pickerStyle(.segmented)
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Permissions Tab

struct PermissionsTab: View {
    let engine: DictationEngine

    @State private var mic = false
    @State private var ax = false
    @State private var im = false

    var body: some View {
        Form {
            permRow("Microphone", granted: mic) {
                Task {
                    mic = await PermissionHelper.requestMicrophone()
                    refreshPermissions()
                }
            }
            permRow("Accessibility", granted: ax) {
                PermissionHelper.requestAccessibility()
                refreshPermissions()
            }
            permRow("Input Monitoring", granted: im) {
                PermissionHelper.requestInputMonitoring()
                refreshPermissions()
            }
        }
        .formStyle(.grouped)
        .onAppear {
            refreshPermissions()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            refreshPermissions()
        }
        .task {
            while !Task.isCancelled {
                refreshPermissions()
                try? await Task.sleep(for: .milliseconds(700))
            }
        }
    }

    private func permRow(_ title: String, granted: Bool, action: @escaping () -> Void) -> some View {
        HStack {
            Text(title)
            Spacer()
            if granted {
                Label("Granted", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .font(.callout)
            } else {
                Button("Grant", action: action)
            }
        }
    }

    @MainActor
    private func refreshPermissions() {
        mic = PermissionHelper.microphoneGranted
        ax = PermissionHelper.accessibilityGranted
        im = PermissionHelper.inputMonitoringGranted
        engine.refreshPermissionDependentServices()
    }
}

// MARK: - API Keys Tab

struct APIKeysTab: View {
    let prefs: Preferences
    let onSave: ([String: Any]) -> Void

    @State private var gateway = ""
    @State private var deepgram = ""
    @State private var gemini = ""
    @State private var groq = ""

    var body: some View {
        Form {
            Section("Gateway API Key") {
                SecureField("sk-…", text: $gateway)
            }
            Section("Deepgram API Key") {
                SecureField("Token…", text: $deepgram)
            }
            Section {
                SecureField("AIza…", text: $gemini)
            } header: {
                Text("Gemini API Key")
            } footer: {
                Text("Optional — enables smart learning")
            }
            Section {
                SecureField("gsk_…", text: $groq)
            } header: {
                Text("Groq API Key")
            } footer: {
                Text("Free at console.groq.com — fastest provider")
            }

            Button("Save Keys") {
                var update: [String: Any] = [:]
                if gateway != (prefs.gateway_api_key ?? "") { update["gateway_api_key"] = gateway.isEmpty ? NSNull() : gateway }
                if deepgram != (prefs.deepgram_api_key ?? "") { update["deepgram_api_key"] = deepgram.isEmpty ? NSNull() : deepgram }
                if gemini != (prefs.gemini_api_key ?? "") { update["gemini_api_key"] = gemini.isEmpty ? NSNull() : gemini }
                if groq != (prefs.groq_api_key ?? "") { update["groq_api_key"] = groq.isEmpty ? NSNull() : groq }
                if !update.isEmpty { onSave(update) }
            }
            .buttonStyle(.borderedProminent)
        }
        .formStyle(.grouped)
        .onAppear {
            gateway = prefs.gateway_api_key ?? ""
            deepgram = prefs.deepgram_api_key ?? ""
            gemini = prefs.gemini_api_key ?? ""
            groq = prefs.groq_api_key ?? ""
        }
    }
}

// MARK: - Account Tab

struct AccountTab: View {
    let sidecar: SidecarManager
    @State private var status: OpenAIStatus?
    @State private var busy = false

    var body: some View {
        Form {
            Section("OpenAI Account") {
                if let s = status, s.connected {
                    HStack {
                        Label("Connected", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                        Spacer()
                        Button("Disconnect") {
                            Task {
                                let client = BackendClient(sidecar: sidecar)
                                try? await client.disconnectOpenAI()
                                status = try? await client.getOpenAIStatus()
                            }
                        }
                        .foregroundStyle(.red)
                    }
                } else {
                    Button("Connect OpenAI") {
                        Task {
                            let client = BackendClient(sidecar: sidecar)
                            try? await client.initiateOpenAIOAuth()
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    Text("Opens browser to sign in with your ChatGPT account.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .task {
            let client = BackendClient(sidecar: sidecar)
            status = try? await client.getOpenAIStatus()
        }
    }
}

// MARK: - Debug Tab

struct DebugTab: View {
    let sidecar: SidecarManager
    @State private var logs: DebugLogs?
    @State private var tab = "combined"
    @State private var isRefreshing = false
    @State private var lastUpdated: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Picker("", selection: $tab) {
                    Text("Combined").tag("combined")
                    Text("Said").tag("desktop")
                    Text("Backend").tag("backend")
                }
                .pickerStyle(.segmented)
                .frame(width: 260)
                Spacer()
                if let lastUpdated {
                    Text("Updated \(lastUpdated.formatted(date: .omitted, time: .standard))")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Button {
                    Task { await loadLogs() }
                } label: {
                    if isRefreshing {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Image(systemName: "arrow.clockwise")
                    }
                }
                .disabled(isRefreshing)
                Button("Copy") {
                    let text: String
                    switch tab {
                    case "desktop": text = logs?.desktop ?? ""
                    case "backend": text = logs?.backend ?? ""
                    default: text = logs?.combined ?? ""
                    }
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                }
            }

            ScrollView {
                Text(currentLog)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Color(nsColor: .textBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .task {
            await loadLogs()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                await loadLogs()
            }
        }
    }

    private var currentLog: String {
        guard let logs else {
            return isRefreshing ? "(loading…)" : "(no logs loaded yet)"
        }

        let text: String
        switch tab {
        case "desktop": text = logs.desktop
        case "backend": text = logs.backend
        default: text = logs.combined
        }

        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            switch tab {
            case "desktop": return "(no Said app logs found yet)\n\(logs.desktop_path ?? "")"
            case "backend": return "(no backend logs found yet)\n\(logs.backend_path ?? "")"
            default: return "(no logs found yet)"
            }
        }
        return text
    }

    @MainActor
    private func loadLogs() async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }

        let nextLogs = await DebugLogCollector.collect(
            backendHealthy: sidecar.isHealthy,
            backendPort: sidecar.port
        )
        logs = nextLogs
        lastUpdated = Date()
    }
}

// MARK: - About Tab

struct AboutTab: View {
    let updateManager: SoftwareUpdateManager

    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "3.0.0"
    }

    private var build: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "dev"
    }

    var body: some View {
        Form {
            Section {
                VStack(alignment: .leading, spacing: 8) {
                    Image(systemName: "waveform")
                        .font(.system(size: 36))
                        .foregroundStyle(.tint)
                    Text("Said")
                        .font(.title.bold())
                    Text("Voice Polish Studio")
                        .foregroundStyle(.secondary)
                    Text("Version \(version) (\(build))")
                        .font(.callout)
                        .foregroundStyle(.tertiary)
                    Text("Local-first · Built with Swift + Rust")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
                .padding(.vertical, 4)
            }

            SoftwareUpdateSettingsSection(manager: updateManager)

            Section {
                SoftwareUpdateButton(manager: updateManager)
            }
        }
        .formStyle(.grouped)
    }
}
