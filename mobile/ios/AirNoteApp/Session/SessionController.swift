import Foundation
import AirNoteShared

@MainActor
final class SessionController: ObservableObject {
    @Published private(set) var state: SessionState = .idle

    private let bridge: AppGroupBridge?
    private let gateway: MobileGatewayClient

    init(bridge: AppGroupBridge? = try? AppGroupBridge(), gateway: MobileGatewayClient = MockMobileGatewayClient()) {
        self.bridge = bridge
        self.gateway = gateway
    }

    func startSession(deviceID: String, context: KeyboardContext, languageHint: LanguageHint, style: DictationStyle) async {
        let request = MobileSessionRequest(
            clientRequestID: RequestId.make(),
            deviceID: deviceID,
            languageHint: languageHint,
            style: style,
            keyboardContext: context
        )

        do {
            let response = try await gateway.createSession(request)
            let session = BridgeSession(
                sessionID: response.sessionID,
                deviceID: deviceID,
                state: .ready,
                startedAt: Date(),
                expiresAt: response.expiresAt,
                heartbeatAt: Date(),
                languageHint: languageHint,
                style: style,
                surface: .iosKeyboard,
                gatewayRegion: "mock",
                resultSeq: 0,
                commandSeq: 0
            )
            try bridge?.write(session, to: .session)
            state = .ready
        } catch {
            state = .retryableError("Could not start AirNote Session.")
        }
    }

    func markStale() {
        state = .stale
    }
}
