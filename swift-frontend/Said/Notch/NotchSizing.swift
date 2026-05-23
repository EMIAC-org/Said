import AppKit

let shadowPadding: CGFloat = 20
let notchCornerRadius = (
    opened: (top: CGFloat(19), bottom: CGFloat(24)),
    closed: (top: CGFloat(6), bottom: CGFloat(14))
)

struct NotchMetrics {
    var hasNotch: Bool
    var notchWidth: CGFloat
    var notchHeight: CGFloat
    var screenFrame: CGRect

    var closedSize: CGSize {
        CGSize(width: notchWidth, height: notchHeight)
    }

    var windowSize: CGSize {
        CGSize(width: 640, height: 190 + shadowPadding)
    }
}

enum NotchDetector {
    static func detect(screen: NSScreen? = nil) -> NotchMetrics {
        let screen = screen ?? NSScreen.main ?? NSScreen.screens.first!
        let frame = screen.frame
        let safeTop = screen.safeAreaInsets.top
        let hasNotch = safeTop > 0

        var notchWidth: CGFloat = 185
        var notchHeight: CGFloat = hasNotch ? safeTop : (frame.maxY - screen.visibleFrame.maxY)

        if hasNotch,
           let leftArea = screen.auxiliaryTopLeftArea,
           let rightArea = screen.auxiliaryTopRightArea {
            notchWidth = frame.width - leftArea.width - rightArea.width + 4
        }

        if notchHeight < 24 { notchHeight = 24 }

        return NotchMetrics(
            hasNotch: hasNotch,
            notchWidth: notchWidth,
            notchHeight: notchHeight,
            screenFrame: frame
        )
    }
}
