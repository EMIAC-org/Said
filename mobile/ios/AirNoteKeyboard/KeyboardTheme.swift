import UIKit

enum KeyboardTheme {
    static let accent = UIColor(red: 0.62, green: 0.70, blue: 0.98, alpha: 1.0)
    static let teal = UIColor(red: 0.62, green: 0.70, blue: 0.98, alpha: 1.0)
    static let success = UIColor(red: 0.53, green: 0.82, blue: 0.61, alpha: 1.0)
    static let warning = UIColor(red: 0.98, green: 0.70, blue: 0.30, alpha: 1.0)
    static let danger = UIColor(red: 0.94, green: 0.30, blue: 0.36, alpha: 1.0)
    static var primaryButtonBackground: UIColor {
        adaptiveColor(
            dark: UIColor(white: 0.98, alpha: 1.0),
            light: UIColor(red: 0.045, green: 0.045, blue: 0.060, alpha: 1.0)
        )
    }

    static var primaryButtonForeground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.045, green: 0.045, blue: 0.060, alpha: 1.0),
            light: UIColor.white
        )
    }

    static var foreground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.93, green: 0.93, blue: 0.95, alpha: 1.0),
            light: UIColor(red: 0.07, green: 0.075, blue: 0.10, alpha: 1.0)
        )
    }

    static var muted: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.58, green: 0.59, blue: 0.64, alpha: 1.0),
            light: UIColor(red: 0.42, green: 0.44, blue: 0.52, alpha: 1.0)
        )
    }

    static var secondaryButtonForeground: UIColor { foreground }
    static let radius: CGFloat = 8
    static let surfaceRadius: CGFloat = 18
    static let tileRadius: CGFloat = 12
    static let keyHeight: CGFloat = 40
    static let actionHeight: CGFloat = 36

    /// Deeper accent stop for the Mic Orb's vertical gradient (premium sphere look).
    static let accentDeep = UIColor(red: 0.42, green: 0.52, blue: 0.92, alpha: 1.0)
    static var accentGradientStops: [CGColor] { [accent.cgColor, accentDeep.cgColor] }
    static var recordingGradientStops: [CGColor] {
        [UIColor(red: 0.98, green: 0.42, blue: 0.46, alpha: 1.0).cgColor, danger.cgColor]
    }
    /// Soft, ink-tinted card shadow (reads modern on white; pure black on dark).
    static var cardShadow: UIColor {
        adaptiveColor(
            dark: UIColor.black,
            light: UIColor(red: 0.07, green: 0.09, blue: 0.18, alpha: 1.0)
        )
    }

    static var keyboardBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.110, green: 0.110, blue: 0.125, alpha: 1.0),
            light: UIColor(red: 0.820, green: 0.831, blue: 0.859, alpha: 1.0)
        )
    }

    static var surfaceBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.165, green: 0.165, blue: 0.185, alpha: 1.0),
            light: UIColor.white
        )
    }

    static var keyBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.302, green: 0.302, blue: 0.337, alpha: 1.0),
            light: UIColor.white
        )
    }

    static var secondarySurface: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.204, green: 0.204, blue: 0.235, alpha: 1.0),
            light: UIColor(red: 0.706, green: 0.722, blue: 0.761, alpha: 1.0)
        )
    }

    static var border: UIColor {
        adaptiveColor(
            dark: UIColor.white.withAlphaComponent(0.09),
            light: UIColor(red: 0.07, green: 0.075, blue: 0.10, alpha: 0.12)
        )
    }

    private static func adaptiveColor(dark: UIColor, light: UIColor) -> UIColor {
        UIColor { traits in
            traits.userInterfaceStyle == .dark ? dark : light
        }
    }
}
