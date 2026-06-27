import AirNoteShared
import Foundation
import UIKit
#if canImport(ActivityKit)
import ActivityKit
#endif

/// Keeps the microphone warm in the background after a dictation so the keyboard
/// can dictate IN-PLACE (no app re-foreground) — the Wispr "session" model.
///
/// iOS only lets a foreground app *start* the mic, so the first dictation always
/// runs in the foreground (via the handoff). Right after it finishes (still
/// foreground), we keep the engine running; with `UIBackgroundModes: audio` the
/// app then stays alive in the background with the mic warm. The keyboard wakes
/// it over Darwin notifications for each subsequent dictation, and we drop the
/// polished text in the App Group for the keyboard to insert.
@MainActor
final class WarmDictationHost: ObservableObject {
    static let shared = WarmDictationHost()

    /// When the warm session expires (drives the in-app "keyboard ready" status).
    @Published private(set) var warmUntil: Date?

    /// The user's session ON/OFF intent (Wispr-style), surfaced for the header
    /// toggle. Reflects SharedStore.sessionEnabled — NOT the transient warm-engine
    /// state (which tears down + re-arms every dictation), so the toggle never
    /// flickers off mid-use.
    @Published private(set) var isSessionActive: Bool = SharedStore.sessionEnabled

    private let streamer = VoiceStreamingClient()
    private let gateway = GatewayEnvironment.makeClient()
    private var warmWindowTask: Task<Void, Never>?
    /// Liveness heartbeat: while the mic is warm we stamp SharedStore every ~2s so
    /// the keyboard (and the notch) can tell a live session from a force-quit one.
    private var heartbeatTimer: Timer?
    private var heartbeatTick = 0
    private var currentRunID = ""
    private var isStreaming = false
    private var maxLevel: Float = 0
    private var lastLoudAt: Date?
    private var lastPartialAt = Date.distantPast
    private var lastLevelAt = Date.distantPast
    private var observing = false

    /// How long the mic stays warm after the last dictation (extends on each use),
    /// from the user's session-duration setting. -1 minutes = "until I stop it".
    private var warmWindow: TimeInterval {
        let m = SharedStore.sessionDurationMinutes
        return m < 0 ? .infinity : TimeInterval(max(1, m) * 60)
    }
    private var neverExpires: Bool { SharedStore.sessionDurationMinutes < 0 }
    // Generous silence window so natural pauses never clip words (the user can
    // still tap the mic to stop sooner). Lower speech threshold keeps soft/
    // trailing speech from being misread as silence.
    private let silenceAutoStop: TimeInterval = 4.0
    private let speechLevelThreshold: Float = 0.04

    private init() {
        streamer.onUpdate = { [weak self] update in
            Task { @MainActor in self?.handle(update) }
        }
    }

    /// Register Darwin observers once (called at launch).
    func startObserving() {
        guard !observing else { return }
        observing = true
        DarwinSignal.shared.observe(DarwinSignal.startDictation) { [weak self] in
            Task { @MainActor in await self?.beginDictation() }
        }
        DarwinSignal.shared.observe(DarwinSignal.stopDictation) { [weak self] in
            Task { @MainActor in await self?.stopDictation() }
        }
        // The Dynamic Island Stop/Resume buttons flip SharedStore.sessionEnabled,
        // then signal us to reconcile the warm engine + the Activity.
        DarwinSignal.shared.observe(DarwinSignal.sessionControl) { [weak self] in
            Task { @MainActor in self?.setSessionEnabled(SharedStore.sessionEnabled) }
        }
        // Removed from the app switcher (deck): iOS posts this just before tearing
        // the scene down. `willTerminate` never fires on a force-quit, but this does
        // — and because the warm session keeps the app alive in the background, the
        // handler gets to run and clear the notch before the process dies. We end
        // the Activity synchronously (semaphore) because a fire-and-forget Task can
        // lose the race with termination.
        NotificationCenter.default.addObserver(
            forName: UIScene.didDisconnectNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.handleSceneDisconnect() }
        }
    }

    /// App is being removed from the deck. Tear down the warm engine + clear the
    /// notch NOW. Keep `sessionEnabled` ON so the session auto-restarts next launch
    /// (the user's intent persists — they removed the app, they didn't turn it off).
    private func handleSceneDisconnect() {
        stopHeartbeat()
        warmWindowTask?.cancel()
        warmWindowTask = nil
        isStreaming = false
        SharedStore.sessionWarmUntil = nil
        warmUntil = nil
        streamer.stopWarmEngine()
        #if canImport(ActivityKit)
        // Block briefly so the async end() completes before the process is killed —
        // the validated pattern for ending Live Activities during scene teardown.
        let semaphore = DispatchSemaphore(value: 0)
        let held = liveActivity
        liveActivity = nil
        Task.detached {
            await held?.end(nil, dismissalPolicy: .immediate)
            for activity in Activity<DictationSessionAttributes>.activities {
                await activity.end(nil, dismissalPolicy: .immediate)
            }
            semaphore.signal()
        }
        _ = semaphore.wait(timeout: .now() + 2.0)
        #endif
    }

    /// Called (while foreground) right after a handoff dictation completes, to
    /// keep the mic warm for in-place dictations. Best-effort: if it fails, the
    /// keyboard simply falls back to the foreground handoff.
    func warmUp() {
        Task { @MainActor in
            for attempt in 0..<3 {
                do {
                    try await streamer.startWarmEngine()
                    extendWarmWindow()
                    prewarm()
                    syncLiveActivity()   // engine is warm → show the notch
                    return
                } catch {
                    // The mic engine can fail to start during the app-launch
                    // transition; back off briefly and retry before giving up.
                    if attempt < 2 { try? await Task.sleep(nanoseconds: 450_000_000) }
                }
            }
            SharedStore.sessionWarmUntil = nil
            syncLiveActivity()   // never started → don't leave a stale notch
        }
    }

    /// Build a session shell (token + voice-WS URL) for streaming / pre-warming.
    private func makeSession() -> MobileSessionResponse? {
        guard let token = SharedStore.accessToken, !token.isEmpty else { return nil }
        return MobileSessionResponse(
            sessionID: RequestId.make(),
            sessionToken: token,
            expiresAt: Date().addingTimeInterval(15 * 60),
            streamingEnabled: true,
            currentVocabHash: "keyboard",
            voiceWSURL: "/v1/runtime/voice/ws?token=\(token)",
            batchURL: "/v1/runtime/voice/wav",
            maxRecordingSeconds: BuildConfig.maxRecordingSeconds
        )
    }

    /// Open the next dictation's socket now (warm-idle) so the first words ship
    /// instantly instead of waiting on a TLS + WebSocket handshake.
    private func prewarm() {
        if let session = makeSession() { streamer.prewarmConnection(session: session) }
    }

    func endWarmSession() {
        warmWindowTask?.cancel()
        warmWindowTask = nil
        stopHeartbeat()
        isStreaming = false
        SharedStore.sessionWarmUntil = nil
        warmUntil = nil
        streamer.stopWarmEngine()
        syncLiveActivity()   // engine stopped → clear the notch
    }

    // MARK: Liveness heartbeat

    /// Stamp `warmHeartbeatAt` now and every ~2s while warm. Idempotent. A live app
    /// keeps this fresh; a force-quit stops it, so the keyboard's `warmHeartbeatFresh`
    /// check flips to false within seconds and it stops waiting on the dead app.
    /// Every ~16s it also refreshes the Live Activity so its `staleDate` stays ahead
    /// of "now" while alive, and lapses (notch goes stale) shortly after a force-quit.
    private func startHeartbeat() {
        SharedStore.warmHeartbeatAt = Date()
        guard heartbeatTimer == nil else { return }
        let timer = Timer(timeInterval: 2.0, repeats: true) { [weak self] _ in
            SharedStore.warmHeartbeatAt = Date()   // synchronous, thread-safe, every 2s
            Task { @MainActor in
                guard let self else { return }
                self.heartbeatTick += 1
                if self.heartbeatTick % 8 == 0 { self.syncLiveActivity() }
            }
        }
        // `.common` so it keeps firing in the background (the warm audio session
        // keeps the run loop alive) and during scroll/scene transitions.
        RunLoop.main.add(timer, forMode: .common)
        heartbeatTimer = timer
    }

    private func stopHeartbeat() {
        heartbeatTimer?.invalidate()
        heartbeatTimer = nil
        heartbeatTick = 0
        SharedStore.warmHeartbeatAt = nil   // immediately stale, not just on next check
    }

    // MARK: Session intent (Wispr-style persistent session)

    /// Auto-start entry — idempotent, safe to call on every app foreground. Starts
    /// the warm engine ONLY if the user's session intent is ON, the mic is already
    /// granted (never prompts here — this is a silent lifecycle hook), and no
    /// dictation is in flight. Sets the session persistent ("until I stop it").
    /// Called on app foreground: when the session intent is ON and the mic is
    /// granted, ARM the warm engine now (this is the Wispr "session starts when the
    /// app opens" behavior) so the keyboard can dictate in-place. The app is
    /// foreground here, which is the one place iOS lets the mic start.
    func ensureSessionActive() {
        guard SharedStore.sessionEnabled else { syncLiveActivity(); return }
        guard PermissionManager.currentMicPermission() == .granted else { syncLiveActivity(); return }
        guard !isStreaming else { return }
        SharedStore.sessionDurationMinutes = -1   // persistent: never auto-expires
        isSessionActive = true
        if streamer.isWarmEngineRunning {
            extendWarmWindow(); prewarm(); syncLiveActivity()
        } else {
            warmUp()   // starts the engine (with retry), then syncs the notch
        }
    }

    /// Turn the session ON/OFF from the header toggle / the notch Stop button. OFF
    /// ends the warm session + clears the notch. ON sets the intent and, since this
    /// is a deliberate foreground gesture, warms the engine if it isn't already.
    func setSessionEnabled(_ on: Bool) {
        SharedStore.sessionEnabled = on
        isSessionActive = on
        if on {
            SharedStore.sessionDurationMinutes = -1
            if streamer.isWarmEngineRunning {
                extendWarmWindow(); prewarm(); syncLiveActivity()
            } else {
                warmUp()
            }
        } else {
            endWarmSession()
        }
    }

    // MARK: Dynamic Island Live Activity

    #if canImport(ActivityKit)
    private var liveActivity: Activity<DictationSessionAttributes>?

    /// Show the Dynamic Island Activity ONLY while the warm session is genuinely
    /// running, and END it the instant the session stops (Stop button / interruption
    /// / sign-out / session off) so the notch clears and reflects reality. Adopts an
    /// orphan left by a force-quit, so reopening AirNote reconciles + clears it.
    private func syncLiveActivity() {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        // Drop a stale handle — the Stop intent ends the Activity from its OWN
        // process, leaving our reference pointing at a dead Activity. Without this,
        // a later Resume would `update()` the corpse and the notch would never come
        // back. Re-adopt only a genuinely live Activity.
        if let held = liveActivity, held.activityState != .active { liveActivity = nil }
        if liveActivity == nil {
            liveActivity = Activity<DictationSessionAttributes>.activities.first { $0.activityState == .active }
        }
        // Tie the notch to the session INTENT (the toggle / sessionEnabled), not the
        // instantaneous engine state — so it appears reliably while the session is
        // on and clears when turned off, without flickering off on a momentary
        // engine dip or a scene transition (which made it never appear).
        if SharedStore.sessionEnabled {
            // staleDate ~25s ahead; the heartbeat re-syncs every ~16s so it stays
            // fresh while the app is alive. After a force-quit nothing re-syncs, so
            // the system marks the notch stale within seconds — signalling the dead
            // session — and it fully clears the moment AirNote is reopened.
            let content = ActivityContent(
                state: DictationSessionAttributes.ContentState(listening: false, active: true),
                staleDate: Date().addingTimeInterval(25)
            )
            if let activity = liveActivity {
                Task { await activity.update(content) }
            } else {
                liveActivity = try? Activity.request(
                    attributes: DictationSessionAttributes(), content: content, pushType: nil
                )
            }
        } else {
            endLiveActivity()
        }
    }

    /// Reconcile only the notch (no engine start) — safe to call from any scene
    /// phase, including background.
    func reconcileNotch() { syncLiveActivity() }

    private func endLiveActivity() {
        let held = liveActivity
        liveActivity = nil
        Task {
            await held?.end(nil, dismissalPolicy: .immediate)
            // Reap any orphan (e.g. one left after a force-quit) so it can't linger.
            for activity in Activity<DictationSessionAttributes>.activities {
                await activity.end(nil, dismissalPolicy: .immediate)
            }
        }
    }
    #else
    private func syncLiveActivity() {}
    #endif

    // MARK: Streaming

    private func beginDictation() async {
        guard !isStreaming, let token = SharedStore.accessToken, !token.isEmpty else {
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
            return
        }
        maxLevel = 0
        lastLoudAt = nil
        clearLivePartial()
        SharedStore.pendingKeyboardText = nil
        currentRunID = RequestId.make()
        let runID = currentRunID
        let session = MobileSessionResponse(
            sessionID: runID,
            sessionToken: token,
            expiresAt: Date().addingTimeInterval(15 * 60),
            streamingEnabled: true,
            currentVocabHash: "keyboard",
            voiceWSURL: "/v1/runtime/voice/ws?token=\(token)",
            batchURL: "/v1/runtime/voice/wav",
            maxRecordingSeconds: BuildConfig.maxRecordingSeconds
        )
        let config = VoiceStreamConfig(
            runID: runID,
            selectedModel: SharedStore.selectedModel,
            outputLanguage: SharedStore.outputLanguage,
            safeVocabTerms: SharedStore.safeVocabTerms,
            screenContext: nil
        )
        // Tell the keyboard IMMEDIATELY that the warm app is alive and handling
        // this — before opening the (pre-warmed) socket — so it keeps the in-place
        // recording UI instead of flashing the "open AirNote" handoff. If the
        // socket then fails, dictationFailed corrects it.
        DarwinSignal.shared.post(DarwinSignal.dictationAck)
        do {
            try await streamer.beginStreaming(session: session, config: config)
            isStreaming = true
            extendWarmWindow()
        } catch {
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
        }
    }

    private func stopDictation() async {
        guard isStreaming else { return }
        // Only TRUE silence counts as "didn't catch" — quiet-but-real speech
        // (0.02–0.04) must still go to the server, not flash a false failure.
        if maxLevel < 0.02 {
            await streamer.endStreaming()
            isStreaming = false
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
            extendWarmWindow()
            return
        }
        await streamer.endStreaming()
    }

    private func handle(_ update: VoiceStreamUpdate) {
        switch update {
        case .level(let value):
            maxLevel = max(maxLevel, value)
            publishLiveLevel(value)
            if value > speechLevelThreshold {
                lastLoudAt = Date()
            } else if isStreaming, maxLevel > speechLevelThreshold, let last = lastLoudAt,
                      Date().timeIntervalSince(last) > silenceAutoStop {
                lastLoudAt = nil
                Task { @MainActor in await self.stopDictation() }
            }
        case .interimTranscript(let text), .finalTranscript(let text):
            publishLivePartial(text)
        case .final(let final):
            deliver(transcript: final.transcript, polished: final.polished)
        case .done:
            if isStreaming { isStreaming = false }
            extendWarmWindow()
            prewarm()   // open the next socket now
        case .error:
            isStreaming = false
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
            // If the engine is still warm it was a transient stream error — keep
            // the session; if the engine died (e.g. interruption), end it cleanly
            // so the keyboard falls back to the handoff.
            if streamer.isWarmEngineRunning { extendWarmWindow(); prewarm() } else { endWarmSession() }
        default:
            break
        }
    }

    /// Push a romanized live partial to the keyboard (throttled), so it can show
    /// words as the user speaks during an in-place dictation.
    private func publishLivePartial(_ text: String) {
        let now = Date()
        if now.timeIntervalSince(lastPartialAt) < 0.18 { return }
        lastPartialAt = now
        let roman = HinglishScript.enforceRomanHinglish(text)
        SharedStore.keyboardLivePartial = roman
        DarwinSignal.shared.post(DarwinSignal.livePartial)
    }

    /// Pipe the live mic level (0...1) to the keyboard, throttled to ~20fps, so its
    /// waveform tracks the user's voice during an in-place dictation. The keyboard
    /// extension can't capture audio itself, so the warm app relays the level.
    private func publishLiveLevel(_ level: Float) {
        let now = Date()
        if now.timeIntervalSince(lastLevelAt) < 0.05 { return }
        lastLevelAt = now
        SharedStore.keyboardLiveLevel = Double(max(0, min(1, level)))
        DarwinSignal.shared.post(DarwinSignal.liveLevel)
    }

    private func clearLivePartial() {
        lastPartialAt = .distantPast
        lastLevelAt = .distantPast
        SharedStore.keyboardLivePartial = ""
        SharedStore.keyboardLiveLevel = 0
    }

    private func deliver(transcript: String, polished: String) {
        isStreaming = false
        clearLivePartial()
        let roman = HinglishScript.enforceRomanHinglish(polished.isEmpty ? transcript : polished)
        // Apply the user's taught corrections on-device (the server's streaming
        // path doesn't), using the transcript as evidence.
        let clean = LearnedAliasResolver.apply(
            roman,
            transcript: HinglishScript.enforceRomanHinglish(transcript),
            aliases: SharedStore.learnedAliases
        )
        guard !clean.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
            extendWarmWindow()
            return
        }
        SharedStore.putPendingKeyboardText(clean, at: Date())
        DarwinSignal.shared.post(DarwinSignal.resultReady)
        extendWarmWindow()
        prewarm()   // open the next socket now so the next dictation is instant
        // Persist to history (WS path doesn't server-side) so in-place keyboard
        // dictations also show up + are reviewable for learning.
        let runID = currentRunID
        Task { try? await gateway.syncHistory(clientRunID: runID, transcript: transcript, polished: clean, source: "ios_keyboard") }
    }

    // MARK: Warm window

    private func extendWarmWindow() {
        startHeartbeat()   // keep proving the app is alive while the session is warm
        warmWindowTask?.cancel()
        warmWindowTask = nil

        // "Until I stop it": keep warm with no auto-expiry. We still stamp a
        // far-future warmUntil so the keyboard's `isSessionWarm` check passes;
        // the session ends only on explicit End, interruption, or app kill.
        if neverExpires {
            let until = Date().addingTimeInterval(60 * 60 * 24 * 30)
            SharedStore.sessionWarmUntil = until
            warmUntil = until
            return
        }

        let window = warmWindow
        let until = Date().addingTimeInterval(window)
        SharedStore.sessionWarmUntil = until
        warmUntil = until
        warmWindowTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(window) * 1_000_000_000)
            guard let self, !Task.isCancelled, !self.isStreaming else { return }
            self.endWarmSession()
        }
    }
}
