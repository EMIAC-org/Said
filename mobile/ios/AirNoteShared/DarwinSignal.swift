import Foundation

/// Lightweight cross-process notifications between the app and its keyboard
/// extension via the Darwin notification center. Used to drive the warm
/// dictation session (keyboard ⇄ app) without polling.
public final class DarwinSignal {
    public static let shared = DarwinSignal()

    /// Keyboard → app: start a dictation on the warm session.
    public static let startDictation = "com.emiac.airnote.signal.start"
    /// Keyboard → app: stop the current dictation now.
    public static let stopDictation = "com.emiac.airnote.signal.stop"
    /// App → keyboard: a dictation began (the warm app is alive + recording).
    public static let dictationAck = "com.emiac.airnote.signal.ack"
    /// App → keyboard: the polished result is ready in the App Group.
    public static let resultReady = "com.emiac.airnote.signal.result"
    /// App → keyboard: the dictation failed / was cancelled.
    public static let dictationFailed = "com.emiac.airnote.signal.failed"
    /// App → keyboard: a new live (romanized) partial transcript is in
    /// SharedStore.keyboardLivePartial — show words as the user speaks.
    public static let livePartial = "com.emiac.airnote.signal.partial"

    private var handlers: [String: () -> Void] = [:]
    private let lock = NSLock()
    private let center = CFNotificationCenterGetDarwinNotifyCenter()

    private init() {}

    public func post(_ name: String) {
        CFNotificationCenterPostNotification(center, CFNotificationName(name as CFString), nil, nil, true)
    }

    public func observe(_ name: String, handler: @escaping () -> Void) {
        let observer = Unmanaged.passUnretained(self).toOpaque()
        // Idempotent: drop any prior registration for this name first.
        CFNotificationCenterRemoveObserver(center, observer, CFNotificationName(name as CFString), nil)
        lock.lock()
        handlers[name] = handler
        lock.unlock()

        CFNotificationCenterAddObserver(
            center,
            observer,
            { _, _, name, _, _ in
                guard let raw = name?.rawValue as String? else { return }
                DispatchQueue.main.async {
                    DarwinSignal.shared.fire(raw)
                }
            },
            name as CFString,
            nil,
            .deliverImmediately
        )
    }

    public func stopObserving(_ name: String) {
        lock.lock()
        handlers[name] = nil
        lock.unlock()
        let observer = Unmanaged.passUnretained(self).toOpaque()
        CFNotificationCenterRemoveObserver(center, observer, CFNotificationName(name as CFString), nil)
    }

    private func fire(_ name: String) {
        lock.lock()
        let handler = handlers[name]
        lock.unlock()
        handler?()
    }
}
