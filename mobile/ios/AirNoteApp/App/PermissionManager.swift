import AVFoundation
import AirNoteShared
import Combine
import UIKit

enum MicPermission: Equatable {
    case undetermined
    case granted
    case denied
}

/// Whether AirNote's custom keyboard is enabled and has Full Access.
///
/// iOS gives no public API to query another extension's state from the host
/// app, so we use a health handshake over the App Group: the keyboard writes
/// `recordKeyboardHealth(...)` every time it loads. A keyboard extension can
/// only reach the shared container (or the network) when Full Access is ON, so
/// a fresh health write is a reliable "enabled + Full Access" signal.
enum KeyboardReadiness: Equatable {
    case unknown          // never seen the keyboard load
    case needsFullAccess  // seen, but Full Access reported off
    case ready            // enabled + Full Access
}

@MainActor
final class PermissionManager: ObservableObject {
    @Published private(set) var micPermission: MicPermission
    @Published private(set) var keyboard: KeyboardReadiness

    init() {
        micPermission = Self.currentMicPermission()
        keyboard = Self.currentKeyboardReadiness()
    }

    // MARK: Microphone

    static func currentMicPermission() -> MicPermission {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return .granted
        case .denied: return .denied
        case .undetermined: return .undetermined
        @unknown default: return .undetermined
        }
    }

    func refreshMic() {
        micPermission = Self.currentMicPermission()
    }

    /// Requests microphone access. Triggers the native OS dialog only the first
    /// time; afterwards it resolves immediately with the stored decision.
    @discardableResult
    func requestMic() async -> Bool {
        if micPermission == .granted { return true }
        let granted = await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            AVAudioApplication.requestRecordPermission { allowed in
                continuation.resume(returning: allowed)
            }
        }
        micPermission = granted ? .granted : .denied
        return granted
    }

    // MARK: Keyboard

    static func currentKeyboardReadiness() -> KeyboardReadiness {
        guard SharedStore.keyboardLastSeen != nil else { return .unknown }
        return SharedStore.keyboardHasFullAccess ? .ready : .needsFullAccess
    }

    func refreshKeyboard() {
        keyboard = Self.currentKeyboardReadiness()
    }

    func refreshAll() {
        refreshMic()
        refreshKeyboard()
    }

    // MARK: Deep links

    /// Opens AirNote's entry in Settings. iOS exposes no public deep link to the
    /// Keyboards list, so the onboarding walkthrough guides the last two taps.
    func openSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}
