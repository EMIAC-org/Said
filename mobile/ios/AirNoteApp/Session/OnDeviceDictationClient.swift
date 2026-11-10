import AirNoteShared
import AVFoundation
import Foundation

/// Dictation with on-device speech-to-text. Drop-in for `VoiceStreamingClient`:
/// same API and the same `VoiceStreamUpdate` events, so `DictationController`
/// and `WarmDictationHost` drive it unchanged.
///
/// Flow (matches the desktop app):
///   mic → 16 kHz mono PCM buffered on the phone
///       → whisper.cpp + Clario Hinglish model on the phone (`LocalWhisperTranscriber`)
///       → transcript → `POST /v1/runtime/voice/polish` (text only, no audio leaves the phone)
///       → polished text. If polish fails, the transcript is used as is.
///
/// Warm-session mode keeps the mic engine running between dictations (so the
/// keyboard can dictate in place); audio is only kept while a dictation is
/// active, plus a short pre-roll so the first syllable isn't clipped.
final class OnDeviceDictationClient {
    var onUpdate: ((VoiceStreamUpdate) -> Void)?

    private static let sampleRate: Double = 16_000
    /// Audio kept from just before a warm dictation starts (~0.4 s).
    private static let prerollSamples = 6_400
    private static let polishTimeout: TimeInterval = 9

    private let transcriber = LocalWhisperTranscriber.shared
    private let urlSession: URLSession
    private let audioSession: AVAudioSession
    private let audioEngine = AVAudioEngine()
    private let audioQueue = DispatchQueue(label: "com.emiac.airnote.on-device.audio")
    private let lock = NSLock()

    private var converter: AVAudioConverter?
    private var outputFormat: AVAudioFormat?
    private var maxDurationTask: Task<Void, Never>?
    private var interruptionObservers: [NSObjectProtocol] = []

    // Guarded by `lock`.
    private var capturing = false
    private var samples: [Float] = []
    private var preroll: [Float] = []
    private var generation = 0
    private var activeSession: MobileSessionResponse?
    private var activeConfig = VoiceStreamConfig()
    private var keepEngineWarm = false

    init(urlSession: URLSession = .shared, audioSession: AVAudioSession = .sharedInstance()) {
        self.urlSession = urlSession
        self.audioSession = audioSession
        transcriber.prewarm()
    }

    // MARK: One-shot dictation (in-app sheet)

    func start(session: MobileSessionResponse, config: VoiceStreamConfig = VoiceStreamConfig()) async throws {
        guard !isCapturing else { return }
        beginCapture(session: session, config: config, usePreroll: false)
        do {
            if !audioEngine.isRunning { try startAudioEngine() }
        } catch {
            hardStop()
            throw VoiceStreamError(code: "audio_start_failed", retryable: true, message: "Couldn't start the microphone. Try again.")
        }
        emit(.sessionReady(sessionID: session.sessionID, runID: config.runID))
        armMaxDuration(session: session) { [weak self] in await self?.stop() }
    }

    func stop() async {
        guard let job = endCapture() else { return }
        if !keepWarm { stopAudioEngine() }
        process(job)
    }

    func cancel() async {
        hardStop()
    }

    /// Synchronous teardown, safe from `deinit` / view teardown.
    func hardStop() {
        lock.withLock {
            capturing = false
            samples = []
            preroll = []
            generation += 1
            activeSession = nil
        }
        maxDurationTask?.cancel()
        maxDurationTask = nil
        stopAudioEngine()
    }

    var isRecording: Bool { audioEngine.isRunning }

    // MARK: Warm-session (keyboard) mode

    var isWarmEngineRunning: Bool { audioEngine.isRunning }

    func startWarmEngine() async throws {
        setKeepWarm(true)
        if !audioEngine.isRunning {
            do { try startAudioEngine() } catch { setKeepWarm(false); throw error }
        }
        transcriber.prewarm()
    }

    /// No socket to open any more. Kept so callers don't change; it just makes
    /// sure the model is loaded before the next dictation.
    func prewarmConnection(session _: MobileSessionResponse) {
        transcriber.prewarm()
    }

    func beginStreaming(session: MobileSessionResponse, config: VoiceStreamConfig) async throws {
        setKeepWarm(true)
        if !audioEngine.isRunning {
            do { try startAudioEngine() } catch { setKeepWarm(false); throw error }
        }
        beginCapture(session: session, config: config, usePreroll: true)
        armMaxDuration(session: session) { [weak self] in await self?.endStreaming() }
    }

    func endStreaming() async {
        guard let job = endCapture() else { return }
        process(job)
    }

    func stopWarmEngine() {
        setKeepWarm(false)
        hardStop()
    }

    // MARK: Capture state

    private struct Job {
        let generation: Int
        let samples: [Float]
        let session: MobileSessionResponse?
        let config: VoiceStreamConfig
        let startedAt: Date
    }

    private var isCapturing: Bool { lock.withLock { capturing } }
    private var keepWarm: Bool { lock.withLock { keepEngineWarm } }
    private func setKeepWarm(_ value: Bool) { lock.withLock { keepEngineWarm = value } }

    private func beginCapture(session: MobileSessionResponse, config: VoiceStreamConfig, usePreroll: Bool) {
        lock.withLock {
            generation += 1
            samples = usePreroll ? preroll : []
            samples.reserveCapacity(Int(Self.sampleRate) * 20)
            preroll = []
            activeSession = session
            activeConfig = config
            capturing = true
        }
    }

    private func endCapture() -> Job? {
        maxDurationTask?.cancel()
        maxDurationTask = nil
        return lock.withLock {
            guard capturing else { return nil }
            capturing = false
            let job = Job(
                generation: generation,
                samples: samples,
                session: activeSession,
                config: activeConfig,
                startedAt: Date()
            )
            samples = []
            return job
        }
    }

    private func isCurrent(_ job: Job) -> Bool {
        lock.withLock { generation == job.generation }
    }

    private func armMaxDuration(session: MobileSessionResponse, onExpire: @escaping () async -> Void) {
        maxDurationTask?.cancel()
        let maxSeconds = session.maxRecordingSeconds ?? BuildConfig.maxRecordingSeconds
        maxDurationTask = Task {
            try? await Task.sleep(nanoseconds: UInt64(maxSeconds) * 1_000_000_000)
            guard !Task.isCancelled else { return }
            await onExpire()
        }
    }

    // MARK: Transcribe + polish

    private func process(_ job: Job) {
        emit(.status("transcribing"))
        Task.detached(priority: .userInitiated) { [weak self] in
            guard let self else { return }
            let sttStarted = Date()
            let raw: String
            do {
                raw = try await self.transcriber.transcribe(job.samples)
            } catch {
                guard self.isCurrent(job) else { return }
                self.emit(.error(VoiceStreamError(
                    code: "local_stt_failed",
                    retryable: true,
                    message: (error as? LocalizedError)?.errorDescription ?? "On-device transcription failed."
                )))
                return
            }
            guard self.isCurrent(job) else { return }
            let sttMS = Int(Date().timeIntervalSince(sttStarted) * 1000)
            let transcript = HinglishScript.enforceRomanHinglish(raw)
            NSLog("AirNote on-device STT: %d ms for %.1f s audio", sttMS, Double(job.samples.count) / Self.sampleRate)

            guard !transcript.isEmpty else {
                // Silence / no speech: callers turn an empty result into "didn't catch that".
                self.emit(.final(self.result(job, transcript: "", polished: "")))
                self.emit(.done)
                return
            }
            self.emit(.finalTranscript(transcript))
            self.emit(.polishStarted(model: job.config.selectedModel))

            let polished = await self.polish(transcript: transcript, job: job)
            guard self.isCurrent(job) else { return }
            if polished == nil { self.emit(.guardWarning("polish_skipped")) }
            self.emit(.final(self.result(job, transcript: transcript, polished: polished ?? transcript)))
            self.emit(.done)
        }
    }

    private func result(_ job: Job, transcript: String, polished: String) -> VoiceFinalResult {
        VoiceFinalResult(
            requestID: job.config.runID ?? job.session?.sessionID ?? RequestId.make(),
            sessionID: job.session?.sessionID,
            transcript: transcript,
            polished: polished,
            language: job.config.outputLanguage == "english" ? .en : .hinglish,
            style: .work,
            latencyMS: Int(Date().timeIntervalSince(job.startedAt) * 1000),
            mock: false
        )
    }

    /// Text-only polish on the server (same endpoint the desktop uses).
    /// Returns nil on any failure so the caller falls back to the transcript.
    private func polish(transcript: String, job: Job) async -> String? {
        let token = job.session?.sessionToken ?? SharedStore.accessToken
        guard let token, !token.isEmpty else { return nil }

        var request = URLRequest(url: BuildConfig.gatewayBaseURL.appendingPathComponent("v1/runtime/voice/polish"))
        request.httpMethod = "POST"
        request.timeoutInterval = Self.polishTimeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")

        let lowered = transcript.lowercased()
        let dictionary = job.config.safeVocabTerms
            .filter { !$0.isEmpty && lowered.contains($0.lowercased()) }
            .prefix(30)
            .map { ["written": $0] }
        var body: [String: Any] = [
            "transcript": transcript,
            "output_language": job.config.outputLanguage,
            "selected_model": job.config.selectedModel,
            "dictionary": Array(dictionary),
            "target_app": "ios",
        ]
        if let runID = job.config.runID { body["client_run_id"] = runID }
        guard let data = try? JSONSerialization.data(withJSONObject: body) else { return nil }
        request.httpBody = data

        do {
            let (responseData, response) = try await urlSession.data(for: request)
            guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                NSLog("AirNote polish failed: HTTP %d", (response as? HTTPURLResponse)?.statusCode ?? -1)
                return nil
            }
            let output = (try? JSONSerialization.jsonObject(with: responseData) as? [String: Any])?["output"] as? String
            let trimmed = output?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return trimmed.isEmpty ? nil : trimmed
        } catch {
            NSLog("AirNote polish failed: %@", error.localizedDescription)
            return nil
        }
    }

    // MARK: Audio engine

    private func startAudioEngine() throws {
        try audioSession.setCategory(
            .playAndRecord,
            mode: .measurement,
            options: [.mixWithOthers, .defaultToSpeaker, .allowBluetoothA2DP]
        )
        try? audioSession.setPreferredIOBufferDuration(0.005)
        try audioSession.setActive(true, options: [])

        let input = audioEngine.inputNode
        let inputFormat = input.inputFormat(forBus: 0)
        guard let outFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: Self.sampleRate,
            channels: 1,
            interleaved: false
        ) else {
            throw VoiceStreamError(code: "audio_format_unavailable", retryable: true, message: "Could not prepare the AirNote audio format.")
        }
        converter = AVAudioConverter(from: inputFormat, to: outFormat)
        outputFormat = outFormat

        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 1_024, format: inputFormat) { [weak self] buffer, _ in
            self?.audioQueue.async { self?.consume(buffer) }
        }
        audioEngine.prepare()
        try audioEngine.start()
        installAudioObservers()
    }

    private func stopAudioEngine() {
        removeAudioObservers()
        if audioEngine.isRunning {
            audioEngine.inputNode.removeTap(onBus: 0)
            audioEngine.stop()
        }
        try? audioSession.setActive(false, options: [.notifyOthersOnDeactivation])
    }

    private func consume(_ buffer: AVAudioPCMBuffer) {
        guard
            let converter,
            let outputFormat,
            let outBuffer = AVAudioPCMBuffer(
                pcmFormat: outputFormat,
                frameCapacity: AVAudioFrameCount(max(1, Double(buffer.frameLength) * outputFormat.sampleRate / buffer.format.sampleRate + 8))
            )
        else { return }

        var didProvideInput = false
        var conversionError: NSError?
        converter.convert(to: outBuffer, error: &conversionError) { _, status in
            if didProvideInput {
                status.pointee = .noDataNow
                return nil
            }
            didProvideInput = true
            status.pointee = .haveData
            return buffer
        }
        guard conversionError == nil, let channel = outBuffer.floatChannelData?.pointee else { return }
        let count = Int(outBuffer.frameLength)
        guard count > 0 else { return }
        let chunk = UnsafeBufferPointer(start: channel, count: count)

        lock.withLock {
            if capturing {
                samples.append(contentsOf: chunk)
            } else if keepEngineWarm {
                preroll.append(contentsOf: chunk)
                if preroll.count > Self.prerollSamples {
                    preroll.removeFirst(preroll.count - Self.prerollSamples)
                }
            }
        }
        emit(.level(averageLevel(buffer)))
    }

    private func averageLevel(_ buffer: AVAudioPCMBuffer) -> Float {
        guard let channel = buffer.floatChannelData?.pointee else { return 0 }
        let count = Int(buffer.frameLength)
        guard count > 0 else { return 0 }
        var sum: Float = 0
        for index in 0..<count { sum += abs(channel[index]) }
        return min(1, sum / Float(count) * 4)
    }

    private func emit(_ update: VoiceStreamUpdate) {
        DispatchQueue.main.async { [weak self] in
            self?.onUpdate?(update)
        }
    }

    // MARK: Interruptions / route changes

    private func installAudioObservers() {
        removeAudioObservers()
        let center = NotificationCenter.default
        interruptionObservers = [
            center.addObserver(forName: AVAudioSession.interruptionNotification, object: audioSession, queue: .main) { [weak self] note in
                guard
                    let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                    AVAudioSession.InterruptionType(rawValue: raw) == .began
                else { return }
                self?.stopAfterAudioSystemChange(status: "audio_interrupted")
            },
            center.addObserver(forName: AVAudioSession.routeChangeNotification, object: audioSession, queue: .main) { [weak self] note in
                guard
                    let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
                    let reason = AVAudioSession.RouteChangeReason(rawValue: raw)
                else { return }
                switch reason {
                case .oldDeviceUnavailable, .newDeviceAvailable, .noSuitableRouteForCategory, .routeConfigurationChange:
                    self?.stopAfterAudioSystemChange(status: "audio_route_changed")
                default:
                    break
                }
            },
        ]
    }

    private func removeAudioObservers() {
        interruptionObservers.forEach { NotificationCenter.default.removeObserver($0) }
        interruptionObservers.removeAll()
    }

    /// iOS won't restart the mic from the background, so drop warm mode; any
    /// in-flight dictation is still transcribed with the audio captured so far.
    private func stopAfterAudioSystemChange(status: String) {
        setKeepWarm(false)
        emit(.status(status))
        let job = endCapture()
        stopAudioEngine()
        if let job { process(job) }
    }
}
