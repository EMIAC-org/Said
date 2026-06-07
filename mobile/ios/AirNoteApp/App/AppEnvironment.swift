import Foundation
import AirNoteShared

@MainActor
final class AppEnvironment: ObservableObject {
    @Published var setupState: SetupState = .notStarted
    @Published var sessionState: SessionState = .idle
    @Published var languageHint: LanguageHint = .hinglish
    @Published var style: DictationStyle = .work
    @Published var lastStatusMessage: String = "AirNote is ready"

    let dictationStore = DictationStore()
    let eventQueue = EventQueue()
    let gateway: MobileGatewayClient = MockMobileGatewayClient()

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
