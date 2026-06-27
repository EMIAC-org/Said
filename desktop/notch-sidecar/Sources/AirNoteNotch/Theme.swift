import SwiftUI

// Palette mirrors the Tauri status bar (StatusBar.tsx / styles.css).
// Accent is hsl(226 80% 78%) — periwinkle — converted to sRGB.
enum Theme {
    static let accent     = Color(red: 0.604, green: 0.686, blue: 0.956)
    static let accentGlow = accent.opacity(0.30)
    static let accentSoft = accent.opacity(0.14)

    static let ok    = Color(red: 0.204, green: 0.827, blue: 0.600) // #34d399
    static let warn  = Color(red: 1.000, green: 0.835, blue: 0.039) // #ffd60a
    static let amber = Color(red: 1.000, green: 0.784, blue: 0.243) // recording indicator
    static let err   = Color(red: 1.000, green: 0.271, blue: 0.227) // #ff453a
    static let rec   = Color(red: 0.930, green: 0.360, blue: 0.400) // hsl(354 85% 62%)

    static let ink      = Color(white: 0.92)
    static let inkDim    = Color(white: 0.60)
    static let inkFaint  = Color(white: 0.42)
    static let hairline  = Color.white.opacity(0.08)

    // Notch corner radii (boring.notch values).
    static let topRadiusClosed: CGFloat = 6
    static let topRadiusOpen: CGFloat = 10
    static let bottomRadiusClosed: CGFloat = 13
    static let bottomRadiusOpen: CGFloat = 22

    // Spring used for the expand/collapse, both the AppKit frame and SwiftUI content.
    static let spring = Animation.spring(response: 0.42, dampingFraction: 0.82)
    static let springClose = Animation.spring(response: 0.40, dampingFraction: 1.0)
}
