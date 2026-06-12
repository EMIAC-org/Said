import AVFoundation
import UIKit
import AirNoteShared

/// AirNote's custom keyboard. With Full Access it records and streams audio
/// directly to the server (no dependency on the main app being open), then
/// inserts the polished text at the cursor.
final class KeyboardViewController: UIInputViewController {
    private let streamer = VoiceStreamingClient()
    private let pasteboard = UIPasteboard.general
    private lazy var gateway = GatewayEnvironment.makeClient()

    private var state: KeyboardState = .ready
    private var pad: RecordingPadView?
    private var currentResult: BridgeResult?
    private var resultSeq: UInt64 = 0
    private var isRecording = false
    private var finalizeTimer: Timer?
    private var warmActive = false
    private var gotAck = false
    private var ackTimer: Timer?

    // MARK: In-place "Teach a fix"
    //
    // iOS won't let the keyboard read other apps' text in general, but while
    // AirNote's keyboard IS the active keyboard it can read the text around the
    // cursor (textDocumentProxy). So after we insert a result, if the user fixes
    // a word using this keyboard, we can diff what we inserted against the
    // corrected text and teach it — the only correction path iOS permits outside
    // the app. Edits made after switching keyboards/apps remain invisible.
    private var lastInsertedText: String?
    private var insertPrefix: String?
    private var teachExpiry: Date?
    private var teachTimer: Timer?
    /// How long the "Teach a fix" affordance stays available after an insertion.
    private let teachWindow: TimeInterval = 120

    private var canTeachFix: Bool {
        guard lastInsertedText != nil, let expiry = teachExpiry else { return false }
        return expiry > Date()
    }

    // MARK: Lifecycle

    override func viewDidLoad() {
        super.viewDidLoad()
        streamer.onUpdate = { [weak self] update in
            self?.handle(update)
        }
        DarwinSignal.shared.observe(DarwinSignal.dictationAck) { [weak self] in
            self?.gotAck = true
            self?.ackTimer?.invalidate()
        }
        DarwinSignal.shared.observe(DarwinSignal.resultReady) { [weak self] in
            self?.handleWarmResult()
        }
        DarwinSignal.shared.observe(DarwinSignal.dictationFailed) { [weak self] in
            self?.handleWarmFailed()
        }
        DarwinSignal.shared.observe(DarwinSignal.livePartial) { [weak self] in
            self?.handleLivePartial()
        }
        reportHealth()
        recomputeIdleState()
        render()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        reportHealth()
        // Returning from an app-handoff dictation? Insert the result the app left
        // for us, then we're done.
        if consumePendingDictation() { return }
        if !isRecording { recomputeIdleState(); render() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // Tear down whether recording OR mid-polish (.processing) — the system can
        // unload the keyboard process the instant it disappears, so release audio
        // + the socket now, synchronously.
        isRecording = false
        warmActive = false
        ackTimer?.invalidate()
        ackTimer = nil
        finalizeTimer?.invalidate()
        finalizeTimer = nil
        teachTimer?.invalidate()
        teachTimer = nil
        streamer.hardStop()
    }

    deinit {
        // Clear the callback first so no event can fire on a deallocating self,
        // then tear down synchronously.
        finalizeTimer?.invalidate()
        teachTimer?.invalidate()
        streamer.onUpdate = nil
        streamer.hardStop()
    }

    override func textDidChange(_ textInput: UITextInput?) {
        // Re-evaluate the secure-field gate when focus moves, but never disturb
        // an in-flight recording or a pending result.
        guard !isRecording, currentResult == nil else { return }
        // While a fresh insertion is still teachable, hold the post-insert state
        // so the "Teach a fix" affordance stays put as the user edits the text.
        if canTeachFix { return }
        // Only re-render when the secure-field gate actually flips — re-rendering
        // on every keystroke would needlessly churn the voice surface.
        let previous = state
        recomputeIdleState()
        if state != previous { render(animated: true) }
    }

    // MARK: Health handshake (lets the main app know the keyboard is enabled + Full Access)

    private func reportHealth() {
        SharedStore.recordKeyboardHealth(hasFullAccess: hasFullAccess, at: Date())
    }

    // MARK: State

    private func recomputeIdleState() {
        if !hasFullAccess {
            state = .needsFullAccess
        } else if SharedStore.accessToken == nil {
            state = .needsMainAppSession   // "Open AirNote to sign in"
        } else if TextInsertion(documentProxy: textDocumentProxy).isUnsupportedSecureField {
            state = .unsupportedSecureField
        } else {
            state = .ready
        }
    }

    private func render(animated: Bool = false) {
        // Build the pad once; afterwards only the voice surface is swapped, so the
        // keys never flash and state changes settle in with a soft spring. If the
        // pad ever got detached from the view (system view reload), rebuild it so
        // the keyboard can never end up blank.
        if let pad, pad.superview === view {
            pad.update(state: state, canTeachFix: canTeachFix, animated: animated)
            return
        }
        view.subviews.forEach { $0.removeFromSuperview() }
        let pad = RecordingPadView(state: state, canTeachFix: canTeachFix)
        pad.translatesAutoresizingMaskIntoConstraints = false
        pad.onStart = { [weak self] in self?.requestAppDictation() }
        pad.onStop = { [weak self] in self?.stopRecording() }
        pad.onInsert = { [weak self] in self?.insertResult() }
        pad.onCopy = { [weak self] in self?.copyResult() }
        pad.onSave = { [weak self] in self?.saveResult() }
        pad.onTeachFix = { [weak self] in self?.teachLastFix() }
        pad.onOpenApp = { [weak self] in self?.openMainApp() }
        pad.onKeyTap = { [weak self] text in self?.textDocumentProxy.insertText(text) }
        pad.onDelete = { [weak self] in self?.textDocumentProxy.deleteBackward() }
        pad.onNextKeyboard = { [weak self] in self?.advanceToNextInputMode() }
        view.addSubview(pad)
        NSLayoutConstraint.activate([
            pad.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            pad.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            pad.topAnchor.constraint(equalTo: view.topAnchor),
            pad.bottomAnchor.constraint(equalTo: view.bottomAnchor)
        ])
        self.pad = pad
    }

    private func setState(_ newState: KeyboardState) {
        guard newState != state else { return }
        state = newState
        render(animated: true)
    }

    // MARK: Recording

    private func startRecording() {
        cancelFinalizeTimer()
        clearTeachable()
        guard hasFullAccess else { setState(.needsFullAccess); return }
        guard let token = SharedStore.accessToken, !token.isEmpty else {
            setState(.needsMainAppSession)
            return
        }
        // The keyboard cannot itself prompt for the mic — only the container app
        // can. If it isn't granted yet, send the user to AirNote to grant it.
        guard AVAudioApplication.shared.recordPermission == .granted else {
            setState(.error("Open AirNote once to allow the microphone."))
            return
        }

        currentResult = nil
        isRecording = true
        setState(.recording)

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
            safeVocabTerms: SharedStore.safeVocabTerms,
            screenContext: ContextReader(documentProxy: textDocumentProxy).read().fieldHint
        )

        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.streamer.start(session: session, config: config)
            } catch let error as VoiceStreamError {
                await MainActor.run { self.handleStreamError(error) }
            } catch {
                await MainActor.run {
                    self.isRecording = false
                    self.setState(.error("Couldn't start recording. Try again."))
                }
            }
        }
    }

    private func stopRecording() {
        if warmActive {
            setState(.processing("Polishing"))
            DarwinSignal.shared.post(DarwinSignal.stopDictation)
            startFinalizeTimer()   // result-ready safety net
            return
        }
        guard isRecording else { return }
        isRecording = false
        setState(.processing("Polishing"))
        startFinalizeTimer()
        Task { await streamer.stop() }
    }

    /// Never let the keyboard hang on "Polishing" if the server goes silent.
    private func startFinalizeTimer() {
        finalizeTimer?.invalidate()
        finalizeTimer = Timer.scheduledTimer(withTimeInterval: 14, repeats: false) { [weak self] _ in
            guard let self else { return }
            if case .processing = self.state {
                self.warmActive = false
                self.streamer.hardStop()
                self.currentResult = nil
                self.setState(.error("That took too long — tap the mic and try again."))
            }
        }
    }

    private func cancelFinalizeTimer() {
        finalizeTimer?.invalidate()
        finalizeTimer = nil
    }

    // MARK: Streaming updates

    private func handle(_ update: VoiceStreamUpdate) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            switch update {
            case .interimTranscript:
                if self.isRecording { self.setState(.recording) }
            case .finalTranscript:
                // A per-utterance final mid-recording doesn't mean the user is
                // done — keep recording until they tap stop (which sets .processing).
                if !self.isRecording { self.setState(.processing("Polishing")) }
            case .polishStarted, .polishDelta:
                self.setState(.processing("Polishing"))
            case .final(let final):
                self.finish(transcript: final.transcript, polished: final.polished)
            case .done:
                break
            case .error(let error):
                self.handleStreamError(error)
            case .status, .sessionReady, .guardWarning, .level:
                break
            }
        }
    }

    private func finish(transcript: String, polished: String) {
        cancelFinalizeTimer()
        isRecording = false
        resultSeq += 1
        // Guarantee Roman Hinglish before inserting into the host app (including
        // the raw-transcript fallback when polish came back empty).
        let polished = HinglishScript.enforceRomanHinglish(polished)
        let transcript = HinglishScript.enforceRomanHinglish(transcript)
        let secure = TextInsertion(documentProxy: textDocumentProxy).isUnsupportedSecureField
        let result = BridgeResult(
            resultSeq: resultSeq,
            sessionID: "keyboard",
            clientRequestID: RequestId.make(),
            requestID: RequestId.make(),
            state: .final,
            transcript: transcript,
            polished: polished.isEmpty ? transcript : polished,
            language: SharedStore.outputLanguage == "english" ? .en : .hinglish,
            style: .work,
            latencyMS: 0,
            expiresAt: Date().addingTimeInterval(10 * 60),
            insertPolicy: secure ? .copyOnly : .insertAtCursor,
            learningAllowed: true
        )
        currentResult = result
        setState(secure ? .secureCopyReady(result) : .insertReady(result))
    }

    private func handleStreamError(_ error: VoiceStreamError) {
        cancelFinalizeTimer()
        isRecording = false
        currentResult = nil
        if error.isCredentialMissing {
            setState(.error("Dictation isn't set up on this workspace yet."))
        } else {
            setState(.error(error.message))
        }
    }

    // MARK: Result actions

    private func insertResult() {
        guard let result = currentResult else { return }
        if case .secureCopyReady = state {
            copyResult()
            return
        }
        let prefix = textDocumentProxy.documentContextBeforeInput ?? ""
        let inserter = TextInsertion(documentProxy: textDocumentProxy)
        if inserter.insert(result) {
            currentResult = nil
            captureTeachable(insertedText: result.polished, prefix: prefix)
            setState(.inserted)
        } else {
            copyResult()
        }
    }

    private func copyResult() {
        guard let result = currentResult else { return }
        pasteboard.string = result.polished
        currentResult = nil
        setState(.copied)
    }

    private func saveResult() {
        // The server already persists completed runtime sessions to history, so
        // "save" simply acknowledges and clears the pending result.
        currentResult = nil
        setState(.savedToHistory)
    }

    // MARK: In-place "Teach a fix"

    private func captureTeachable(insertedText: String, prefix: String) {
        let trimmed = insertedText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { clearTeachable(); return }
        lastInsertedText = insertedText
        insertPrefix = prefix
        teachExpiry = Date().addingTimeInterval(teachWindow)
        teachTimer?.invalidate()
        teachTimer = Timer.scheduledTimer(withTimeInterval: teachWindow, repeats: false) { [weak self] _ in
            guard let self else { return }
            self.clearTeachable()
            // Drop the now-stale "Teach a fix" affordance if we're still idle.
            if !self.isRecording, self.currentResult == nil {
                self.recomputeIdleState()
                self.render()
            }
        }
    }

    private func clearTeachable() {
        lastInsertedText = nil
        insertPrefix = nil
        teachExpiry = nil
        teachTimer?.invalidate()
        teachTimer = nil
    }

    /// Diff what we inserted against the text the user edited in-place (read via
    /// textDocumentProxy while AirNote's keyboard is active) and teach the fix.
    private func teachLastFix() {
        guard let inserted = lastInsertedText else { return }
        let currentBefore = textDocumentProxy.documentContextBeforeInput ?? ""
        let edited: String
        switch TeachFixDiff.evaluate(
            insertedText: inserted,
            insertPrefix: insertPrefix ?? "",
            currentBeforeCursor: currentBefore
        ) {
        case .empty:
            setState(.learned("Fix the text first, then tap Teach a fix."))
            return
        case .unchanged:
            setState(.learned("No change to teach — it looks the same."))
            return
        case .edited(let corrected):
            edited = corrected
        }
        let original = inserted.trimmingCharacters(in: .whitespacesAndNewlines)

        setState(.teaching)
        let recordingID = RequestId.make()
        Task { [weak self] in
            guard let self else { return }
            do {
                let analysis = try await self.gateway.analyzeEdit(
                    recordingID: recordingID,
                    transcript: original,
                    aiOutput: original,
                    userKept: edited
                )
                let learnable = analysis.candidates.filter(\.learnable)
                guard !learnable.isEmpty else {
                    await MainActor.run {
                        self.clearTeachable()
                        self.setState(.learned("Nothing new to teach in that edit."))
                    }
                    return
                }
                let result = try await self.gateway.confirmLearning(recordingID: recordingID, items: learnable)
                await MainActor.run {
                    self.clearTeachable()
                    if result.learnedCount > 0 {
                        // Cache heard→meant locally so it's applied to future output.
                        for candidate in learnable {
                            SharedStore.addLearnedAlias(heard: candidate.original, correct: candidate.corrected)
                        }
                        let terms = result.learnedTerms.isEmpty
                            ? "\(result.learnedCount) correction\(result.learnedCount == 1 ? "" : "s")"
                            : result.learnedTerms.joined(separator: ", ")
                        self.setState(.learned("Learned \(terms) — AirNote will use it next time."))
                    } else if result.blockedCount > 0 {
                        self.setState(.learned("That's too common to learn as a custom word."))
                    } else {
                        self.setState(.learned("Nothing new to teach here."))
                    }
                }
            } catch {
                await MainActor.run {
                    // Keep the insertion teachable so the user can retry.
                    self.setState(.learned("Couldn't teach that just now — try again."))
                }
            }
        }
    }

    // MARK: App-handoff dictation
    //
    // iOS does not permit microphone capture inside a keyboard extension (the OS
    // denies AVAudioEngine.start with "extension doesn't have entitlements to
    // record audio"). So we ask the main app to record; it polishes the text and
    // leaves it in the App Group for us to insert when the user swipes back.

    private func requestAppDictation() {
        cancelFinalizeTimer()
        clearTeachable()
        guard hasFullAccess else { setState(.needsFullAccess); return }
        guard let token = SharedStore.accessToken, !token.isEmpty else {
            setState(.needsMainAppSession)
            return
        }
        SharedStore.clearKeyboardDictation()
        SharedStore.keyboardDictationRequestedAt = Date()
        currentResult = nil

        if SharedStore.isSessionWarm {
            // In-place: the app is holding the mic warm in the background. Signal
            // it and record right here — no app switch.
            warmActive = true
            gotAck = false
            setState(.recording)
            DarwinSignal.shared.post(DarwinSignal.startDictation)
            ackTimer?.invalidate()
            ackTimer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: false) { [weak self] _ in
                guard let self, self.warmActive, !self.gotAck else { return }
                // The warm app was suspended after all — fall back to the handoff.
                self.warmActive = false
                self.coldHandoff()
            }
        } else {
            coldHandoff()
        }
    }

    private func coldHandoff() {
        // The request timestamp is already written by requestAppDictation. iOS
        // usually won't let a keyboard open its app, so this is best-effort; if it
        // doesn't open, the user opens AirNote and it auto-starts from the
        // pending request. Either way, the first dictation establishes the warm
        // session and the rest are in-place.
        setState(.dictatingInApp)
        openURLInApp("airnote://dictate")
    }

    private func handleWarmResult() {
        ackTimer?.invalidate()
        warmActive = false
        cancelFinalizeTimer()
        if !consumePendingDictation() {
            recomputeIdleState()
            render()
        }
    }

    private func handleWarmFailed() {
        ackTimer?.invalidate()
        cancelFinalizeTimer()
        warmActive = false
        setState(.error("Didn't catch that — tap the mic and speak."))
    }

    /// Show the live (already romanized) transcript the warm app is producing, so
    /// the user sees words appear as they speak — without a full re-render.
    private func handleLivePartial() {
        guard warmActive, case .recording = state else { return }
        let text = SharedStore.keyboardLivePartial
        if !text.isEmpty { pad?.setLivePartial(text) }
    }

    /// On returning to the keyboard, insert any polished text the app produced for
    /// a request newer than ours. Returns true if a result was handled.
    @discardableResult
    private func consumePendingDictation() -> Bool {
        guard
            let raw = SharedStore.pendingKeyboardText, !raw.isEmpty,
            let producedAt = SharedStore.pendingKeyboardTextAt,
            let requestedAt = SharedStore.keyboardDictationRequestedAt,
            producedAt >= requestedAt
        else { return false }

        SharedStore.clearKeyboardDictation()
        let text = HinglishScript.enforceRomanHinglish(raw)
        let secure = TextInsertion(documentProxy: textDocumentProxy).isUnsupportedSecureField
        resultSeq += 1
        let result = BridgeResult(
            resultSeq: resultSeq,
            sessionID: "keyboard",
            clientRequestID: RequestId.make(),
            requestID: RequestId.make(),
            state: .final,
            transcript: text,
            polished: text,
            language: SharedStore.outputLanguage == "english" ? .en : .hinglish,
            style: .work,
            latencyMS: 0,
            expiresAt: Date().addingTimeInterval(10 * 60),
            insertPolicy: secure ? .copyOnly : .insertAtCursor,
            learningAllowed: true
        )
        currentResult = result

        if secure {
            setState(.secureCopyReady(result))
        } else {
            // Capture the text already before the cursor so a later "Teach a fix"
            // can isolate the user's edited version of just our insertion.
            let prefix = textDocumentProxy.documentContextBeforeInput ?? ""
            if TextInsertion(documentProxy: textDocumentProxy).insert(result) {
                currentResult = nil
                captureTeachable(insertedText: text, prefix: prefix)
                setState(.inserted)
            } else {
                // Insertion failed (no proxy / restricted) — fall back to clipboard.
                pasteboard.string = text
                currentResult = nil
                setState(.copied)
            }
        }
        return true
    }

    // MARK: Open container app

    private func openMainApp() {
        openURLInApp("airnote://open")
    }

    @discardableResult
    private func openURLInApp(_ string: String) -> Bool {
        guard let url = URL(string: string) else { return false }
        // Keyboard extensions are built with APPLICATION_EXTENSION_API_ONLY, so
        // UIApplication.open isn't callable directly. Walk the responder chain and
        // dynamically invoke an open selector — the standard extension hack. Try
        // the modern open:options:completionHandler: first, then legacy openURL:.
        let openURL = NSSelectorFromString("openURL:")
        let openOptions = NSSelectorFromString("open:options:completionHandler:")
        var responder: UIResponder? = self
        while let current = responder {
            if let app = current as? UIApplication {
                app.perform(openURL, with: url)
                NSLog("AirNoteKeyboard: opened via UIApplication in responder chain")
                return true
            }
            if current.responds(to: openOptions) {
                _ = current.perform(openOptions, with: url, with: [:])
                NSLog("AirNoteKeyboard: opened via open:options: on %@", String(describing: type(of: current)))
                return true
            }
            if current.responds(to: openURL) {
                current.perform(openURL, with: url)
                NSLog("AirNoteKeyboard: opened via openURL: on %@", String(describing: type(of: current)))
                return true
            }
            responder = current.next
        }
        NSLog("AirNoteKeyboard: NO responder could open %@ — manual open required", string)
        return false
    }
}
