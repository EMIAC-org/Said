import AppKit
import Foundation
import os

final class DictationEngine: ObservableObject {
    let recorder = AudioRecorder()
    let hotkey = HotkeyManager()
    let notchVM = NotchViewModel()

    private var sidecar: SidecarManager
    private var backendClient: BackendClient
    private var sseClient = SSEClient()
    private var dgStream = DeepgramStream()
    private let logger = RuntimeLogger(category: "dictation")

    private var deepgramKey: String = ""
    private var sttMode: String = "multi"
    private var sttKeyterms: [String] = []
    private var sttReplacements: [ReplacementRule] = []
    private var recordingStart: ContinuousClock.Instant?
    private var latestResult: String?
    private var activeTextPolishTask: Task<Void, Never>?
    private let stateLock = NSLock()
    private var pipelineState: PipelineState = .idle

    private let minRecordingMs = 400

    private enum PipelineState: Equatable {
        case idle
        case recording
        case processing
    }

    init(sidecar: SidecarManager) {
        self.sidecar = sidecar
        self.backendClient = BackendClient(sidecar: sidecar)

        hotkey.onPress = { [weak self] in self?.startRecording() }
        hotkey.onRelease = { [weak self] in self?.stopRecording() }
        hotkey.onShortcut = { [weak self] n in self?.handleShortcut(n) }
        hotkey.onPasteLatest = { [weak self] in self?.pasteLatest() }
        recorder.onLevelUpdate = { [weak self] level in
            DispatchQueue.main.async { self?.notchVM.audioLevel = level }
        }
    }

    func configure(deepgramKey: String, sttMode: String, hotkey: RecordHotkey) {
        self.deepgramKey = deepgramKey
        self.sttMode = sttMode
        self.hotkey.activeKey = hotkey
    }

    func start() {
        hotkey.start()
        logger.info("dictation engine started (hotkey=\(self.hotkey.activeKey.rawValue))")
        fetchSTTBias()
    }

    func refreshPermissionDependentServices() {
        if PermissionHelper.inputMonitoringGranted {
            hotkey.start()
        }
    }

    private func fetchSTTBias() {
        Task {
            while !sidecar.isHealthy {
                try? await Task.sleep(for: .milliseconds(300))
            }
            do {
                let bias = try await backendClient.getSTTBias()
                sttKeyterms = bias.keyterms
                sttReplacements = bias.replacements
                logger.info("STT bias loaded: \(bias.keyterms.count) keyterms, \(bias.replacements.count) replacements")
            } catch {
                logger.warning("STT bias fetch failed: \(error)")
            }
        }
    }

    // MARK: - Recording

    func startRecording() {
        guard beginRecordingState() else {
            logger.info("recording ignored — dictation pipeline is busy (\(currentPipelineState()))")
            return
        }
        recordingStart = ContinuousClock.now
        DispatchQueue.main.async { self.notchVM.startRecording() }
        guard recorder.start() else {
            recordingStart = nil
            if dgStream.isConnected { dgStream.disconnect() }
            setPipelineIdle()
            DispatchQueue.main.async { self.notchVM.showError("Microphone did not start") }
            return
        }

        if !deepgramKey.isEmpty {
            let connected = dgStream.connect(
                apiKey: deepgramKey, sttMode: sttMode,
                keyterms: sttKeyterms, replacements: sttReplacements
            )
            if connected {
                recorder.onChunk = { [weak self] pcm in
                    self?.dgStream.sendAudio(pcm)
                }
            }
        }

        logger.info("[timing] recording started")
    }

    func stopRecording() {
        guard recorder.isRecording else { return }
        let pipelineStart = ContinuousClock.now
        let recDuration = ms(since: recordingStart ?? pipelineStart)

        // Short recording cancellation
        if recDuration < minRecordingMs {
            recorder.onChunk = nil
            _ = recorder.stop()
            if dgStream.isConnected { dgStream.disconnect() }
            let keyLabel = hotkey.activeKey.label
            DispatchQueue.main.async {
                self.notchVM.showError("Hold \(keyLabel) to record")
            }
            setPipelineIdle()
            logger.info("[timing] short recording cancelled (\(recDuration)ms)")
            return
        }

        guard beginProcessingState() else {
            logger.info("stop ignored — dictation pipeline is not recording")
            return
        }

        recorder.onChunk = nil
        if dgStream.isConnected {
            dgStream.sendCloseStream()
        }

        let wavStart = ContinuousClock.now
        let wav = recorder.stop()
        let wavMs = ms(since: wavStart)
        logger.info("[timing] rec=\(recDuration)ms wav=\(wavMs)ms (\(wav.count / 1024)KB)")

        DispatchQueue.main.async { self.notchVM.startProcessing(phase: "Transcribing") }

        let hasDG = !deepgramKey.isEmpty && dgStream.isConnected

        Task.detached { [weak self] in
            guard let self else { return }
            defer { self.setPipelineIdle() }

            var streamingTranscript: StreamingTranscript? = nil

            if hasDG {
                let currentPlain = self.dgStream.plainTranscript
                if !currentPlain.isEmpty {
                    let client = self.backendClient
                    Task.detached { await client.preEmbed(text: currentPlain) }
                }

                streamingTranscript = await self.dgStream.waitForClose(timeoutMs: 3000)
                self.dgStream.disconnect()

                if let st = streamingTranscript {
                    self.logger.info("[timing] transcript (\(st.meta.word_count) words, conf=\(String(format: "%.2f", st.meta.mean_word_confidence)), low=\(st.meta.low_confidence_count)): \(st.transcript.prefix(80))")
                    await MainActor.run {
                        self.notchVM.setProcessingTranscript(st.transcript, phase: "Enhancing")
                    }
                } else {
                    await MainActor.run {
                        self.notchVM.setProcessingPhase("Enhancing")
                    }
                }
            } else {
                await MainActor.run {
                    self.notchVM.setProcessingPhase("Enhancing")
                }
            }

            let backendStart = ContinuousClock.now
            self.logger.info("[timing] → backend (pre_transcript=\(streamingTranscript != nil), wav=\(wav.count / 1024)KB)")
            await self.polishAndPaste(
                wav: wav, streamingTranscript: streamingTranscript,
                backendStart: backendStart, pipelineStart: pipelineStart
            )
        }
    }

    // MARK: - Polish + Paste

    private func polishAndPaste(
        wav: Data, streamingTranscript: StreamingTranscript?,
        backendStart: ContinuousClock.Instant, pipelineStart: ContinuousClock.Instant
    ) async {
        let request = backendClient.buildPolishRequest(wav: wav, streamingTranscript: streamingTranscript)

        var fullText = ""
        var finalDone: PolishDone?
        var typedAny = false
        var tokenCount = 0
        var failCount = 0
        var liveTypingDisabled = false
        var firstTokenTime: ContinuousClock.Instant?
        var typingTotalUs: UInt64 = 0

        do {
            for try await token in sseClient.streamTokens(request: request) {
                if token.done {
                    finalDone = token.final
                    break
                }

                // LiveTypingGuard: detect stream reset sentinel
                if token.text == STREAM_RESET_SENTINEL {
                    liveTypingDisabled = true
                    logger.warning("stream reset detected — disabling HID typing, will paste at end")
                    continue
                }

                if firstTokenTime == nil {
                    firstTokenTime = ContinuousClock.now
                    let ttft = ms(since: backendStart)
                    let ttftTotal = ms(since: pipelineStart)
                    logger.info("[timing] TTFT: \(ttft)ms from backend, \(ttftTotal)ms from key-release")
                }

                fullText += token.text
                let t = token.text
                await MainActor.run { notchVM.appendToken(t) }

                if liveTypingDisabled {
                    failCount += 1
                    continue
                }

                let typeStart = ContinuousClock.now
                let typed = TextPaster.typeText(t)
                let typeUs = us(since: typeStart)
                typingTotalUs += typeUs

                if typed {
                    if !typedAny {
                        logger.info("[timing] first HID type: \(typeUs)µs for: \(t)")
                    }
                    typedAny = true
                    tokenCount += 1
                } else {
                    failCount += 1
                }
            }

            let authoritative = finalDone?.polished ?? fullText
            let trimmed = authoritative.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty {
                await MainActor.run { notchVM.showError("No text returned") }
                return
            }

            let streamMs = ms(since: backendStart)
            logger.info("[timing] SSE done: \(streamMs)ms, \(tokenCount) tokens, typing=\(typingTotalUs / 1000)ms")

            await MainActor.run { notchVM.setProcessingPhase("Pasting") }

            if typedAny {
                let streamedTrimmed = fullText.trimmingCharacters(in: .whitespacesAndNewlines)
                let finalChanged = trimmed != streamedTrimmed
                if failCount > 0 || liveTypingDisabled || finalChanged {
                    logger.warning("[timing] safety paste: \(tokenCount) ok, \(failCount) failed, final_changed=\(finalChanged)")
                    TextPaster.pasteReplacing(trimmed)
                }
            } else {
                TextPaster.paste(trimmed)
            }

            latestResult = trimmed
            let totalMs = ms(since: pipelineStart)
            logger.info("[timing] ✓ TOTAL: \(totalMs)ms — \(trimmed.count) chars")
            await MainActor.run { notchVM.finish(text: trimmed) }
        } catch {
            let totalMs = ms(since: pipelineStart)
            let rawMessage = error.localizedDescription
            let message = Self.humanize(rawMessage)
            logger.error("[timing] ✗ FAILED at \(totalMs)ms: \(message) — \(rawMessage)")
            let trimmed = fullText.trimmingCharacters(in: .whitespacesAndNewlines)
            if typedAny && failCount > 0 && !trimmed.isEmpty {
                TextPaster.pasteReplacing(trimmed)
            } else if !trimmed.isEmpty && !typedAny {
                TextPaster.paste(trimmed)
            }
            await MainActor.run { notchVM.showError(message) }
        }
    }

    // MARK: - Ctrl+Cmd+V re-paste

    private func pasteLatest() {
        guard let text = latestResult, !text.isEmpty else {
            logger.info("Ctrl+Cmd+V — nothing stored")
            return
        }
        logger.info("Ctrl+Cmd+V — pasting \(text.count) chars")
        DispatchQueue.global(qos: .userInteractive).async {
            TextPaster.paste(text)
        }
    }

    // MARK: - Option+1-5 shortcuts

    func handleShortcutPublic(_ n: UInt8) { handleShortcut(n) }
    func pasteLatestPublic() { pasteLatest() }

    private func handleShortcut(_ n: UInt8) {
        DispatchQueue.global(qos: .userInteractive).asyncAfter(deadline: .now() + 0.05) { [weak self] in
            guard let self else { return }
            switch n {
            case 1: self.formatFixSelectedText()
            case 2: self.polishSelectedText(tone: "professional")
            case 3: self.polishSelectedText(tone: "casual")
            case 4: self.polishSelectedText(tone: "concise")
            case 5: self.polishSelectedText(tone: "hinglish")
            default: break
            }
        }
    }

    private func polishSelectedText(tone: String) {
        guard let text = TextPaster.readSelectedText(), !text.trimmingCharacters(in: .whitespaces).isEmpty else {
            logger.warning("Option+N — no text selected")
            return
        }
        logger.info("polishing \(text.count) chars with tone=\(tone)")

        let request = backendClient.buildTextPolishRequest(text: text, tone: tone)
        activeTextPolishTask?.cancel()
        DispatchQueue.main.async { self.notchVM.startProcessing(phase: "Enhancing") }
        activeTextPolishTask = Task.detached { [weak self] in
            guard let self else { return }
            var fullText = ""
            var finalDone: PolishDone?
            do {
                for try await token in self.sseClient.streamTokens(request: request) {
                    if Task.isCancelled { return }
                    if token.done {
                        finalDone = token.final
                        break
                    }
                    if token.text == STREAM_RESET_SENTINEL {
                        fullText = ""
                        continue
                    }
                    fullText += token.text
                    await MainActor.run { self.notchVM.appendToken(token.text) }
                }
                let trimmed = (finalDone?.polished ?? fullText)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                guard !trimmed.isEmpty else { return }
                await MainActor.run { self.notchVM.setProcessingPhase("Pasting") }
                TextPaster.paste(trimmed)
                self.latestResult = trimmed
                await MainActor.run {
                    self.notchVM.finish(text: trimmed)
                }
            } catch {
                self.logger.error("text polish failed: \(error)")
                await MainActor.run { self.notchVM.showError(Self.humanize(error.localizedDescription)) }
            }
        }
    }

    private func formatFixSelectedText() {
        let selectedText = TextPaster.readSelectedText()?.trimmingCharacters(in: .whitespacesAndNewlines)
        let sourceText: String
        if let selectedText, !selectedText.isEmpty {
            sourceText = selectedText
        } else {
            sourceText = latestResult?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        }

        guard !sourceText.isEmpty else {
            logger.warning("Option+1 — no selected text or latest result to format")
            return
        }

        logger.info("format-fixing \(sourceText.count) chars")
        let request = backendClient.buildTextFormatFixRequest(text: sourceText)
        activeTextPolishTask?.cancel()
        DispatchQueue.main.async { self.notchVM.startProcessing(phase: "Formatting") }
        activeTextPolishTask = Task.detached { [weak self] in
            guard let self else { return }
            var fullText = ""
            var finalDone: PolishDone?
            do {
                for try await token in self.sseClient.streamTokens(request: request) {
                    if Task.isCancelled { return }
                    if token.done {
                        finalDone = token.final
                        break
                    }
                    if token.text == STREAM_RESET_SENTINEL {
                        fullText = ""
                        continue
                    }
                    fullText += token.text
                    await MainActor.run { self.notchVM.appendToken(token.text) }
                }

                let trimmed = (finalDone?.polished ?? fullText)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                guard !trimmed.isEmpty else {
                    await MainActor.run { self.notchVM.showError("No formatted text returned") }
                    return
                }

                await MainActor.run { self.notchVM.setProcessingPhase("Pasting") }
                TextPaster.paste(trimmed)
                self.latestResult = trimmed
                await MainActor.run {
                    self.notchVM.finish(text: trimmed)
                }
            } catch {
                self.logger.error("format fix failed: \(error)")
                await MainActor.run { self.notchVM.showError(Self.humanize(error.localizedDescription)) }
            }
        }
    }

    // MARK: - Error humanization

    static func humanize(_ raw: String) -> String {
        let lower = raw.lowercased()
        if lower.contains("empty transcript") || lower.contains("nothing spoken") {
            return "We couldn't hear anything. Try again."
        }
        if lower.contains("401") || lower.contains("403") || lower.contains("unauthorized") {
            return "API key not accepted. Check Settings."
        }
        if lower.contains("429") || lower.contains("rate") {
            return "Rate limited. Try again in a moment."
        }
        if lower.contains("timeout") || lower.contains("timed out") {
            return "Connection timed out. Check your internet."
        }
        if lower.contains("missing_api_keys") || lower.contains("api keys required") {
            return "API keys required — open Settings."
        }
        return "Something went wrong. Please try again."
    }

    // MARK: - Timing helpers

    private func ms(since start: ContinuousClock.Instant) -> Int {
        let d = ContinuousClock.now - start
        return Int(d.components.seconds * 1000
            + d.components.attoseconds / 1_000_000_000_000_000)
    }

    private func us(since start: ContinuousClock.Instant) -> UInt64 {
        let d = ContinuousClock.now - start
        return UInt64(d.components.seconds) * 1_000_000
            + UInt64(d.components.attoseconds / 1_000_000_000_000)
    }

    private func beginRecordingState() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard pipelineState == .idle else { return false }
        pipelineState = .recording
        return true
    }

    private func beginProcessingState() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard pipelineState == .recording else { return false }
        pipelineState = .processing
        return true
    }

    private func setPipelineIdle() {
        stateLock.lock()
        pipelineState = .idle
        stateLock.unlock()
    }

    private func currentPipelineState() -> PipelineState {
        stateLock.lock()
        defer { stateLock.unlock() }
        return pipelineState
    }
}
