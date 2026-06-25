import AppKit

/// Borderless, transparent, always-on-top panel that hosts the HUD. Floats
/// above the menu bar and over full-screen apps, never steals focus. Mirrors
/// boring.notch's `BoringNotchSkyLightWindow` config.
final class NotchPanel: NSPanel {
    /// When false the panel is fully click-through; flipped on for cards.
    private var interactive = false

    init(contentRect: NSRect) {
        super.init(
            contentRect: contentRect,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        isFloatingPanel = true
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isMovable = false
        isMovableByWindowBackground = false
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        // Above the menu bar so it can overlap the physical notch, like the OS HUD.
        level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        ignoresMouseEvents = true
        appearance = NSAppearance(named: .darkAqua)
    }

    /// Toggle pointer handling. Passive (dictation/toasts) stays click-through so
    /// the user keeps interacting with their app; cards capture the mouse.
    func setInteractive(_ on: Bool) {
        interactive = on
        ignoresMouseEvents = !on
    }

    // Buttons / row toggles need key status, but `.nonactivatingPanel` keeps the
    // user's frontmost app active — we never steal their focus.
    override var canBecomeKey: Bool { interactive }
    override var canBecomeMain: Bool { false }
}
