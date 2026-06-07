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
}

#if os(iOS)
public final class VoiceStreamingClient {
    public var onUpdate: ((VoiceStreamUpdate) -> Void)?

    private let baseURL: URL
    private let urlSession: URLSession
    private let audioSession: AVAudioSession
    private let audioEngine = AVAudioEngine()
    private let audioQueue = DispatchQueue(label: "com.emiac.airnote.voice-stream.audio")

    private var webSocket: URLSessionWebSocketTask?
    private var converter: AVAudioConverter?
    private var outputFormat: AVAudioFormat?
    private var maxDurationTask: Task<Void, Never>?
    private var receiveTask: Task<Void, Never>?
    private var isStopping = false

    public init(
        baseURL: URL = BuildConfig.gatewayBaseURL,
        urlSession: URLSession = .shared,
        audioSession: AVAudioSession = .sharedInstance()
    ) {
        self.baseURL = baseURL
        self.urlSession = urlSession
        self.audioSession = audioSession
    }

    public func start(session: MobileSessionResponse) async throws {
        guard !isRecording else { return }
        guard let voiceWSURL = session.voiceWSURL else {
            throw VoiceStreamError(code: "missing_voice_ws_url", retryable: true, message: "Runtime session is missing its voice socket.")
        }

        isStopping = false
        let socketURL = try websocketURL(relativeOrAbsolute: voiceWSURL)
        let task = urlSession.webSocketTask(with: socketURL)
        webSocket = task
        task.resume()

        receiveTask = Task { [weak self] in
            await self?.receiveLoop()
        }

        try await task.send(.string("{\"type\":\"voice.start\"}"))
        try await startAudioEngine()

        let maxSeconds = session.maxRecordingSeconds ?? BuildConfig.maxRecordingSeconds
        maxDurationTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(maxSeconds) * 1_000_000_000)
            await self?.stop()
        }
    }

    public func stop() async {
        guard !isStopping else { return }
        isStopping = true
        stopAudioEngine()
        maxDurationTask?.cancel()
        maxDurationTask = nil
        try? await webSocket?.send(.string("{\"type\":\"audio.end\"}"))
    }

    public func cancel() async {
        isStopping = true
        stopAudioEngine()
        maxDurationTask?.cancel()
        maxDurationTask = nil
        receiveTask?.cancel()
        receiveTask = nil
        webSocket?.cancel(with: .goingAway, reason: nil)
        webSocket = nil
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
    }

    private func stopAudioEngine() {
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
        let task = webSocket
        Task {
            try? await task?.send(.data(data))
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
                guard let message = try await webSocket?.receive() else {
                    emit(.done)
                    return
                }
                switch message {
                case .string(let text):
                    handleServerText(text)
                case .data:
                    break
                @unknown default:
                    break
                }
            } catch {
                if !isStopping {
                    emit(.error(VoiceStreamError(code: "ws_disconnected", retryable: true, message: "AirNote lost the voice connection.")))
                }
                return
            }
        }
    }

    private func handleServerText(_ text: String) {
        guard
            let data = text.data(using: .utf8),
            let envelope = try? JSONDecoder().decode(ServerEnvelope.self, from: data)
        else {
            return
        }

        switch envelope.type {
        case "session.ready":
            if let event = try? JSONDecoder().decode(SessionReadyEvent.self, from: data) {
                emit(.sessionReady(sessionID: event.sessionID, runID: event.runID))
            }
        case "runtime.status":
            if let event = try? JSONDecoder().decode(StatusEvent.self, from: data) {
                emit(.status(event.status))
            }
        case "stt.interim":
            if let event = try? JSONDecoder().decode(TextEvent.self, from: data) {
                emit(.interimTranscript(event.text))
            }
        case "stt.final":
            if let event = try? JSONDecoder().decode(TextEvent.self, from: data) {
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
            emit(.done)
        case "error":
            if let event = try? JSONDecoder().decode(ErrorEvent.self, from: data) {
                emit(.error(VoiceStreamError(code: event.code, retryable: event.retryable, message: event.message)))
            }
        default:
            break
        }
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
}
#else
public final class VoiceStreamingClient {
    public var onUpdate: ((VoiceStreamUpdate) -> Void)?

    public init(baseURL: URL = BuildConfig.gatewayBaseURL, urlSession: URLSession = .shared) {}

    public func start(session: MobileSessionResponse) async throws {
        throw VoiceStreamError(
            code: "ios_audio_unavailable",
            retryable: false,
            message: "Live audio streaming is only available in the iOS app build."
        )
    }

    public func stop() async {}

    public func cancel() async {}

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
    var status: String
}

private struct TextEvent: Decodable {
    var text: String
}

private struct DeltaEvent: Decodable {
    var token: String
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
