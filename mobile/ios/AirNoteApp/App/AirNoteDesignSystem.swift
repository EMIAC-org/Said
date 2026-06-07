import Foundation
import SwiftUI

// MARK: - Design tokens

enum AirNoteDesign {
    // Brand palette — light-mode first. Indigo → cyan is the AirNote signature.
    static let accent = Color(red: 0.36, green: 0.40, blue: 0.96)   // indigo
    static let accent2 = Color(red: 0.10, green: 0.66, blue: 0.91)  // cyan
    static let teal = Color(red: 0.07, green: 0.62, blue: 0.85)
    static let success = Color(red: 0.10, green: 0.62, blue: 0.40)
    static let warning = Color(red: 0.92, green: 0.55, blue: 0.10)
    static let danger = Color(red: 0.94, green: 0.28, blue: 0.33)   // recording red
    static let ink = Color(red: 0.09, green: 0.10, blue: 0.16)

    static let radius: CGFloat = 8
    static let cardRadius: CGFloat = 22
    static let tileRadius: CGFloat = 18

    static var accentGradient: LinearGradient {
        LinearGradient(colors: [accent, accent2], startPoint: .topLeading, endPoint: .bottomTrailing)
    }
    static var recordingGradient: LinearGradient {
        LinearGradient(colors: [Color(red: 0.98, green: 0.38, blue: 0.45), danger],
                       startPoint: .top, endPoint: .bottom)
    }
    static var softCardFill: Color { Color(.secondarySystemBackground) }

    static let cardShadow = Color.black.opacity(0.06)
}

// MARK: - Ambient background (calm, premium, light-first)

struct AirNoteBackground: View {
    var tint: Color = AirNoteDesign.accent

    var body: some View {
        ZStack {
            Color(.systemBackground)
            RadialGradient(
                colors: [tint.opacity(0.16), .clear],
                center: .topTrailing, startRadius: 8, endRadius: 460
            )
            RadialGradient(
                colors: [AirNoteDesign.accent2.opacity(0.12), .clear],
                center: .bottomLeading, startRadius: 8, endRadius: 420
            )
        }
        .ignoresSafeArea()
    }
}

// MARK: - Card

struct AirNoteCard<Content: View>: View {
    var padding: CGFloat = 18
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                    .fill(Color(.secondarySystemBackground))
            )
            .overlay(
                RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.05), lineWidth: 1)
            )
            .shadow(color: AirNoteDesign.cardShadow, radius: 18, x: 0, y: 10)
    }
}

// MARK: - Buttons

struct AirNotePrimaryButtonStyle: ButtonStyle {
    var gradient: LinearGradient = AirNoteDesign.accentGradient
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity)
            .frame(height: 52)
            .background(gradient, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .shadow(color: AirNoteDesign.accent.opacity(configuration.isPressed ? 0.18 : 0.32),
                    radius: configuration.isPressed ? 8 : 16, x: 0, y: 8)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.easeOut(duration: 0.15), value: configuration.isPressed)
    }
}

struct AirNoteGhostButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundStyle(AirNoteDesign.ink)
            .frame(maxWidth: .infinity)
            .frame(height: 52)
            .background(
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .fill(Color(.tertiarySystemBackground))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.08), lineWidth: 1)
            )
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.easeOut(duration: 0.15), value: configuration.isPressed)
    }
}

// MARK: - Status pill

struct AirNoteStatusPill: View {
    var systemImage: String
    var text: String
    var color: Color = AirNoteDesign.accent
    var animated: Bool = false
    @State private var pulse = false

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: systemImage)
                .imageScale(.small)
                .opacity(animated && pulse ? 0.4 : 1)
            Text(text)
        }
        .font(.caption.weight(.bold))
        .foregroundStyle(color)
        .padding(.horizontal, 11)
        .padding(.vertical, 6)
        .background(color.opacity(0.14), in: Capsule())
        .onAppear {
            guard animated else { return }
            withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { pulse = true }
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Action row (primary + secondary)

struct AirNoteActionRow: View {
    var primaryTitle: String
    var primarySystemImage: String
    var secondaryTitle: String
    var secondarySystemImage: String
    var primaryAction: () -> Void
    var secondaryAction: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Button(action: primaryAction) {
                Label(primaryTitle, systemImage: primarySystemImage)
            }
            .buttonStyle(AirNotePrimaryButtonStyle())

            Button(action: secondaryAction) {
                Label(secondaryTitle, systemImage: secondarySystemImage)
            }
            .buttonStyle(AirNoteGhostButtonStyle())
            .frame(maxWidth: 140)
        }
    }
}

// MARK: - Mic orb (the hero voice control)

struct MicOrb: View {
    var isRecording: Bool
    var level: CGFloat = 0
    var size: CGFloat = 116
    var action: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pulse = false

    var body: some View {
        Button(action: action) {
            ZStack {
                // expanding glow rings while recording
                if isRecording && !reduceMotion {
                    Circle()
                        .stroke(AirNoteDesign.danger.opacity(0.35), lineWidth: 2)
                        .frame(width: size, height: size)
                        .scaleEffect(pulse ? 1.6 : 1.0)
                        .opacity(pulse ? 0 : 0.8)
                    Circle()
                        .stroke(AirNoteDesign.danger.opacity(0.25), lineWidth: 2)
                        .frame(width: size, height: size)
                        .scaleEffect(pulse ? 1.35 : 1.0)
                        .opacity(pulse ? 0 : 0.6)
                }
                Circle()
                    .fill(isRecording ? AirNoteDesign.recordingGradient : AirNoteDesign.accentGradient)
                    .frame(width: size, height: size)
                    .shadow(color: (isRecording ? AirNoteDesign.danger : AirNoteDesign.accent).opacity(0.45),
                            radius: 26, x: 0, y: 12)
                    .scaleEffect(isRecording ? 1.0 + min(0.08, level * 0.12) : 1.0)
                Image(systemName: isRecording ? "stop.fill" : "mic.fill")
                    .font(.system(size: size * 0.32, weight: .bold))
                    .foregroundStyle(.white)
            }
        }
        .buttonStyle(.plain)
        .onChange(of: isRecording) { _, recording in
            if recording && !reduceMotion {
                withAnimation(.easeOut(duration: 1.4).repeatForever(autoreverses: false)) { pulse = true }
            } else {
                pulse = false
            }
        }
        .accessibilityLabel(isRecording ? "Stop recording" : "Start recording")
    }
}

// MARK: - Waveform (reacts to level; reduce-motion aware)

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
        .frame(height: 56)
        .frame(maxWidth: .infinity)
        .accessibilityHidden(true)
    }

    private var staticBars: some View {
        HStack(spacing: 6) {
            ForEach(0..<barCount, id: \.self) { i in
                bar(height: 14 + CGFloat((i * 7) % 22))
            }
        }
    }

    private func bar(height: CGFloat) -> some View {
        Capsule()
            .fill(color)
            .frame(width: 7, height: max(8, height))
    }

    private func animatedHeight(_ i: Int, _ t: Double) -> CGFloat {
        let phase = Double(i) * 0.55
        let wave = (sin(t * 6 + phase) + 1) / 2          // 0...1
        let amp = 16 + level * 36                          // louder = taller
        return 10 + CGFloat(wave) * amp
    }
}

// MARK: - Section label

struct AirNoteSectionLabel: View {
    var text: String
    var body: some View {
        Text(text.uppercased())
            .font(.caption.weight(.bold))
            .tracking(0.6)
            .foregroundStyle(.secondary)
    }
}
