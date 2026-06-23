import SwiftUI

/// A reviewable correction, with local include/exclude selection.
struct ReviewCandidate: Identifiable, Equatable {
    var id: String { "\(original)→\(corrected)" }
    let original: String
    let corrected: String
    let tag: String
    let learnable: Bool
    var selected: Bool
}

/// A recent dictation row.
struct RecentItem: Identifiable, Equatable {
    let id = UUID()
    let text: String
    let ago: String

    static func == (a: RecentItem, b: RecentItem) -> Bool {
        a.text == b.text && a.ago == b.ago
    }
}

/// The full pill state machine — one case per `BarState` family in StatusBar.tsx.
enum HUDState: Equatable {
    case idle
    // dictation
    case listening(startedAt: Date)
    case polishing(phase: String)
    case pasted(text: String)
    case manualPaste(message: String)
    // learning toasts
    case learned(term: String, message: String)
    case emailSaved(email: String, message: String)
    case queued(term: String, remaining: Int)
    case wrongFixed(term: String, wrong: String)
    case retraining
    case retrainDone(durationS: Double)
    // actionable feedback cards
    case confirming(term: String, original: String, recordingId: String)
    case negativeConfirm(term: String, wrong: String)
    case reviewing(candidates: [ReviewCandidate], recordingId: String)
    // system
    case error(message: String, audioId: String?)
    case updateReady(version: String, message: String)
    case recents([RecentItem])
    case placement(message: String)

    /// Open states draw the expanded chin; idle sits as the bare notch.
    var isOpen: Bool {
        if case .idle = self { return false }
        return true
    }

    /// States that take pointer input (buttons, row toggles). These resize the
    /// window to fit; passive states animate the shape inside a fixed stage.
    var isInteractive: Bool {
        switch self {
        case .confirming, .negativeConfirm, .reviewing, .error,
             .updateReady, .recents, .learned:
            return true
        default:
            return false
        }
    }

    /// How long a transient toast lingers before collapsing to idle (seconds).
    /// nil = sticky (driven entirely by Rust / user action).
    var autoHide: TimeInterval? {
        switch self {
        case .pasted:        return 1.6
        case .manualPaste:   return 2.4
        case .learned:       return 3.0
        case .emailSaved:    return 3.0
        case .queued:        return 2.6
        case .wrongFixed:    return 4.0
        case .retrainDone:   return 2.4
        default:             return nil
        }
    }
}

final class HUDModel: ObservableObject {
    @Published var state: HUDState = .idle
    @Published var liveTranscript: String = ""
    /// Size of the black notch shape for the current state. Animating this (not
    /// the window) is what makes the notch grow smoothly with no black "pop".
    @Published var box: CGSize = .zero
}
