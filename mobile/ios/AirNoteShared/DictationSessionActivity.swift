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
        public init(listening: Bool) { self.listening = listening }
    }

    public init() {}
}
#endif
