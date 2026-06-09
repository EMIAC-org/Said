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
final class WarmDictationHost {
    static let shared = WarmDictationHost()

    private let streamer = VoiceStreamingClient()
    private var warmWindowTask: Task<Void, Never>?
    private var isStreaming = false
    private var maxLevel: Float = 0
    private var lastLoudAt: Date?
    private var observing = false

    /// How long the mic stays warm after the last dictation.
    private let warmWindow: TimeInterval = 90
    private let silenceAutoStop: TimeInterval = 2.6
    private let speechLevelThreshold: Float = 0.05

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
            } catch {
                SharedStore.sessionWarmUntil = nil
            }
        }
    }

    func endWarmSession() {
        warmWindowTask?.cancel()
        warmWindowTask = nil
        isStreaming = false
        SharedStore.sessionWarmUntil = nil
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
        SharedStore.pendingKeyboardText = nil
        let runID = RequestId.make()
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
            safeVocabTerms: [],
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
        case .final(let final):
            deliver(transcript: final.transcript, polished: final.polished)
        case .done:
            if isStreaming { isStreaming = false }
            extendWarmWindow()
        case .error:
            isStreaming = false
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
            extendWarmWindow()
        default:
            break
        }
    }

    private func deliver(transcript: String, polished: String) {
        isStreaming = false
        let clean = HinglishScript.enforceRomanHinglish(polished.isEmpty ? transcript : polished)
        guard !clean.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            DarwinSignal.shared.post(DarwinSignal.dictationFailed)
            extendWarmWindow()
            return
        }
        SharedStore.putPendingKeyboardText(clean, at: Date())
        DarwinSignal.shared.post(DarwinSignal.resultReady)
        extendWarmWindow()
    }

    // MARK: Warm window

    private func extendWarmWindow() {
        SharedStore.sessionWarmUntil = Date().addingTimeInterval(warmWindow)
        warmWindowTask?.cancel()
        warmWindowTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(self?.warmWindow ?? 90) * 1_000_000_000)
            guard let self, !Task.isCancelled, !self.isStreaming else { return }
            self.endWarmSession()
        }
    }
}
