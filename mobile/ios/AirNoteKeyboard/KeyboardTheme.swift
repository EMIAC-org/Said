import UIKit

enum KeyboardTheme {
    static let accent = UIColor(red: 0.31, green: 0.40, blue: 0.92, alpha: 1.0)
    static let teal = UIColor(red: 0.07, green: 0.62, blue: 0.85, alpha: 1.0)
    static let success = UIColor(red: 0.05, green: 0.52, blue: 0.30, alpha: 1.0)
    static let warning = UIColor(red: 0.82, green: 0.46, blue: 0.0, alpha: 1.0)
    static let danger = UIColor(red: 0.78, green: 0.17, blue: 0.14, alpha: 1.0)
    static let radius: CGFloat = 8
    static let keyHeight: CGFloat = 40
    static let actionHeight: CGFloat = 36

    static var keyboardBackground: UIColor {
        UIColor { trait in
            trait.userInterfaceStyle == .dark ? UIColor(red: 0.08, green: 0.09, blue: 0.10, alpha: 1) : UIColor.systemGray6
        }
    }

    static var surfaceBackground: UIColor {
        UIColor { trait in
            trait.userInterfaceStyle == .dark ? UIColor(red: 0.13, green: 0.14, blue: 0.16, alpha: 1) : UIColor.systemBackground
        }
    }

    static var keyBackground: UIColor {
        UIColor { trait in
            trait.userInterfaceStyle == .dark ? UIColor(red: 0.20, green: 0.21, blue: 0.23, alpha: 1) : UIColor.white
        }
    }
}
