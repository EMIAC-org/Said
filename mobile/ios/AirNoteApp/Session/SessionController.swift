import AirNoteShared
import Combine
import Foundation

@MainActor
final class SessionController: ObservableObject {
    @Published private(set) var state: SessionState = .idle
    @Published private(set) var interimTranscript: String = ""
    @Published private(set) var polishPreview: String = ""
    @Published private(set) var level: Float = 0
    @Published private(set) var lastLatencyMS: Int?
    @Published private(set) var lastErrorMessage: String?

    private let bridge: AppGroupBridge?
    private let gateway: any MobileGatewayClient
    private let streamer: VoiceStreamingClient
    private var activeSession: MobileSessionResponse?
    private var activeClientRequestID: String?
    private var activeDeviceID: String?
    private var activeLanguageHint: LanguageHint = .hinglish
    private var activeStyle: DictationStyle = .work
    private var lastCommandSeq: UInt64 = 0
    private var resultSeq: UInt64 = 0
    private var commandTimer: Timer?

    init(
        bridge: AppGroupBridge? = try? AppGroupBridge(),
        gateway: any MobileGatewayClient = GatewayEnvironment.makeClient(),
        streamer: VoiceStreamingClient = VoiceStreamingClient()
    ) {
        self.bridge = bridge
        self.gateway = gateway
        self.streamer = streamer
        self.streamer.onUpdate = { [weak self] update in
            Task { @MainActor in
                self?.handle(update)
            }
        }
    }

    func startSession(deviceID: String, context: KeyboardContext, languageHint: LanguageHint, style: DictationStyle) async {
        let clientRequestID = RequestId.make()
        let request = MobileSessionRequest(
            clientRequestID: clientRequestID,
            deviceID: deviceID,
            languageHint: languageHint,
            style: style,
            keyboardContext: context
        )

        do {
            let response = try await gateway.createSession(request)
            activeSession = response
            activeClientRequestID = clientRequestID
            activeDeviceID = deviceID
            activeLanguageHint = languageHint
            activeStyle = style
            try writeSession(response, deviceID: deviceID, state: .ready, languageHint: languageHint, style: style)
            state = .ready
            lastErrorMessage = nil
        } catch {
            state = .retryableError("Could not start AirNote Session.")
            lastErrorMessage = "Could not start AirNote Session."
        }
    }

    func startCommandWatcher() {
        guard commandTimer == nil else { return }
        commandTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: true) { [weak self] _ in
            Task { @MainActor in
                await self?.pollCommand()
            }
        }
    }

    func stopCommandWatcher() {
        commandTimer?.invalidate()
        commandTimer = nil
    }

    func markStale() {
        state = .stale
    }

    func stopRecording() async {
        if BuildConfig.useMockGateway {
            await finishMockRecording()
            return
        }
        await streamer.stop()
    }

    func cancelRecording() async {
        await streamer.cancel()
        if let activeSession, let deviceID = activeDeviceID {
            try? writeSession(activeSession, deviceID: deviceID, state: .ready, languageHint: activeLanguageHint, style: activeStyle)
        }
        state = .ready
        interimTranscript = ""
        polishPreview = ""
    }

    private func pollCommand() async {
        guard let command = try? bridge?.read(BridgeCommand.self, from: .command) else {
            return
        }
        guard command.commandSeq > lastCommandSeq else {
            return
        }
        lastCommandSeq = command.commandSeq

        switch command.kind {
        case .startSession:
            await startSession(
                deviceID: activeDeviceID ?? "ios-\(UUID().uuidString)",
                context: command.keyboardContext,
                languageHint: command.languageHint,
                style: command.style
            )
        case .startRecording:
            await beginRecording(command)
        case .stopRecording:
            await stopRecording()
        case .cancelRecording:
            await cancelRecording()
        case .requestInsert, .clearState:
            break
        }
    }

    private func beginRecording(_ command: BridgeCommand) async {
        let deviceID = activeDeviceID ?? "ios-\(UUID().uuidString)"
        if activeSession == nil || activeSession?.expiresAt ?? .distantPast < Date() {
            await startSession(
                deviceID: deviceID,
                context: command.keyboardContext,
                languageHint: command.languageHint,
                style: command.style
            )
        }

        guard let session = activeSession else {
            state = .retryableError("Start AirNote Session before recording.")
            return
        }

        activeClientRequestID = command.clientRequestID
        activeLanguageHint = command.languageHint
        activeStyle = command.style
        interimTranscript = ""
        polishPreview = ""
        lastLatencyMS = nil
        lastErrorMessage = nil

        do {
            try writeSession(session, deviceID: deviceID, state: .recording, languageHint: command.languageHint, style: command.style)
            state = .recording(startedAt: Date())
            if BuildConfig.useMockGateway {
                await runMockRecording(session: session, command: command, deviceID: deviceID)
            } else {
                try await streamer.start(session: session)
            }
        } catch {
            state = .retryableError("Could not start recording. Retry or use copy recovery.")
            lastErrorMessage = "Could not start recording. Retry or use copy recovery."
            try? writeSession(session, deviceID: deviceID, state: .ready, languageHint: command.languageHint, style: command.style)
        }
    }

    private func handle(_ update: VoiceStreamUpdate) {
        switch update {
        case .sessionReady:
            state = .recording(startedAt: Date())
        case .status(let status):
            if status == "ready_for_audio" {
                state = .recording(startedAt: Date())
            } else if status == "audio_interrupted" {
                state = .processing
                writeProcessingState("Paused by iOS")
            } else if status == "audio_route_changed" {
                state = .processing
                writeProcessingState("Microphone route changed")
            } else {
                state = .processing
            }
        case .interimTranscript(let text):
            interimTranscript = text
            state = .processing
            writeProcessingState("Transcribing")
        case .finalTranscript(let text):
            interimTranscript = text
            state = .processing
            writeProcessingState("Polishing")
        case .polishStarted:
            polishPreview = ""
            state = .processing
            writeProcessingState("Polishing")
        case .polishDelta(let token):
            polishPreview += token
            state = .processing
            writeProcessingState("Polishing")
        case .guardWarning:
            state = .processing
        case .final(let final):
            publish(final)
        case .done:
            break
        case .error(let error):
            state = .retryableError(error.message)
            lastErrorMessage = error.message
            writeProcessingState(error.retryable ? "Retry available" : "Needs repair")
        case .level(let value):
            level = value
        }
    }

    private func publish(_ final: VoiceFinalResult) {
        resultSeq += 1
        let result = BridgeResult(
            resultSeq: resultSeq,
            sessionID: final.sessionID ?? activeSession?.sessionID ?? "unknown-session",
            clientRequestID: activeClientRequestID ?? RequestId.make(),
            requestID: final.requestID,
            state: .final,
            transcript: final.transcript,
            polished: final.polished,
            language: final.language,
            style: final.style,
            latencyMS: final.latencyMS,
            expiresAt: Date().addingTimeInterval(10 * 60),
            insertPolicy: .insertAtCursor,
            learningAllowed: true
        )
        try? bridge?.write(result, to: .result)
        if let session = activeSession, let deviceID = activeDeviceID {
            try? writeSession(session, deviceID: deviceID, state: .insertReady, languageHint: final.language, style: final.style)
        }
        lastLatencyMS = final.latencyMS
        state = .insertReady(result)
    }

    private func runMockRecording(session: MobileSessionResponse, command: BridgeCommand, deviceID: String) async {
        try? await Task.sleep(nanoseconds: 650_000_000)
        interimTranscript = "kal ka update concise banake rahul ko bhej do"
        writeProcessingState("Transcribing")
        try? await Task.sleep(nanoseconds: 650_000_000)
        polishPreview = "Kal ka update concise bana ke Rahul ko bhej do."
        writeProcessingState("Polishing")
        try? await Task.sleep(nanoseconds: 450_000_000)
        let final = VoiceFinalResult(
            requestID: RequestId.make(),
            sessionID: session.sessionID,
            transcript: interimTranscript,
            polished: polishPreview,
            language: command.languageHint == .auto ? .hinglish : command.languageHint,
            style: command.style,
            latencyMS: 420,
            mock: true
        )
        activeSession = session
        activeDeviceID = deviceID
        publish(final)
    }

    private func finishMockRecording() async {
        guard case .recording = state, let session = activeSession else { return }
        let command = BridgeCommand(
            kind: .stopRecording,
            commandSeq: UInt64(Date().timeIntervalSince1970 * 1000),
            keyboardContext: KeyboardContext(beforeText: "", afterText: "", selectedText: "", hostAppLabel: "AirNote", fieldHint: "practice"),
            languageHint: activeLanguageHint,
            style: activeStyle,
            clientRequestID: activeClientRequestID ?? RequestId.make()
        )
        await runMockRecording(session: session, command: command, deviceID: activeDeviceID ?? "ios-mock")
    }

    private func writeProcessingState(_ phase: String) {
        guard let activeSession, let deviceID = activeDeviceID else { return }
        let bridgeState: BridgeSessionState = phase.contains("Retry") || phase.contains("repair") ? .error : .processing
        try? writeSession(activeSession, deviceID: deviceID, state: bridgeState, languageHint: activeLanguageHint, style: activeStyle)
    }

    private func writeSession(
        _ response: MobileSessionResponse,
        deviceID: String,
        state: BridgeSessionState,
        languageHint: LanguageHint,
        style: DictationStyle
    ) throws {
        let session = BridgeSession(
            sessionID: response.sessionID,
            deviceID: deviceID,
            state: state,
            startedAt: Date(),
            expiresAt: response.expiresAt,
            heartbeatAt: Date(),
            languageHint: languageHint,
            style: style,
            surface: .iosKeyboard,
            gatewayRegion: BuildConfig.gatewayBaseURL.host ?? "gateway",
            resultSeq: resultSeq,
            commandSeq: lastCommandSeq,
            sessionToken: response.sessionToken,
            voiceWSURL: response.voiceWSURL,
            batchURL: response.batchURL,
            currentVocabHash: response.currentVocabHash,
            maxRecordingSeconds: response.maxRecordingSeconds
        )
        try bridge?.write(session, to: .session)
    }
}
