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
    static let keyHeight: CGFloat = 40
    static let actionHeight: CGFloat = 36

    static var keyboardBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.025, green: 0.025, blue: 0.035, alpha: 1.0),
            light: UIColor(red: 0.962, green: 0.966, blue: 0.980, alpha: 1.0)
        )
    }

    static var surfaceBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.055, green: 0.055, blue: 0.075, alpha: 1.0),
            light: UIColor.white
        )
    }

    static var keyBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.125, green: 0.125, blue: 0.155, alpha: 1.0),
            light: UIColor(red: 0.930, green: 0.940, blue: 0.968, alpha: 1.0)
        )
    }

    static var secondarySurface: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.085, green: 0.085, blue: 0.110, alpha: 1.0),
            light: UIColor(red: 0.930, green: 0.940, blue: 0.968, alpha: 1.0)
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
