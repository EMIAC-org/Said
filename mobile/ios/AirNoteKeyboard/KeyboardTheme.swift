import UIKit

/// Visual tokens for the AirNote keyboard.
///
/// These are kept in lockstep with the main app's `AirNoteDesign` system
/// (`mobile/ios/AirNoteApp/App/AirNoteDesignSystem.swift`) so the keyboard reads
/// as the same premium product — inky near-black surfaces in dark, clean in light,
/// a single rationed periwinkle accent, monochrome high-contrast primary buttons,
/// continuous-curve corners, hairline borders and soft ambient shadow. The keyboard
/// extension can't import the app target, so the values are mirrored here by hand:
/// when `AirNoteDesign` changes, change these to match.
enum KeyboardTheme {
    static let accent = UIColor(red: 0.62, green: 0.70, blue: 0.98, alpha: 1.0)
    static let teal = UIColor(red: 0.62, green: 0.70, blue: 0.98, alpha: 1.0)
    static let success = UIColor(red: 0.53, green: 0.82, blue: 0.61, alpha: 1.0)
    static let warning = UIColor(red: 0.98, green: 0.70, blue: 0.30, alpha: 1.0)
    static let danger = UIColor(red: 0.94, green: 0.30, blue: 0.36, alpha: 1.0)

    /// Inky near-black (= `AirNoteDesign.ink`) — the app's light-mode primary fill.
    static let ink = UIColor(red: 0.045, green: 0.045, blue: 0.060, alpha: 1.0)

    /// Monochrome primary button (= `AirNoteDesign.primaryButtonFill`): near-white
    /// on dark, inky on light. The single biggest "premium" tell in the app.
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

    // ── Radii (= AirNoteDesign: radius / tileRadius / cardRadius) ──────────────
    static let radius: CGFloat = 8         // key caps + small controls
    static let surfaceRadius: CGFloat = 12 // voice card + waveform hero (= cardRadius)
    static let tileRadius: CGFloat = 10    // transcript / preview tiles
    static let keyHeight: CGFloat = 40
    static let actionHeight: CGFloat = 36

    /// Deeper accent stop for any gradient badge (premium sphere look).
    static let accentDeep = UIColor(red: 0.42, green: 0.52, blue: 0.92, alpha: 1.0)
    static var accentGradientStops: [CGColor] { [accent.cgColor, accentDeep.cgColor] }
    static var recordingGradientStops: [CGColor] {
        [UIColor(red: 0.98, green: 0.38, blue: 0.45, alpha: 1.0).cgColor, danger.cgColor]
    }

    /// Soft ambient card shadow (= `AirNoteDesign.cardShadow`). The alpha is baked
    /// into the color, so consumers set `shadowOpacity = 1.0`.
    static var cardShadow: UIColor {
        adaptiveColor(
            dark: UIColor.black.withAlphaComponent(0.38),
            light: UIColor(red: 0.060, green: 0.070, blue: 0.110, alpha: 0.12)
        )
    }

    // ── Surfaces (= AirNoteDesign: keyboardWell / surface / surfaceRaised /
    //    surfaceHover) — inky near-black in dark, clean in light ────────────────
    static var keyboardBackground: UIColor {
        adaptiveColor(
            dark: UIColor(red: 0.035, green: 0.035, blue: 0.045, alpha: 1.0),
            light: UIColor(red: 0.885, green: 0.895, blue: 0.930, alpha: 1.0)
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
            dark: UIColor(red: 0.120, green: 0.120, blue: 0.150, alpha: 1.0),
            light: UIColor.white
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
            dark: UIColor.white.withAlphaComponent(0.070),
            light: UIColor(red: 0.070, green: 0.075, blue: 0.100, alpha: 0.090)
        )
    }

    /// Slightly stronger hairline for the voice card / hero / primary button outline.
    static var borderStrong: UIColor {
        adaptiveColor(
            dark: UIColor.white.withAlphaComponent(0.115),
            light: UIColor(red: 0.070, green: 0.075, blue: 0.100, alpha: 0.145)
        )
    }

    private static func adaptiveColor(dark: UIColor, light: UIColor) -> UIColor {
        UIColor { traits in
            traits.userInterfaceStyle == .dark ? dark : light
        }
    }
}
