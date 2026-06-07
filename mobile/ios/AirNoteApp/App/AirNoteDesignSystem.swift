import Foundation
import SwiftUI
import UIKit

// MARK: - Desktop-aligned design tokens

enum AirNoteDesign {
    static let background = adaptiveColor(
        dark: UIColor(red: 0.025, green: 0.025, blue: 0.035, alpha: 1),
        light: UIColor(red: 0.962, green: 0.966, blue: 0.980, alpha: 1)
    )
    static let background2 = adaptiveColor(
        dark: UIColor(red: 0.035, green: 0.035, blue: 0.048, alpha: 1),
        light: UIColor(red: 0.988, green: 0.990, blue: 0.996, alpha: 1)
    )
    static let surface = adaptiveColor(
        dark: UIColor(red: 0.055, green: 0.055, blue: 0.075, alpha: 1),
        light: UIColor(red: 1.000, green: 1.000, blue: 1.000, alpha: 1)
    )
    static let surfaceRaised = adaptiveColor(
        dark: UIColor(red: 0.085, green: 0.085, blue: 0.110, alpha: 1),
        light: UIColor(red: 0.930, green: 0.940, blue: 0.968, alpha: 1)
    )
    static let surfaceHover = adaptiveColor(
        dark: UIColor(red: 0.120, green: 0.120, blue: 0.150, alpha: 1),
        light: UIColor(red: 0.875, green: 0.890, blue: 0.930, alpha: 1)
    )
    static let foreground = adaptiveColor(
        dark: UIColor(red: 0.930, green: 0.930, blue: 0.950, alpha: 1),
        light: UIColor(red: 0.070, green: 0.075, blue: 0.100, alpha: 1)
    )
    static let muted = adaptiveColor(
        dark: UIColor(red: 0.580, green: 0.590, blue: 0.640, alpha: 1),
        light: UIColor(red: 0.420, green: 0.440, blue: 0.520, alpha: 1)
    )
    static let border = adaptiveColor(
        dark: UIColor.white.withAlphaComponent(0.070),
        light: UIColor(red: 0.070, green: 0.075, blue: 0.100, alpha: 0.090)
    )
    static let borderStrong = adaptiveColor(
        dark: UIColor.white.withAlphaComponent(0.115),
        light: UIColor(red: 0.070, green: 0.075, blue: 0.100, alpha: 0.145)
    )

    static let accent = Color(red: 0.620, green: 0.700, blue: 0.980)
    static let accent2 = accent
    static let teal = Color(red: 0.620, green: 0.700, blue: 0.980)
    static let success = Color(red: 0.530, green: 0.820, blue: 0.610)
    static let warning = Color(red: 0.980, green: 0.700, blue: 0.300)
    static let danger = Color(red: 0.940, green: 0.300, blue: 0.360)
    static let ink = Color(red: 0.045, green: 0.045, blue: 0.060)
    static let primaryButtonFill = adaptiveColor(
        dark: UIColor(white: 0.98, alpha: 1),
        light: UIColor(red: 0.045, green: 0.045, blue: 0.060, alpha: 1)
    )
    static let primaryButtonForeground = adaptiveColor(
        dark: UIColor(red: 0.045, green: 0.045, blue: 0.060, alpha: 1),
        light: UIColor.white
    )
    static let keyboardWell = adaptiveColor(
        dark: UIColor(red: 0.035, green: 0.035, blue: 0.045, alpha: 1),
        light: UIColor(red: 0.885, green: 0.895, blue: 0.930, alpha: 1)
    )

    static let radius: CGFloat = 8
    static let cardRadius: CGFloat = 12
    static let tileRadius: CGFloat = 10

    static var accentGradient: LinearGradient {
        LinearGradient(colors: [accent.opacity(0.95), accent.opacity(0.72)],
                       startPoint: .topLeading,
                       endPoint: .bottomTrailing)
    }

    static var recordingGradient: LinearGradient {
        LinearGradient(colors: [Color(red: 0.98, green: 0.38, blue: 0.45), danger],
                       startPoint: .top,
                       endPoint: .bottom)
    }

    static var softCardFill: Color { surfaceRaised }
    static let cardShadow = adaptiveColor(
        dark: UIColor.black.withAlphaComponent(0.38),
        light: UIColor(red: 0.060, green: 0.070, blue: 0.110, alpha: 0.12)
    )

    private static func adaptiveColor(dark: UIColor, light: UIColor) -> Color {
        Color(UIColor { traits in
            traits.userInterfaceStyle == .dark ? dark : light
        })
    }
}

enum AirNoteAppearance: String {
    case dark
    case light

    var colorScheme: ColorScheme {
        switch self {
        case .dark: return .dark
        case .light: return .light
        }
    }

    var next: AirNoteAppearance {
        switch self {
        case .dark: return .light
        case .light: return .dark
        }
    }
}

private struct AirNoteAppearanceModifier: ViewModifier {
    @AppStorage("airnotePreferredAppearance") private var appearance = AirNoteAppearance.dark.rawValue

    func body(content: Content) -> some View {
        let mode = AirNoteAppearance(rawValue: appearance) ?? .dark
        content.preferredColorScheme(mode.colorScheme)
    }
}

extension View {
    func airNotePreferredAppearance() -> some View {
        modifier(AirNoteAppearanceModifier())
    }
}

struct AirNoteAppearanceToggle: View {
    @AppStorage("airnotePreferredAppearance") private var appearance = AirNoteAppearance.dark.rawValue

    var body: some View {
        let mode = AirNoteAppearance(rawValue: appearance) ?? .dark
        Button {
            withAnimation(.easeInOut(duration: 0.18)) {
                appearance = mode.next.rawValue
            }
        } label: {
            Label(mode == .dark ? "Light" : "Dark",
                  systemImage: mode == .dark ? "sun.max.fill" : "moon.fill")
                .font(.caption2.weight(.bold))
                .labelStyle(.titleAndIcon)
                .padding(.horizontal, 9)
                .padding(.vertical, 6)
                .foregroundStyle(AirNoteDesign.accent)
                .background(AirNoteDesign.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .strokeBorder(AirNoteDesign.accent.opacity(0.20), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(mode == .dark ? "Switch to light mode" : "Switch to dark mode")
    }
}

// MARK: - Background

struct AirNoteBackground: View {
    var tint: Color = AirNoteDesign.accent

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [
                    AirNoteDesign.background,
                    AirNoteDesign.background2,
                    AirNoteDesign.background
                ],
                startPoint: .topTrailing,
                endPoint: .bottomLeading
            )
            LinearGradient(
                colors: [AirNoteDesign.surface.opacity(0.16), .clear],
                startPoint: .top,
                endPoint: .bottom
            )
        }
        .ignoresSafeArea()
    }
}

// MARK: - Shared surfaces

struct AirNoteCard<Content: View>: View {
    var padding: CGFloat = 16
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                    .fill(AirNoteDesign.surface.opacity(0.92))
                    .shadow(color: AirNoteDesign.cardShadow, radius: 22, x: 0, y: 12)
            )
            .overlay(
                RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                    .strokeBorder(AirNoteDesign.border, lineWidth: 1)
            )
    }
}

struct AirNoteLogoTile: View {
    var size: CGFloat = 44

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
            .fill(
                LinearGradient(
                    colors: [AirNoteDesign.surfaceRaised, AirNoteDesign.surface],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            )
            .frame(width: size, height: size)
            .overlay(AirNoteWaveMark(size: size * 0.46))
            .overlay(
                RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
                    .strokeBorder(AirNoteDesign.borderStrong, lineWidth: 1)
            )
            .shadow(color: Color.black.opacity(0.35), radius: 18, x: 0, y: 10)
    }
}

struct AirNoteWaveMark: View {
    var size: CGFloat = 22
    private let heights: [CGFloat] = [0.38, 0.78, 0.55, 0.92]

    var body: some View {
        HStack(alignment: .center, spacing: size * 0.11) {
            ForEach(0..<heights.count, id: \.self) { index in
                Capsule()
                    .fill(AirNoteDesign.foreground)
                    .frame(width: size * 0.15, height: size * heights[index])
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: - Buttons

struct AirNotePrimaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(.subheadline, design: .default).weight(.semibold))
            .foregroundStyle(AirNoteDesign.primaryButtonForeground)
            .frame(maxWidth: .infinity)
            .frame(height: 44)
            .background(AirNoteDesign.primaryButtonFill.opacity(configuration.isPressed ? 0.88 : 1),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(AirNoteDesign.borderStrong, lineWidth: 1)
            )
            .shadow(color: Color.black.opacity(configuration.isPressed ? 0.18 : 0.30),
                    radius: configuration.isPressed ? 6 : 16,
                    x: 0,
                    y: configuration.isPressed ? 3 : 8)
            .scaleEffect(configuration.isPressed ? 0.985 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

struct AirNoteGhostButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(.subheadline, design: .default).weight(.semibold))
            .foregroundStyle(AirNoteDesign.foreground)
            .frame(maxWidth: .infinity)
            .frame(height: 44)
            .background(AirNoteDesign.surfaceRaised.opacity(configuration.isPressed ? 0.72 : 0.52),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(AirNoteDesign.borderStrong, lineWidth: 1)
            )
            .scaleEffect(configuration.isPressed ? 0.985 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

// MARK: - Status + rows

struct AirNoteStatusPill: View {
    var systemImage: String
    var text: String
    var color: Color = AirNoteDesign.accent
    var animated: Bool = false
    @State private var pulse = false

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
                .opacity(animated && pulse ? 0.45 : 1)
            Image(systemName: systemImage)
                .imageScale(.small)
            Text(text)
        }
        .font(.caption2.weight(.bold))
        .foregroundStyle(color)
        .padding(.horizontal, 9)
        .padding(.vertical, 6)
        .background(color.opacity(0.14), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(color.opacity(0.20), lineWidth: 1)
        )
        .onAppear {
            guard animated else { return }
            withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) {
                pulse = true
            }
        }
        .accessibilityElement(children: .combine)
    }
}

struct AirNoteActionRow: View {
    var primaryTitle: String
    var primarySystemImage: String
    var secondaryTitle: String
    var secondarySystemImage: String
    var primaryAction: () -> Void
    var secondaryAction: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Button(action: primaryAction) {
                Label(primaryTitle, systemImage: primarySystemImage)
            }
            .buttonStyle(AirNotePrimaryButtonStyle())

            Button(action: secondaryAction) {
                Label(secondaryTitle, systemImage: secondarySystemImage)
            }
            .buttonStyle(AirNoteGhostButtonStyle())
            .frame(maxWidth: 132)
        }
    }
}

struct AirNoteSetupRow: View {
    var icon: String
    var title: String
    var subtitle: String
    var status: String?
    var tint: Color = AirNoteDesign.accent

    var body: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(Color.white.opacity(0.045))
                .frame(width: 34, height: 34)
                .overlay(Image(systemName: icon).font(.system(size: 14, weight: .semibold)).foregroundStyle(tint))
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(AirNoteDesign.border, lineWidth: 1)
                )
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            if let status {
                Text(status)
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(tint)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            }
        }
        .padding(12)
        .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(AirNoteDesign.border, lineWidth: 1)
        )
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Mic orb + waveform

struct MicOrb: View {
    var isRecording: Bool
    var level: CGFloat = 0
    var size: CGFloat = 104
    var action: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pulse = false

    var body: some View {
        Button(action: action) {
            ZStack {
                if isRecording && !reduceMotion {
                    Circle()
                        .stroke(AirNoteDesign.danger.opacity(0.35), lineWidth: 2)
                        .frame(width: size, height: size)
                        .scaleEffect(pulse ? 1.55 : 1.0)
                        .opacity(pulse ? 0 : 0.8)
                }
                Circle()
                    .fill(isRecording ? AirNoteDesign.recordingGradient : LinearGradient(colors: [Color.white, Color.white.opacity(0.88)], startPoint: .top, endPoint: .bottom))
                    .frame(width: size, height: size)
                    .shadow(color: Color.black.opacity(0.42), radius: 24, x: 0, y: 12)
                    .scaleEffect(isRecording ? 1.0 + min(0.08, level * 0.12) : 1.0)
                Image(systemName: isRecording ? "stop.fill" : "mic.fill")
                    .font(.system(size: size * 0.30, weight: .bold))
                    .foregroundStyle(isRecording ? .white : AirNoteDesign.ink)
            }
        }
        .buttonStyle(.plain)
        .onChange(of: isRecording) { _, recording in
            if recording && !reduceMotion {
                withAnimation(.easeOut(duration: 1.4).repeatForever(autoreverses: false)) {
                    pulse = true
                }
            } else {
                pulse = false
            }
        }
        .accessibilityLabel(isRecording ? "Stop recording" : "Start recording")
    }
}

struct AirNoteWaveform: View {
    var level: CGFloat
    var active: Bool
    var barCount: Int = 7
    var color: Color = AirNoteDesign.accent
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Group {
            if reduceMotion || !active {
                staticBars
            } else {
                TimelineView(.animation) { timeline in
                    let t = timeline.date.timeIntervalSinceReferenceDate
                    HStack(spacing: 6) {
                        ForEach(0..<barCount, id: \.self) { i in
                            bar(height: animatedHeight(i, t))
                        }
                    }
                }
            }
        }
        .frame(height: 48)
        .frame(maxWidth: .infinity)
        .accessibilityHidden(true)
    }

    private var staticBars: some View {
        HStack(spacing: 6) {
            ForEach(0..<barCount, id: \.self) { i in
                bar(height: 12 + CGFloat((i * 7) % 20))
            }
        }
    }

    private func bar(height: CGFloat) -> some View {
        Capsule()
            .fill(color.opacity(active ? 0.95 : 0.48))
            .frame(width: 6, height: max(8, height))
    }

    private func animatedHeight(_ i: Int, _ t: Double) -> CGFloat {
        let phase = Double(i) * 0.55
        let wave = (sin(t * 6 + phase) + 1) / 2
        let amp = 14 + level * 34
        return 9 + CGFloat(wave) * amp
    }
}

// MARK: - Typography

struct AirNoteSectionLabel: View {
    var text: String
    var body: some View {
        Text(text.uppercased())
            .font(.caption2.weight(.bold))
            .tracking(0.9)
            .foregroundStyle(AirNoteDesign.muted)
    }
}
