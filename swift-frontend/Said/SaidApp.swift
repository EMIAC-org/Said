import SwiftUI

@main
struct SaidApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        MenuBarExtra("Said", systemImage: "waveform") {
            Button(appDelegate.manualRecording ? "Stop Recording" : "Start Recording") {
                appDelegate.toggleManualRecording()
            }

            Divider()

            Menu("Output Language") {
                Button("✓  Hinglish") { appDelegate.setOutputLanguage("hinglish") }
                Button("    English") { appDelegate.setOutputLanguage("english") }
                Button("    Hindi") { appDelegate.setOutputLanguage("hindi") }
            }

            Menu("Polish my message") {
                Button("Smart Repair Last  ⌥1") { appDelegate.engine?.handleShortcutPublic(1) }
                Button("Professional  ⌥2") { appDelegate.engine?.handleShortcutPublic(2) }
                Button("Casual  ⌥3") { appDelegate.engine?.handleShortcutPublic(3) }
                Button("Concise  ⌥4") { appDelegate.engine?.handleShortcutPublic(4) }
                Button("Hinglish  ⌥5") { appDelegate.engine?.handleShortcutPublic(5) }
            }

            Divider()

            Button("Open Said") {
                appDelegate.showMainWindow()
            }

            Button("Settings…") {
                appDelegate.showMainWindow()
            }
            .keyboardShortcut(",", modifiers: .command)

            SoftwareUpdateButton(manager: appDelegate.updateManager)

            Divider()

            Button("Quit Said") {
                appDelegate.cleanup()
                NSApplication.shared.terminate(nil)
            }
            .keyboardShortcut("q", modifiers: .command)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let sidecar = SidecarManager()
    let updateManager = SoftwareUpdateManager.shared
    var engine: DictationEngine?
    private var notchWindow: NotchWindow?
    private var mainWindow: NSWindow?
    private var hasCompletedOnboarding: Bool {
        get { UserDefaults.standard.bool(forKey: "onboardingComplete") }
        set { UserDefaults.standard.set(newValue, forKey: "onboardingComplete") }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        sidecar.start()
        engine = DictationEngine(sidecar: sidecar)

        if hasCompletedOnboarding {
            setupNotchAndStart()
        } else {
            showOnboarding()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        cleanup()
    }

    func cleanup() {
        sidecar.stop()
    }

    func setupNotchAndStart() {
        let screen = NSScreen.main ?? NSScreen.screens.first!
        let metrics = NotchDetector.detect(screen: screen)
        engine!.notchVM.refreshMetrics(screen: screen)
        let window = NotchWindow(metrics: metrics, screen: screen)
        let contentView = NotchContentView(
            vm: engine!.notchVM,
            onManualRecord: { [weak self] in self?.toggleManualRecording() },
            onPolishShortcut: { [weak self] n in self?.engine?.handleShortcutPublic(n) },
            onRepaste: { [weak self] in self?.engine?.pasteLatestPublic() },
            onSetLanguage: { [weak self] lang in self?.setOutputLanguage(lang) },
            onOpenSettings: { [weak self] in self?.showMainWindow() }
        )
        window.contentView = NSHostingView(rootView: contentView)
        window.orderFrontRegardless()
        NotchSpaceManager.shared.addWindow(window)
        notchWindow = window

        loadPrefsAndStart()
    }

    private func loadPrefsAndStart() {
        Task {
            while !sidecar.isHealthy {
                try? await Task.sleep(for: .milliseconds(200))
            }

            let client = BackendClient(sidecar: sidecar)
            do {
                let prefs = try await client.getPreferences()
                let hotkey = RecordHotkey(rawValue: prefs.record_hotkey) ?? .capsLock
                let dgKey = prefs.deepgram_api_key ?? ProcessInfo.processInfo.environment["DEEPGRAM_API_KEY"] ?? ""
                let sttMode = prefs.language == "auto" || prefs.language == "multi" ? "multi" : prefs.language

                await MainActor.run {
                    engine?.configure(deepgramKey: dgKey, sttMode: sttMode, hotkey: hotkey)
                    engine?.notchVM.activeHotkeyLabel = hotkey.label
                    engine?.notchVM.outputLanguage = prefs.output_language
                    engine?.notchVM.backendReady = true
                    engine?.start()
                }
            } catch {
                await MainActor.run {
                    engine?.configure(deepgramKey: "", sttMode: "multi", hotkey: .capsLock)
                    engine?.notchVM.activeHotkeyLabel = "Caps Lock"
                    engine?.notchVM.backendReady = true
                    engine?.start()
                }
            }
        }
    }

    var manualRecording = false

    func setOutputLanguage(_ lang: String) {
        let client = BackendClient(sidecar: sidecar)
        Task {
            _ = try? await client.patchPreferences(["output_language": lang])
        }
    }

    func toggleManualRecording() {
        guard let engine else { return }
        if manualRecording {
            engine.stopRecording()
            manualRecording = false
        } else {
            engine.startRecording()
            manualRecording = true
        }
    }

    func showOnboarding() {
        let onboarding = OnboardingFlow(
            sidecar: sidecar,
            onComplete: { [weak self] in
                self?.hasCompletedOnboarding = true
                self?.mainWindow?.close()
                self?.mainWindow = nil
                self?.setupNotchAndStart()
            }
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 480),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        window.center()
        window.contentView = NSHostingView(rootView: onboarding)
        window.makeKeyAndOrderFront(nil)
        mainWindow = window
    }

    func showMainWindow() {
        if let existing = mainWindow {
            existing.makeKeyAndOrderFront(nil)
            return
        }
        let settingsView = MainContentView(
            sidecar: sidecar,
            engine: engine!,
            updateManager: updateManager
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 620),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.titlebarAppearsTransparent = true
        window.title = "Said"
        window.center()
        window.contentView = NSHostingView(rootView: settingsView)
        window.makeKeyAndOrderFront(nil)
        mainWindow = window
    }
}
