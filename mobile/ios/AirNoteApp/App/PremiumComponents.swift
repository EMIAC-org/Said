import SwiftUI
import UIKit

// MARK: - Haptics

enum Haptics {
    static func tap() { UIImpactFeedbackGenerator(style: .light).impactOccurred() }
    static func start() { UIImpactFeedbackGenerator(style: .medium).impactOccurred() }
    static func stop() { UIImpactFeedbackGenerator(style: .soft).impactOccurred() }
    static func success() { UINotificationFeedbackGenerator().notificationOccurred(.success) }
    static func warning() { UINotificationFeedbackGenerator().notificationOccurred(.warning) }
}

// MARK: - Where work happens

/// Every step of a dictation runs either on the iPhone or in AirNote's cloud.
/// The UI labels each step with one of these so the split is always visible.
enum ProcessingPlace {
    case onDevice
    case cloud

    var label: String {
        switch self {
        case .onDevice: return "On iPhone"
        case .cloud: return "Cloud"
        }
    }

    var systemImage: String {
        switch self {
        case .onDevice: return "iphone"
        case .cloud: return "cloud.fill"
        }
    }

    var color: Color {
        switch self {
        case .onDevice: return AirNoteDesign.onDevice
        case .cloud: return AirNoteDesign.cloud
        }
    }
}

struct PlaceBadge: View {
    var place: ProcessingPlace
    var text: String?

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: place.systemImage)
                .font(.system(size: 9, weight: .bold))
            Text(text ?? place.label)
                .font(.system(size: 11, weight: .semibold, design: .rounded))
        }
        .foregroundStyle(place.color)
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(place.color.opacity(0.13), in: Capsule())
        .overlay(Capsule().strokeBorder(place.color.opacity(0.22), lineWidth: 0.5))
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Pipeline strip (Home + On-device screen)

/// Record → Speech to text → Polish, each tagged with where it runs.
struct PipelineStrip: View {
    var onDark = false

    private struct Step: Identifiable {
        let id = UUID()
        let icon: String
        let title: String
        let place: ProcessingPlace
    }

    private let steps = [
        Step(icon: "mic.fill", title: "Record", place: .onDevice),
        Step(icon: "waveform", title: "Speech → text", place: .onDevice),
        Step(icon: "sparkles", title: "Polish", place: .cloud),
    ]

    var body: some View {
        HStack(spacing: 6) {
            ForEach(Array(steps.enumerated()), id: \.element.id) { index, step in
                VStack(spacing: 6) {
                    Image(systemName: step.icon)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(onDark ? .white : step.place.color)
                        .frame(width: 34, height: 34)
                        .background(
                            Circle().fill(onDark ? Color.white.opacity(0.14) : step.place.color.opacity(0.13))
                        )
                    Text(step.title)
                        .font(.system(size: 11, weight: .semibold, design: .rounded))
                        .foregroundStyle(onDark ? .white : AirNoteDesign.foreground)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                    HStack(spacing: 3) {
                        Image(systemName: step.place.systemImage).font(.system(size: 8, weight: .bold))
                        Text(step.place.label).font(.system(size: 9.5, weight: .medium, design: .rounded))
                    }
                    .foregroundStyle(onDark ? Color.white.opacity(0.72) : step.place.color)
                }
                .frame(maxWidth: .infinity)
                if index < steps.count - 1 {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(onDark ? Color.white.opacity(0.45) : AirNoteDesign.muted.opacity(0.6))
                        .padding(.bottom, 26)
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Recording and speech to text run on your iPhone. Polish runs in the cloud.")
    }
}

// MARK: - Dictation stage tracker

enum DictationStage: Int, Comparable {
    case idle, listening, transcribing, polishing, done

    static func < (lhs: DictationStage, rhs: DictationStage) -> Bool { lhs.rawValue < rhs.rawValue }
}

/// Live, per-stage progress for one dictation, with where each stage ran and
/// how long it took.
struct StageTracker: View {
    var stage: DictationStage
    var audioSeconds: Double?
    var sttMS: Int?
    var polishMS: Int?
    var polishSkipped = false

    var body: some View {
        VStack(spacing: 0) {
            row(
                icon: "mic.fill",
                title: "Recorded",
                detail: audioSeconds.map { String(format: "%.1f s of audio", $0) } ?? "Listening…",
                place: .onDevice,
                state: state(for: .listening)
            )
            connector(active: stage > .listening)
            row(
                icon: "waveform",
                title: "Speech to text",
                detail: sttMS.map { "Clario Hinglish · \(seconds($0))" } ?? "Clario Hinglish model",
                place: .onDevice,
                state: state(for: .transcribing)
            )
            connector(active: stage > .transcribing)
            row(
                icon: "sparkles",
                title: "Polish",
                detail: polishSkipped ? "Offline — kept your exact words" : (polishMS.map { "Tone & punctuation · \(seconds($0))" } ?? "Punctuation, tone & your words"),
                place: .cloud,
                state: polishSkipped && stage == .done ? .skipped : state(for: .polishing)
            )
        }
    }

    private enum RowState { case pending, active, done, skipped }

    private func state(for rowStage: DictationStage) -> RowState {
        if stage > rowStage { return .done }
        if stage == rowStage { return .active }
        return .pending
    }

    private func seconds(_ ms: Int) -> String {
        ms < 1000 ? "\(ms) ms" : String(format: "%.1f s", Double(ms) / 1000)
    }

    private func row(icon: String, title: String, detail: String, place: ProcessingPlace, state: RowState) -> some View {
        HStack(spacing: 12) {
            ZStack {
                Circle()
                    .fill(state == .pending ? AirNoteDesign.surfaceRaised : place.color.opacity(0.16))
                    .frame(width: 36, height: 36)
                switch state {
                case .done:
                    Image(systemName: "checkmark")
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(place.color)
                        .transition(.scale.combined(with: .opacity))
                case .active:
                    ProgressView().controlSize(.small).tint(place.color)
                case .skipped:
                    Image(systemName: "minus")
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(AirNoteDesign.muted)
                case .pending:
                    Image(systemName: icon)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(AirNoteDesign.muted)
                }
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(.subheadline, design: .rounded).weight(.semibold))
                    .foregroundStyle(state == .pending ? AirNoteDesign.muted : AirNoteDesign.foreground)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .contentTransition(.opacity)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
            }
            Spacer(minLength: 6)
            PlaceBadge(place: place)
                .opacity(state == .pending ? 0.5 : 1)
        }
        .animation(.spring(response: 0.35, dampingFraction: 0.85), value: state)
        .accessibilityElement(children: .combine)
    }

    private func connector(active: Bool) -> some View {
        HStack {
            Capsule()
                .fill(active ? AirNoteDesign.onDevice.opacity(0.5) : AirNoteDesign.border)
                .frame(width: 2, height: 14)
                .padding(.leading, 17)
            Spacer()
        }
    }
}

// MARK: - Hero mic

/// The big dictation button: gradient orb, soft glow, and rings that breathe
/// with the voice level while recording.
struct HeroMicButton: View {
    var isRecording: Bool
    var isBusy: Bool = false
    var level: CGFloat = 0
    var size: CGFloat = 132
    var action: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Button(action: action) {
            ZStack {
                if isRecording && !reduceMotion {
                    ForEach(0..<3) { ring in
                        Circle()
                            .stroke(AirNoteDesign.danger.opacity(0.22 - Double(ring) * 0.06), lineWidth: 2)
                            .frame(width: size, height: size)
                            .scaleEffect(1.12 + CGFloat(ring) * 0.16 + level * (0.35 + CGFloat(ring) * 0.15))
                            .animation(.easeOut(duration: 0.12), value: level)
                    }
                }
                Circle()
                    .fill(isRecording ? AirNoteDesign.recordingGradient : AirNoteDesign.micGradient)
                    .frame(width: size, height: size)
                    .shadow(color: (isRecording ? AirNoteDesign.danger : AirNoteDesign.violet).opacity(0.45), radius: 28, x: 0, y: 14)
                    .overlay(
                        Circle()
                            .strokeBorder(Color.white.opacity(0.28), lineWidth: 1)
                            .padding(1)
                    )
                    .overlay(
                        Circle()
                            .fill(LinearGradient(colors: [Color.white.opacity(0.28), .clear], startPoint: .top, endPoint: .center))
                            .padding(4)
                    )
                if isBusy {
                    ProgressView().controlSize(.large).tint(.white)
                } else {
                    Image(systemName: isRecording ? "stop.fill" : "mic.fill")
                        .font(.system(size: size * 0.30, weight: .semibold))
                        .foregroundStyle(.white)
                        .contentTransition(.symbolEffect(.replace))
                }
            }
            .scaleEffect(isRecording ? 1 + min(0.05, level * 0.08) : 1)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isRecording)
        }
        .buttonStyle(PressScaleStyle())
        .accessibilityLabel(isRecording ? "Stop recording" : "Start recording")
    }
}

struct PressScaleStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.94 : 1)
            .animation(.spring(response: 0.25, dampingFraction: 0.7), value: configuration.isPressed)
    }
}

// MARK: - Small building blocks

/// Rounded icon tile used at the start of list rows.
struct IconTile: View {
    var systemImage: String
    var tint: Color = AirNoteDesign.accent
    var size: CGFloat = 36

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.3, style: .continuous)
            .fill(tint.opacity(0.14))
            .frame(width: size, height: size)
            .overlay(
                Image(systemName: systemImage)
                    .font(.system(size: size * 0.42, weight: .semibold))
                    .foregroundStyle(tint)
            )
            .accessibilityHidden(true)
    }
}

/// "Label ······ value" row for fact lists.
struct FactRow: View {
    var label: String
    var value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(.subheadline)
                .foregroundStyle(AirNoteDesign.muted)
            Spacer(minLength: 12)
            Text(value)
                .font(.system(.subheadline, design: .rounded).weight(.semibold))
                .foregroundStyle(AirNoteDesign.foreground)
                .multilineTextAlignment(.trailing)
        }
        .accessibilityElement(children: .combine)
    }
}

/// A status dot that softly pulses while `pulsing` is true.
struct StatusDot: View {
    var color: Color
    var pulsing = false
    @State private var on = false

    var body: some View {
        ZStack {
            if pulsing {
                Circle().fill(color.opacity(0.35)).frame(width: 14, height: 14).scaleEffect(on ? 1.3 : 0.7).opacity(on ? 0 : 1)
            }
            Circle().fill(color).frame(width: 8, height: 8)
        }
        .frame(width: 14, height: 14)
        .onAppear {
            guard pulsing else { return }
            withAnimation(.easeOut(duration: 1.4).repeatForever(autoreverses: false)) { on = true }
        }
        .accessibilityHidden(true)
    }
}

/// Section title used above cards on scrolling screens.
struct ScreenSectionTitle: View {
    var title: String
    var trailing: String?
    var action: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.system(.title3, design: .rounded).weight(.bold))
                .foregroundStyle(AirNoteDesign.foreground)
            Spacer()
            if let trailing, let action {
                Button(trailing, action: action)
                    .font(.system(.subheadline, design: .rounded).weight(.semibold))
                    .foregroundStyle(AirNoteDesign.accent)
            }
        }
        .padding(.horizontal, 4)
    }
}
