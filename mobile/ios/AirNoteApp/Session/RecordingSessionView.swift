import SwiftUI
import UIKit
import AirNoteShared

struct RecordingSessionView: View {
    @StateObject private var controller = SessionController()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 14) {
                    HStack {
                        AirNoteStatusPill(systemImage: statusIcon, text: statusText, color: statusColor)
                        Spacer()
                        Text(latencyText)
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }

                    Text(headline)
                        .font(.title2.weight(.semibold))

                    Text(detailCopy)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    LevelPreview(level: CGFloat(controller.level), reduceMotion: reduceMotion)

                    if !controller.interimTranscript.isEmpty || !controller.polishPreview.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            if !controller.interimTranscript.isEmpty {
                                PreviewRow(title: "Transcript", text: controller.interimTranscript)
                            }
                            if !controller.polishPreview.isEmpty {
                                PreviewRow(title: "Polish preview", text: controller.polishPreview)
                            }
                        }
                    }

                    AirNoteActionRow(
                        primaryTitle: primaryTitle,
                        primarySystemImage: primaryIcon,
                        secondaryTitle: "Cancel",
                        secondarySystemImage: "xmark.circle",
                        primaryAction: { Task { await primaryAction() } },
                        secondaryAction: { Task { await controller.cancelRecording() } }
                    )
                }
                .padding(14)
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))

                VStack(alignment: .leading, spacing: 10) {
                    Text("Session health")
                        .font(.headline)
                    HealthRow(systemImage: "mic.fill", title: "Microphone", value: micHealth)
                    HealthRow(systemImage: "keyboard", title: "Keyboard bridge", value: "Watching commands")
                    HealthRow(systemImage: "network", title: "Gateway", value: BuildConfig.useMockGateway ? "Mock" : BuildConfig.gatewayBaseURL.host ?? "Configured")
                    HealthRow(systemImage: "lock.shield", title: "Privacy", value: "Final only inserts")
                }
                .padding(14)
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
            }
            .padding(16)
        }
        .navigationTitle("AirNote Session")
        .background(Color(.systemGroupedBackground))
        .onAppear {
            controller.startCommandWatcher()
        }
        .onDisappear {
            controller.stopCommandWatcher()
        }
    }

    private var headline: String {
        switch controller.state {
        case .recording: return "Speak naturally"
        case .processing: return "AirNote is polishing"
        case .insertReady: return "Final text is ready"
        case .retryableError: return "Recovery is available"
        case .ready: return "Swipe back to your app"
        default: return "Start an AirNote Session"
        }
    }

    private var detailCopy: String {
        switch controller.state {
        case .recording:
            return "Audio is streaming to the independent AirNote Mobile Gateway. Stop when you are done speaking."
        case .processing:
            return "The server is transcribing, polishing, and applying the Hinglish guard. Only the final text can be inserted."
        case .insertReady:
            return "Return to the keyboard to insert, copy, or save the polished final."
        case .retryableError(let message):
            return message
        default:
            return "AirNote Keyboard will use this visible session to record after you tap the mic. If iOS pauses this app, the keyboard asks you to restart instead of hanging."
        }
    }

    private var statusIcon: String {
        switch controller.state {
        case .recording: return "mic.fill"
        case .processing: return "bolt.horizontal.fill"
        case .insertReady, .inserted: return "checkmark.circle.fill"
        case .retryableError, .stale: return "exclamationmark.triangle.fill"
        default: return "waveform"
        }
    }

    private var statusText: String {
        switch controller.state {
        case .recording: return "Listening"
        case .processing: return "Processing"
        case .insertReady: return "Insert ready"
        case .inserted: return "Inserted"
        case .retryableError: return "Retry"
        case .stale: return "Stale"
        case .ready: return "Session ready"
        default: return "Session setup"
        }
    }

    private var statusColor: Color {
        switch controller.state {
        case .recording: return AirNoteDesign.danger
        case .processing, .ready: return AirNoteDesign.accent
        case .insertReady, .inserted: return AirNoteDesign.success
        case .retryableError, .stale: return AirNoteDesign.warning
        default: return AirNoteDesign.teal
        }
    }

    private var latencyText: String {
        if let latency = controller.lastLatencyMS {
            return "\(latency) ms"
        }
        return BuildConfig.useMockGateway ? "Mock" : "Live"
    }

    private var micHealth: String {
        switch controller.state {
        case .recording: return "Recording"
        case .processing: return "Stopped"
        default: return "Ready"
        }
    }

    private var primaryTitle: String {
        switch controller.state {
        case .recording: return "Stop"
        case .processing: return "Working"
        case .ready, .insertReady, .inserted: return "Start recording"
        default: return "Start session"
        }
    }

    private var primaryIcon: String {
        switch controller.state {
        case .recording: return "stop.fill"
        case .processing: return "hourglass"
        case .ready, .insertReady, .inserted: return "mic.fill"
        default: return "play.fill"
        }
    }

    private func primaryAction() async {
        switch controller.state {
        case .recording:
            await controller.stopRecording()
        case .processing:
            return
        case .ready, .insertReady, .inserted:
            let command = BridgeCommand(
                kind: .startRecording,
                commandSeq: UInt64(Date().timeIntervalSince1970 * 1000),
                keyboardContext: sampleContext,
                languageHint: .hinglish,
                style: .work,
                clientRequestID: RequestId.make()
            )
            try? AppGroupBridge().write(command, to: .command)
        default:
            await controller.startSession(
                deviceID: deviceID,
                context: sampleContext,
                languageHint: .hinglish,
                style: .work
            )
        }
    }

    private var sampleContext: KeyboardContext {
        KeyboardContext(beforeText: "", afterText: "", selectedText: "", hostAppLabel: "AirNote", fieldHint: "practice")
    }

    private var deviceID: String {
        UIDevice.current.identifierForVendor?.uuidString ?? "ios-\(UUID().uuidString)"
    }
}

private struct LevelPreview: View {
    var level: CGFloat
    var reduceMotion: Bool
    private let levels: [CGFloat] = [0.25, 0.58, 0.86, 0.48, 0.72, 0.34, 0.62]

    var body: some View {
        HStack(alignment: .center, spacing: 7) {
            ForEach(levels.indices, id: \.self) { index in
                Capsule()
                    .fill(level > 0.05 ? AirNoteDesign.danger : (index == 2 ? AirNoteDesign.accent : AirNoteDesign.teal.opacity(0.55)))
                    .frame(width: 8, height: 52 * displayLevel(index))
                    .accessibilityHidden(true)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 58)
        .padding(.vertical, 8)
        .background(Color(.tertiarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
        .accessibilityLabel("Microphone level preview")
    }

    private func displayLevel(_ index: Int) -> CGFloat {
        if reduceMotion || level <= 0.05 {
            return levels[index]
        }
        return min(1, max(0.16, (levels[index] + level) / 2))
    }
}

private struct PreviewRow: View {
    var title: String
    var text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            Text(text)
                .font(.subheadline)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.tertiarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

private struct HealthRow: View {
    var systemImage: String
    var title: String
    var value: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .foregroundStyle(AirNoteDesign.accent)
                .frame(width: 22)
            Text(title)
                .font(.subheadline)
            Spacer()
            Text(value)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }
}
