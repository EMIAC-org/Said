import AirNoteShared
import Foundation
import UIKit

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

    private let streamer = VoiceStreamingClient()
    private let gateway = GatewayEnvironment.makeClient()
    private var warmWindowTask: Task<Void, Never>?
    private var currentRunID = ""
    private var isStreaming = false
    private var maxLevel: Float = 0
    private var lastLoudAt: Date?
    private var lastPartialAt = Date.distantPast
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
    }

    /// Called (while foreground) right after a handoff dictation completes, to
    /// keep the mic warm for in-place dictations. Best-effort: if it fails, the
    /// keyboard simply falls back to the foreground handoff.
    func warmUp() {
        Task { @MainActor in
            do {
                try await streamer.startWarmEngine()
                extendWarmWindow()
                prewarm()
            } catch {
                SharedStore.sessionWarmUntil = nil
            }
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
        isStreaming = false
        SharedStore.sessionWarmUntil = nil
        warmUntil = nil
        streamer.stopWarmEngine()
    }

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
        do {
            try await streamer.beginStreaming(session: session, config: config)
            isStreaming = true
            extendWarmWindow()
            DarwinSignal.shared.post(DarwinSignal.dictationAck)
        } catch {
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
        }
    }

    private func stopDictation() async {
        guard isStreaming else { return }
        if maxLevel < 0.035 {
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

    private func clearLivePartial() {
        lastPartialAt = .distantPast
        SharedStore.keyboardLivePartial = ""
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
