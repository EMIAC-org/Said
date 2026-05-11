import AppKit
import os

final class NotchSpaceManager {
    static let shared = NotchSpaceManager()
    private let logger = RuntimeLogger(category: "notch-space")

    func addWindow(_ window: NSWindow) {
        window.collectionBehavior = [.fullScreenAuxiliary, .stationary, .canJoinAllSpaces, .ignoresCycle]
        logger.info("notch window configured (level=\(window.level.rawValue))")
    }
}
