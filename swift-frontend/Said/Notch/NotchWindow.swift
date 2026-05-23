import AppKit
import SwiftUI

final class NotchWindow: NSPanel {
    init(metrics: NotchMetrics, screen: NSScreen) {
        let rect = NSRect(origin: .zero, size: metrics.windowSize)
        super.init(
            contentRect: rect,
            styleMask: [.borderless, .nonactivatingPanel, .utilityWindow, .hudWindow],
            backing: .buffered,
            defer: false
        )
        isFloatingPanel = true
        level = .mainMenu + 3
        collectionBehavior = [.fullScreenAuxiliary, .stationary, .canJoinAllSpaces, .ignoresCycle]
        isOpaque = false
        hasShadow = false
        backgroundColor = .clear
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        isMovableByWindowBackground = false
        hidesOnDeactivate = false
        appearance = NSAppearance(named: .darkAqua)

        position(on: screen)
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    func position(on screen: NSScreen) {
        let screenFrame = screen.frame
        setFrameOrigin(NSPoint(
            x: screenFrame.origin.x + (screenFrame.width / 2) - frame.width / 2,
            y: screenFrame.origin.y + screenFrame.height - frame.height
        ))
    }
}
