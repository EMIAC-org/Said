import AVFoundation
import UIKit
import AirNoteShared

/// AirNote's custom keyboard. With Full Access it records and streams audio
/// directly to the server (no dependency on the main app being open), then
/// inserts the polished text at the cursor.
final class KeyboardViewController: UIInputViewController {
    private let streamer = VoiceStreamingClient()
    private let pasteboard = UIPasteboard.general

    private var state: KeyboardState = .ready
    private var currentResult: BridgeResult?
    private var resultSeq: UInt64 = 0
    private var isRecording = false

    // MARK: Lifecycle

    override func viewDidLoad() {
        super.viewDidLoad()
        streamer.onUpdate = { [weak self] update in
            self?.handle(update)
        }
        reportHealth()
        recomputeIdleState()
        render()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        reportHealth()
        if !isRecording { recomputeIdleState(); render() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        if isRecording {
            isRecording = false
            // Synchronous teardown — the system can unload the keyboard process
            // the instant it disappears, so we must release audio + the socket
            // now, not in a detached Task that might never run.
            streamer.hardStop()
        }
    }

    deinit {
        // Clear the callback first so no event can fire on a deallocating self,
        // then tear down synchronously.
        streamer.onUpdate = nil
        streamer.hardStop()
    }

    override func textDidChange(_ textInput: UITextInput?) {
        // Re-evaluate the secure-field gate when focus moves, but never disturb
        // an in-flight recording or a pending result.
        guard !isRecording, currentResult == nil else { return }
        recomputeIdleState()
        render()
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

    private func render() {
        view.subviews.forEach { $0.removeFromSuperview() }
        let pad = RecordingPadView(state: state)
        pad.translatesAutoresizingMaskIntoConstraints = false
        pad.onStart = { [weak self] in self?.startRecording() }
        pad.onStop = { [weak self] in self?.stopRecording() }
        pad.onInsert = { [weak self] in self?.insertResult() }
        pad.onCopy = { [weak self] in self?.copyResult() }
        pad.onSave = { [weak self] in self?.saveResult() }
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
    }

    private func setState(_ newState: KeyboardState) {
        guard newState != state else { return }
        state = newState
        render()
    }

    // MARK: Recording

    private func startRecording() {
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
            safeVocabTerms: [],
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
        guard isRecording else { return }
        isRecording = false
        setState(.processing("Polishing"))
        Task { await streamer.stop() }
    }

    // MARK: Streaming updates

    private func handle(_ update: VoiceStreamUpdate) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            switch update {
            case .interimTranscript:
                if self.isRecording { self.setState(.recording) }
            case .finalTranscript:
                self.setState(.processing("Polishing"))
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
        isRecording = false
        resultSeq += 1
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
        let inserter = TextInsertion(documentProxy: textDocumentProxy)
        if inserter.insert(result) {
            currentResult = nil
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

    // MARK: Open container app

    private func openMainApp() {
        guard let url = URL(string: "airnote://open") else { return }
        // Keyboard extensions are built with APPLICATION_EXTENSION_API_ONLY, so
        // UIApplication.open isn't callable directly. Walk the responder chain and
        // invoke the openURL: selector dynamically — the standard extension hack.
        let selector = NSSelectorFromString("openURL:")
        var responder: UIResponder? = self
        while let current = responder {
            if current.responds(to: selector) {
                current.perform(selector, with: url)
                return
            }
            responder = current.next
        }
    }
}
