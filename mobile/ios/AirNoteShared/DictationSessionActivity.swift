#if canImport(ActivityKit)
import ActivityKit
import Foundation

/// Live Activity describing the warm dictation session — shown in the Dynamic
/// Island + Lock Screen while the session is on, so the user sees AirNote is
/// ready to dictate in any app (the Wispr "session on" model).
///
/// The ActivityAttributes protocol is App-Extension-API-safe, so this type lives
/// in AirNoteShared and is read by the widget extension. The lifecycle calls
/// (Activity.request/.end) run in the app target only.
public struct DictationSessionAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        /// True while actively dictating; false when the session is warm/ready.
        public var listening: Bool
        /// True when the session is on (show Stop); false when paused (show Resume).
        public var active: Bool
        public init(listening: Bool, active: Bool = true) {
            self.listening = listening
            self.active = active
        }
    }

    public init() {}
}
#endif
