import AirNoteShared
import Combine
import Foundation

@MainActor
final class AppEnvironment: ObservableObject {
    @Published var setupState: SetupState = .notStarted
    @Published var sessionState: SessionState = .idle
    @Published var languageHint: LanguageHint = .hinglish
    @Published var style: DictationStyle = .work
    @Published var lastStatusMessage: String = "AirNote is ready"

    let dictationStore = DictationStore()
    let eventQueue = EventQueue()
    let gateway: any MobileGatewayClient

    init(gateway: (any MobileGatewayClient)? = nil) {
        self.gateway = gateway ?? GatewayEnvironment.makeClient()
    }

    func markSetupReady() {
        setupState = .ready
        lastStatusMessage = "Keyboard, mic, and Gateway are ready"
    }
}

enum SetupState: Equatable {
    case notStarted
    case accountReady
    case privacyAccepted
    case micReady
    case keyboardReady
    case fullAccessReady
    case ready
    case blocked(String)
}
