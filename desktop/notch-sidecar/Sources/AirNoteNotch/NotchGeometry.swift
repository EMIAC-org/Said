import AppKit

/// Measured notch metrics for the active screen.
struct NotchMetrics {
    let hasNotch: Bool
    let closedSize: CGSize
    let screen: NSScreen
}

/// Detects the physical notch (size + which screen) and the non-notch fallback.
/// Mirrors boring.notch `getClosedNotchSize`.
enum NotchGeometry {
    /// Default closed pill for flat displays (no physical cutout).
    static let flatClosed = CGSize(width: 200, height: 32)

    static func current() -> NotchMetrics {
        guard let screen = notchScreen() ?? NSScreen.main else {
            // Headless / no screen: return something sane.
            return NotchMetrics(hasNotch: false, closedSize: flatClosed,
                                screen: NSScreen.screens.first ?? NSScreen()) // never nil in practice
        }

        let insetTop = screen.safeAreaInsets.top
        let hasNotch = insetTop > 0

        var width: CGFloat = hasNotch ? 200 : flatClosed.width
        if let left = screen.auxiliaryTopLeftArea?.width,
           let right = screen.auxiliaryTopRightArea?.width {
            // Exact notch width = screen width minus the two menu-bar wings.
            width = screen.frame.width - left - right + 4
        }

        let height: CGFloat = hasNotch ? insetTop : flatClosed.height
        let closed = CGSize(width: max(width, 180), height: max(height, 32))
        return NotchMetrics(hasNotch: hasNotch, closedSize: closed, screen: screen)
    }

    /// First screen that actually has a notch, else the main screen.
    private static func notchScreen() -> NSScreen? {
        NSScreen.screens.first { $0.safeAreaInsets.top > 0 } ?? NSScreen.main
    }

    /// Frame for the panel given a content size, anchored top-centre on `screen`.
    /// `topGap` lets flat displays float a few points below the bezel.
    static func frame(for size: CGSize, on screen: NSScreen, topGap: CGFloat) -> NSRect {
        let f = screen.frame
        let x = f.midX - size.width / 2
        let y = f.maxY - size.height - topGap
        return NSRect(x: x.rounded(), y: y.rounded(), width: size.width, height: size.height)
    }
}
