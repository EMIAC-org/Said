import UIKit

enum KeyboardTheme {
    static let accent = UIColor(red: 0.62, green: 0.70, blue: 0.98, alpha: 1.0)
    static let teal = UIColor(red: 0.62, green: 0.70, blue: 0.98, alpha: 1.0)
    static let success = UIColor(red: 0.53, green: 0.82, blue: 0.61, alpha: 1.0)
    static let warning = UIColor(red: 0.98, green: 0.70, blue: 0.30, alpha: 1.0)
    static let danger = UIColor(red: 0.94, green: 0.30, blue: 0.36, alpha: 1.0)
    static let primaryButtonBackground = UIColor(white: 0.98, alpha: 1.0)
    static let primaryButtonForeground = UIColor(red: 0.045, green: 0.045, blue: 0.060, alpha: 1.0)
    static let radius: CGFloat = 8
    static let keyHeight: CGFloat = 40
    static let actionHeight: CGFloat = 36

    static var keyboardBackground: UIColor {
        UIColor(red: 0.025, green: 0.025, blue: 0.035, alpha: 1.0)
    }

    static var surfaceBackground: UIColor {
        UIColor(red: 0.055, green: 0.055, blue: 0.075, alpha: 1.0)
    }

    static var keyBackground: UIColor {
        UIColor(red: 0.125, green: 0.125, blue: 0.155, alpha: 1.0)
    }

    static var secondarySurface: UIColor {
        UIColor(red: 0.085, green: 0.085, blue: 0.110, alpha: 1.0)
    }

    static var border: UIColor {
        UIColor.white.withAlphaComponent(0.09)
    }
}
