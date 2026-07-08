import AirNoteShared
import Combine
import Foundation
import UIKit

struct DictationResult: Equatable {
    var transcript: String
    var polished: String
    var latencyMS: Int

    var displayText: String {
        let trimmed = polished.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? transcript : trimmed
    }
}

/// Drives an in-app dictation: requests mic → opens the runtime session →
/// streams 16 kHz audio to the server → surfaces live transcript + polished
/// output. This is the app's primary, fully-working dictation surface.
@MainActor
final class DictationController: ObservableObject {
    enum Phase: Equatable {
        case idle
        case preparing
        case recording
        case processing
        case completed
        case failed
        case micDenied
        case unavailable   // server has no provider credentials yet
    }

    @Published private(set) var phase: Phase = .idle
    @Published private(set) var interim = ""
    @Published private(set) var polishPreview = ""
    @Published private(set) var level: Float = 0
    private var maxLevel: Float = 0
    private var recordingStartedAt: Date?
    private var lastLoudAt: Date?
    /// Auto-stop after this much *continuous* silence once the user has actually
    /// spoken. Generous enough to ride over natural between-sentence pauses so it
    /// never clips words; the user can still tap stop to end sooner.
    var silenceAutoStopSeconds: Double = 4.0
    /// Level above which we consider the user to be speaking (forgiving of quiet
    /// speech so soft talkers aren't cut off mid-word).
    private let speechLevelThreshold: Float = 0.04
    @Published private(set) var lastLatencyMS: Int?
    @Published private(set) var errorMessage: String?
    @Published private(set) var result: DictationResult?

    // Strong reference: the controller is owned by a transient view while the
    // environment lives for the whole app, so this never creates a cycle and is
    // safe to touch from async stream callbacks.
    private let env: AppEnvironment
    private let streamer = VoiceStreamingClient()
    private var session: MobileSessionResponse?
    private var runID = ""
    private var finalizeTimeout: Task<Void, Never>?

    init(env: AppEnvironment) {
        self.env = env
        streamer.onUpdate = { [weak self] update in
            Task { @MainActor in self?.handle(update) }
        }
    }

    deinit {
        // The sheet can be swipe-dismissed without hitting Close — make sure the
        // audio engine / socket are torn down and no timer is left dangling.
        finalizeTimeout?.cancel()
        streamer.onUpdate = nil
        streamer.hardStop()
    }

    var isBusy: Bool {
        switch phase {
        case .preparing, .recording, .processing: return true
        default: return false
        }
    }

    var isRecording: Bool { phase == .recording }

    func toggle() async {
        switch phase {
        case .recording:
            await stop()
        case .idle, .completed, .failed, .micDenied, .unavailable:
            await start()
        case .preparing, .processing:
            break
        }
    }

    func start() async {
        guard !isBusy else { return }
        // Release any warm background mic so this foreground dictation owns it.
        WarmDictationHost.shared.endWarmSession()
        cancelFinalizeTimeout()
        errorMessage = nil
        result = nil
        interim = ""
        polishPreview = ""
        maxLevel = 0
        recordingStartedAt = nil
        lastLoudAt = nil
        phase = .preparing

        // Just-in-time microphone permission (Wispr-style: ask at first use).
        let granted = await env.permissions.requestMic()
        guard granted else {
            phase = .micDenied
            errorMessage = "Enable microphone access in Settings to dictate."
            return
        }

        runID = RequestId.make()
        let request = MobileSessionRequest(
            clientRequestID: runID,
            deviceID: AppInfo.deviceID,
            languageHint: env.outputLanguage == "english" ? .en : .hinglish,
            style: .work,
            keyboardContext: KeyboardContext(beforeText: "", afterText: "", selectedText: "", hostAppLabel: "AirNote", fieldHint: "app"),
            surface: .iosActionButton
        )

        do {
            let session = try await env.gateway.createSession(request)
            self.session = session
            try await streamer.start(session: session, config: env.dictationConfig(runID: runID))
            phase = .recording
            recordingStartedAt = Date()
            env.track(.audioStarted)
        } catch let error as VoiceStreamError {
            failOrUnavailable(error.isCredentialMissing, message: error.message)
        } catch let error as GatewayError {
            if error.isUnauthorized {
                phase = .idle
                env.signOut()
                return
            }
            failOrUnavailable(error.isCredentialMissing, message: error.userMessage)
        } catch {
            failOrUnavailable(false, message: "Couldn't start dictation. Try again.")
        }
    }

    func stop() async {
        guard phase == .recording else { return }
        env.track(.audioStopped)

        // No speech captured? Abort immediately instead of hanging on an empty
        // server round-trip (silence never produces a transcript final).
        let elapsed = recordingStartedAt.map { Date().timeIntervalSince($0) } ?? 0
        if maxLevel < 0.035 || elapsed < 0.4 {
            await streamer.cancel()
            interim = ""
            phase = .failed
            errorMessage = "Didn't catch any speech — tap the mic and speak."
            return
        }

        phase = .processing
        await streamer.stop()
        startFinalizeTimeout()
    }

    /// Guarantees the UI never hangs on "Polishing…": if no final result or error
    /// arrives within the window, surface a recoverable error.
    private func startFinalizeTimeout() {
        finalizeTimeout?.cancel()
        finalizeTimeout = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 14 * 1_000_000_000)
            guard let self, !Task.isCancelled, self.phase == .processing else { return }
            await self.streamer.cancel()
            self.phase = .failed
            self.errorMessage = "That took too long — tap the mic and try again."
        }
    }

    private func cancelFinalizeTimeout() {
        finalizeTimeout?.cancel()
        finalizeTimeout = nil
    }

    func cancel() async {
        await streamer.cancel()
        reset()
    }

    func reset() {
        cancelFinalizeTimeout()
        phase = .idle
        interim = ""
        polishPreview = ""
        result = nil
        errorMessage = nil
        level = 0
    }

    private func failOrUnavailable(_ unavailable: Bool, message: String) {
        cancelFinalizeTimeout()
        if unavailable {
            phase = .unavailable
            errorMessage = "Add your Groq polish key in Settings → Voice keys to turn on dictation."
        } else if message.lowercased().contains("decrypt") {
            // The server has the keys but couldn't decrypt them (its credential key
            // rotated). Re-mirror our local copies to heal the vault for the next
            // dictation; if we hold no local copy, tell the user to re-enter once.
            phase = .failed
            errorMessage = "Your saved voice keys couldn't be read — re-enter them in Settings → Voice keys."
            Task { await env.syncProviderKeysToVault(force: true) }
        } else {
            phase = .failed
            errorMessage = message
        }
    }

    private func handle(_ update: VoiceStreamUpdate) {
        switch update {
        case .level(let value):
            level = value
            maxLevel = max(maxLevel, value)
            // Hands-free: once the user has spoken, auto-stop after a short silence.
            if value > speechLevelThreshold {
                lastLoudAt = Date()
            } else if phase == .recording, maxLevel > speechLevelThreshold, let last = lastLoudAt,
                      Date().timeIntervalSince(last) > silenceAutoStopSeconds {
                lastLoudAt = nil
                Task { [weak self] in await self?.stop() }
            }
        case .status:
            if phase == .recording { /* keep */ }
        case .interimTranscript(let text):
            interim = text
        case .finalTranscript(let text):
            // The speech stream emits a "final" for each finished utterance WHILE the user
            // is still speaking. That does NOT mean recording is over — keep the
            // mic live (and the stop button enabled). Only stop() moves us to
            // .processing once the user actually sends audio.end.
            interim = text
        case .polishStarted:
            polishPreview = ""
            phase = .processing
        case .polishDelta(let token):
            polishPreview += token
            phase = .processing
        case .final(let final):
            finish(transcript: final.transcript, polished: final.polished, latencyMS: final.latencyMS)
        case .done:
            if phase != .completed, !polishPreview.isEmpty {
                finish(transcript: interim, polished: polishPreview, latencyMS: lastLatencyMS ?? 0)
            }
        case .error(let error):
            level = 0
            failOrUnavailable(error.isCredentialMissing, message: error.message)
        case .sessionReady, .guardWarning:
            break
        }
    }

    private func finish(transcript: String, polished: String, latencyMS: Int) {
        cancelFinalizeTimeout()
        // Guarantee Roman Hinglish, then apply the user's taught corrections
        // on-device (the server's streaming path doesn't apply learned aliases).
        let polished = LearnedAliasResolver.apply(
            HinglishScript.enforceRomanHinglish(polished),
            transcript: HinglishScript.enforceRomanHinglish(transcript),
            aliases: SharedStore.learnedAliases
        )
        // Empty result (silence / no speech) — surface a friendly retry, don't
        // "complete" with blank text.
        let combined = polished.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? transcript.trimmingCharacters(in: .whitespacesAndNewlines)
            : polished
        if combined.isEmpty {
            phase = .failed
            errorMessage = "Didn't catch any speech — tap the mic and try again."
            return
        }
        let value = DictationResult(transcript: transcript, polished: polished, latencyMS: latencyMS)
        result = value
        polishPreview = polished
        lastLatencyMS = latencyMS
        level = 0
        phase = .completed
        UIPasteboard.general.string = value.displayText
        // The WS voice path doesn't write history server-side — persist it from the
        // client so it shows in History (and can be reviewed for learning).
        let runID = self.runID
        Task {
            try? await env.gateway.syncHistory(clientRunID: runID, transcript: transcript, polished: polished, source: "ios_app")
            await env.refreshHistory()
        }
        // Re-arm the warm mic so the keyboard can dictate in-place next (this
        // foreground dictation released it via start()).
        WarmDictationHost.shared.warmUp()
    }
}
