import Foundation
#if os(iOS)
import AVFoundation
#endif

public enum VoiceStreamUpdate: Equatable {
    case sessionReady(sessionID: String, runID: String?)
    case status(String)
    case interimTranscript(String)
    case finalTranscript(String)
    case polishStarted(model: String?)
    case polishDelta(String)
    case guardWarning(String)
    case final(VoiceFinalResult)
    case done
    case error(VoiceStreamError)
    case level(Float)
}

public struct VoiceFinalResult: Codable, Equatable {
    public var requestID: String
    public var sessionID: String?
    public var transcript: String
    public var polished: String
    public var language: LanguageHint
    public var style: DictationStyle
    public var latencyMS: Int
    public var mock: Bool

    public init(
        requestID: String,
        sessionID: String?,
        transcript: String,
        polished: String,
        language: LanguageHint,
        style: DictationStyle,
        latencyMS: Int,
        mock: Bool
    ) {
        self.requestID = requestID
        self.sessionID = sessionID
        self.transcript = transcript
        self.polished = polished
        self.language = language
        self.style = style
        self.latencyMS = latencyMS
        self.mock = mock
    }

    enum CodingKeys: String, CodingKey {
        case requestID = "request_id"
        case sessionID = "session_id"
        case transcript
        case polished
        case language
        case style
        case latencyMS = "latency_ms"
        case mock
    }
}

public struct VoiceStreamError: Error, Equatable {
    public var code: String
    public var retryable: Bool
    public var message: String

    public init(code: String, retryable: Bool, message: String) {
        self.code = code
        self.retryable = retryable
        self.message = message
    }

    /// True when the server reported it has no provider credentials provisioned.
    public var isCredentialMissing: Bool {
        GatewayError.credentialMissingCodes.contains(code)
    }
}

/// Parameters sent with `voice.start` so the server runs STT/polish with the
/// user's chosen model, output language, and personal vocab hints.
public struct VoiceStreamConfig: Equatable {
    public var runID: String?
    public var selectedModel: String
    public var outputLanguage: String
    public var safeVocabTerms: [String]
    public var screenContext: String?
    public var platform: String
    public var appVersion: String

    public init(
        runID: String? = nil,
        selectedModel: String = "fast",
        outputLanguage: String = "hinglish",
        safeVocabTerms: [String] = [],
        screenContext: String? = nil,
        platform: String = "ios",
        appVersion: String = "0.1.0"
    ) {
        self.runID = runID
        self.selectedModel = selectedModel
        self.outputLanguage = outputLanguage
        self.safeVocabTerms = safeVocabTerms
        self.screenContext = screenContext
        self.platform = platform
        self.appVersion = appVersion
    }

    func startPayloadJSON(sampleRate: Int) -> String {
        var payload: [String: Any] = [
            "type": "voice.start",
            "mode": "normal_voice",
            "selected_model": selectedModel,
            "output_language": outputLanguage,
            "platform": platform,
            "app_version": appVersion,
            "audio": ["sample_rate": sampleRate],
        ]
        if let runID, !runID.isEmpty { payload["run_id"] = runID }
        if !safeVocabTerms.isEmpty { payload["safe_vocab_terms"] = Array(safeVocabTerms.prefix(30)) }
        if let screenContext, !screenContext.isEmpty {
            payload["screen_context"] = String(screenContext.prefix(500))
        }
        guard
            let data = try? JSONSerialization.data(withJSONObject: payload),
            let json = String(data: data, encoding: .utf8)
        else {
            return "{\"type\":\"voice.start\"}"
        }
        return json
    }
}

#if os(iOS)
public final class VoiceStreamingClient {
    public var onUpdate: ((VoiceStreamUpdate) -> Void)?

    private let baseURL: URL
    private let urlSession: URLSession
    private let audioSession: AVAudioSession
    private let audioEngine = AVAudioEngine()
    private let audioQueue = DispatchQueue(label: "com.emiac.airnote.voice-stream.audio")
    private let stateQueue = DispatchQueue(label: "com.emiac.airnote.voice-stream.state")

    private var webSocket: URLSessionWebSocketTask?
    private var converter: AVAudioConverter?
    private var outputFormat: AVAudioFormat?
    private var maxDurationTask: Task<Void, Never>?
    private var receiveTask: Task<Void, Never>?
    private var isStopping = false
    private var didReceiveTerminalEvent = false
    private var interruptionObservers: [NSObjectProtocol] = []
    private var currentSession: MobileSessionResponse?
    private var latestTranscript = ""

    public init(
        baseURL: URL = BuildConfig.gatewayBaseURL,
        urlSession: URLSession = .shared,
        audioSession: AVAudioSession = .sharedInstance()
    ) {
        self.baseURL = baseURL
        self.urlSession = urlSession
        self.audioSession = audioSession
    }

    public func start(session: MobileSessionResponse, config: VoiceStreamConfig = VoiceStreamConfig()) async throws {
        guard !isRecording else { return }
        guard let voiceWSURL = session.voiceWSURL else {
            throw VoiceStreamError(code: "missing_voice_ws_url", retryable: true, message: "Runtime session is missing its voice socket.")
        }

        let socketURL = try websocketURL(relativeOrAbsolute: voiceWSURL)
        let task = urlSession.webSocketTask(with: socketURL)
        currentSession = session
        latestTranscript = ""
        prepareForStart(task: task)
        task.resume()

        receiveTask = Task { [weak self] in
            await self?.receiveLoop()
        }

        do {
            try await task.send(.string(config.startPayloadJSON(sampleRate: 16_000)))
            try await startAudioEngine()
        } catch {
            await cancel()
            throw error
        }

        let maxSeconds = session.maxRecordingSeconds ?? BuildConfig.maxRecordingSeconds
        maxDurationTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(maxSeconds) * 1_000_000_000)
            await self?.stop()
        }
    }

    public func stop() async {
        guard markStopping() else { return }
        stopAudioEngine()
        maxDurationTask?.cancel()
        maxDurationTask = nil
        try? await currentWebSocket()?.send(.string("{\"type\":\"audio.end\"}"))
    }

    public func cancel() async {
        hardStop()
    }

    /// Fully synchronous teardown — safe to call from `deinit` or a view's
    /// `viewWillDisappear`, where spawning a Task is not guaranteed to run
    /// (especially in a keyboard extension the system may unload immediately).
    public func hardStop() {
        setStopping()
        stopAudioEngine()
        maxDurationTask?.cancel()
        maxDurationTask = nil
        receiveTask?.cancel()
        receiveTask = nil
        currentSession = nil
        latestTranscript = ""
        currentWebSocket()?.cancel(with: .goingAway, reason: nil)
        setWebSocket(nil)
    }

    public var isRecording: Bool {
        audioEngine.isRunning
    }

    private func startAudioEngine() async throws {
        try audioSession.setCategory(.record, mode: .measurement, options: [.duckOthers])
        try audioSession.setActive(true, options: [])

        let input = audioEngine.inputNode
        let inputFormat = input.inputFormat(forBus: 0)
        guard let outFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16_000,
            channels: 1,
            interleaved: true
        ) else {
            throw VoiceStreamError(code: "audio_format_unavailable", retryable: true, message: "Could not prepare the AirNote audio format.")
        }

        converter = AVAudioConverter(from: inputFormat, to: outFormat)
        outputFormat = outFormat

        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 1_024, format: inputFormat) { [weak self] buffer, _ in
            self?.audioQueue.async {
                self?.convertAndSend(buffer)
            }
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

    private func convertAndSend(_ buffer: AVAudioPCMBuffer) {
        guard
            let converter,
            let outputFormat,
            let outBuffer = AVAudioPCMBuffer(
                pcmFormat: outputFormat,
                frameCapacity: AVAudioFrameCount(max(1, Double(buffer.frameLength) * outputFormat.sampleRate / buffer.format.sampleRate + 8))
            )
        else {
            return
        }

        var didProvideInput = false
        var conversionError: NSError?
        let inputBlock: AVAudioConverterInputBlock = { _, status in
            if didProvideInput {
                status.pointee = .noDataNow
                return nil
            }
            didProvideInput = true
            status.pointee = .haveData
            return buffer
        }

        converter.convert(to: outBuffer, error: &conversionError, withInputFrom: inputBlock)
        guard conversionError == nil, let data = pcmData(from: outBuffer), !data.isEmpty else {
            return
        }

        emit(.level(averageLevel(buffer)))
        guard let task = audioSendTarget() else { return }
        Task {
            try? await task.send(.data(data))
        }
    }

    private func pcmData(from buffer: AVAudioPCMBuffer) -> Data? {
        let byteCount = Int(buffer.frameLength) * Int(buffer.format.streamDescription.pointee.mBytesPerFrame)
        guard byteCount > 0 else { return nil }
        let audioBuffer = buffer.audioBufferList.pointee.mBuffers
        guard let data = audioBuffer.mData else { return nil }
        return Data(bytes: data, count: byteCount)
    }

    private func averageLevel(_ buffer: AVAudioPCMBuffer) -> Float {
        guard let channel = buffer.floatChannelData?.pointee else {
            return 0
        }
        let count = Int(buffer.frameLength)
        guard count > 0 else { return 0 }
        var sum: Float = 0
        for index in 0..<count {
            sum += abs(channel[index])
        }
        return min(1, sum / Float(count) * 4)
    }

    private func receiveLoop() async {
        while !Task.isCancelled {
            do {
                guard let message = try await currentWebSocket()?.receive() else {
                    // The socket is only nil here because stop()/cancel() cleared
                    // it. If we're stopping or already finished, stay silent — do
                    // NOT report a spurious success for a cancelled recording.
                    if !shouldSuppressDisconnectError, markTerminalEvent() {
                        emit(.done)
                    }
                    return
                }
                switch message {
                case .string(let text):
                    guard handleServerText(text) else {
                        stopAudioEngine()
                        return
                    }
                case .data:
                    break
                @unknown default:
                    break
                }
            } catch {
                stopAudioEngine()
                if !shouldSuppressDisconnectError, markTerminalEvent() {
                    emit(.error(VoiceStreamError(code: "ws_disconnected", retryable: true, message: "AirNote lost the voice connection.")))
                }
                return
            }
        }
    }

    private func handleServerText(_ text: String) -> Bool {
        guard
            let data = text.data(using: .utf8),
            let envelope = try? JSONDecoder().decode(ServerEnvelope.self, from: data)
        else {
            return true
        }

        switch envelope.type {
        case "session.ready":
            if let event = try? JSONDecoder().decode(SessionReadyEvent.self, from: data) {
                emit(.sessionReady(sessionID: event.sessionID, runID: event.runID))
            }
        case "runtime.status":
            if let event = try? JSONDecoder().decode(StatusEvent.self, from: data) {
                emit(.status(event.statusText))
            }
        case "stt.interim", "transcript.partial":
            if let event = try? JSONDecoder().decode(TextEvent.self, from: data) {
                emit(.interimTranscript(event.text))
            }
        case "stt.final", "transcript.final":
            if let event = try? JSONDecoder().decode(TextEvent.self, from: data) {
                latestTranscript = event.text
                emit(.finalTranscript(event.text))
            }
        case "polish.started":
            let event = try? JSONDecoder().decode(PolishStartedEvent.self, from: data)
            emit(.polishStarted(model: event?.model))
        case "polish.delta":
            if let event = try? JSONDecoder().decode(DeltaEvent.self, from: data) {
                emit(.polishDelta(event.token))
            }
        case "guard.warning":
            if let event = try? JSONDecoder().decode(WarningEvent.self, from: data) {
                emit(.guardWarning(event.code))
            }
        case "final":
            if let event = try? JSONDecoder().decode(VoiceFinalResult.self, from: data) {
                emit(.final(event))
            }
        case "runtime.done":
            if markTerminalEvent(), let event = try? JSONDecoder().decode(RuntimeDoneEvent.self, from: data) {
                emit(.final(event.finalResult(session: currentSession, transcript: latestTranscript)))
                emit(.done)
            }
            return false
        case "error":
            if let event = try? JSONDecoder().decode(ErrorEvent.self, from: data) {
                if markTerminalEvent() {
                    emit(.error(VoiceStreamError(code: event.code, retryable: event.retryable, message: event.message)))
                }
            } else if markTerminalEvent() {
                emit(.error(VoiceStreamError(code: "server_error", retryable: true, message: "AirNote could not finish this recording.")))
            }
            return false
        default:
            break
        }
        return true
    }

    private func websocketURL(relativeOrAbsolute: String) throws -> URL {
        if let absolute = URL(string: relativeOrAbsolute), absolute.scheme?.hasPrefix("ws") == true {
            return absolute
        }

        guard var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false) else {
            throw VoiceStreamError(code: "invalid_gateway_url", retryable: false, message: "Gateway URL is invalid.")
        }
        components.scheme = components.scheme == "http" ? "ws" : "wss"

        guard let relativeComponents = URLComponents(string: relativeOrAbsolute) else {
            throw VoiceStreamError(code: "invalid_voice_ws_url", retryable: true, message: "Voice socket URL is invalid.")
        }
        components.path = relativeComponents.path
        components.query = relativeComponents.query

        guard let url = components.url else {
            throw VoiceStreamError(code: "invalid_voice_ws_url", retryable: true, message: "Voice socket URL is invalid.")
        }
        return url
    }

    private func emit(_ update: VoiceStreamUpdate) {
        DispatchQueue.main.async { [onUpdate] in
            onUpdate?(update)
        }
    }

    private func prepareForStart(task: URLSessionWebSocketTask) {
        stateQueue.sync {
            webSocket = task
            isStopping = false
            didReceiveTerminalEvent = false
        }
    }

    private func currentWebSocket() -> URLSessionWebSocketTask? {
        stateQueue.sync {
            webSocket
        }
    }

    private func setWebSocket(_ task: URLSessionWebSocketTask?) {
        stateQueue.sync {
            webSocket = task
        }
    }

    private func audioSendTarget() -> URLSessionWebSocketTask? {
        stateQueue.sync {
            guard !isStopping, !didReceiveTerminalEvent else { return nil }
            return webSocket
        }
    }

    @discardableResult
    private func markStopping() -> Bool {
        stateQueue.sync {
            if isStopping {
                return false
            }
            isStopping = true
            return true
        }
    }

    private func setStopping() {
        stateQueue.sync {
            isStopping = true
        }
    }

    @discardableResult
    private func markTerminalEvent() -> Bool {
        stateQueue.sync {
            if didReceiveTerminalEvent {
                return false
            }
            didReceiveTerminalEvent = true
            return true
        }
    }

    private var shouldSuppressDisconnectError: Bool {
        stateQueue.sync {
            isStopping || didReceiveTerminalEvent
        }
    }

    private func installAudioObservers() {
        let center = NotificationCenter.default
        let interruption = center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: audioSession,
            queue: .main
        ) { [weak self] notification in
            self?.handleAudioInterruption(notification)
        }
        let routeChange = center.addObserver(
            forName: AVAudioSession.routeChangeNotification,
            object: audioSession,
            queue: .main
        ) { [weak self] notification in
            self?.handleRouteChange(notification)
        }

        let newObservers = [interruption, routeChange]
        let observersToRemove = stateQueue.sync {
            let staleObservers = interruptionObservers
            guard !isStopping, !didReceiveTerminalEvent else {
                interruptionObservers.removeAll()
                return staleObservers + newObservers
            }
            interruptionObservers = newObservers
            return staleObservers
        }
        observersToRemove.forEach { center.removeObserver($0) }
    }

    private func removeAudioObservers() {
        let center = NotificationCenter.default
        let observers = stateQueue.sync {
            let observers = interruptionObservers
            interruptionObservers.removeAll()
            return observers
        }
        observers.forEach { center.removeObserver($0) }
    }

    private func handleAudioInterruption(_ notification: Notification) {
        guard
            let rawType = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: rawType),
            type == .began
        else {
            return
        }
        Task { [weak self] in
            await self?.stopAfterAudioSystemChange(status: "audio_interrupted")
        }
    }

    private func handleRouteChange(_ notification: Notification) {
        guard
            let rawReason = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
            let reason = AVAudioSession.RouteChangeReason(rawValue: rawReason)
        else {
            return
        }

        switch reason {
        case .oldDeviceUnavailable, .newDeviceAvailable, .noSuitableRouteForCategory, .routeConfigurationChange:
            Task { [weak self] in
                await self?.stopAfterAudioSystemChange(status: "audio_route_changed")
            }
        default:
            break
        }
    }

    private func stopAfterAudioSystemChange(status: String) async {
        guard markStopping() else { return }
        emit(.status(status))
        stopAudioEngine()
        maxDurationTask?.cancel()
        maxDurationTask = nil
        try? await currentWebSocket()?.send(.string("{\"type\":\"audio.end\"}"))
    }
}
#else
public final class VoiceStreamingClient {
    public var onUpdate: ((VoiceStreamUpdate) -> Void)?

    public init(baseURL: URL = BuildConfig.gatewayBaseURL, urlSession: URLSession = .shared) {}

    public func start(session: MobileSessionResponse, config: VoiceStreamConfig = VoiceStreamConfig()) async throws {
        throw VoiceStreamError(
            code: "ios_audio_unavailable",
            retryable: false,
            message: "Live audio streaming is only available in the iOS app build."
        )
    }

    public func stop() async {}

    public func cancel() async {}

    public func hardStop() {}

    public var isRecording: Bool {
        false
    }
}
#endif

private struct ServerEnvelope: Decodable {
    var type: String
}

private struct SessionReadyEvent: Decodable {
    var sessionID: String
    var runID: String?

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case runID = "run_id"
    }
}

private struct StatusEvent: Decodable {
    var status: String?
    var phase: String?

    var statusText: String {
        status ?? phase ?? "processing"
    }
}

private struct TextEvent: Decodable {
    var text: String
}

private struct DeltaEvent: Decodable {
    var token: String
}

private struct RuntimeDoneEvent: Decodable {
    var runID: String
    var clientRunID: String?
    var output: String
    var modelUsed: String?
    var latencyMS: RuntimeDoneLatency?

    enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case clientRunID = "client_run_id"
        case output
        case modelUsed = "model_used"
        case latencyMS = "latency_ms"
    }

    func finalResult(session: MobileSessionResponse?, transcript: String) -> VoiceFinalResult {
        VoiceFinalResult(
            requestID: clientRunID ?? runID,
            sessionID: session?.sessionID,
            transcript: transcript,
            polished: output,
            language: .hinglish,
            style: .work,
            latencyMS: latencyMS?.total ?? 0,
            mock: false
        )
    }
}

private struct RuntimeDoneLatency: Decodable {
    var stt: Int?
    var polish: Int?
    var total: Int?
}

private struct PolishStartedEvent: Decodable {
    var model: String?
}

private struct WarningEvent: Decodable {
    var code: String
}

private struct ErrorEvent: Decodable {
    var code: String
    var retryable: Bool
    var message: String
}
