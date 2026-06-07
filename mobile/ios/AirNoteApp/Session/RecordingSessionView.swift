import SwiftUI
import UIKit
import AirNoteShared

struct RecordingSessionView: View {
    @StateObject private var controller = SessionController()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            AirNoteBackground(tint: isRecording ? AirNoteDesign.danger : AirNoteDesign.accent)

            ScrollView {
                VStack(spacing: 22) {
                    header

                    // ── Hero voice moment ──────────────────────────────
                    VStack(spacing: 16) {
                        Text(headline)
                            .font(.system(.title, design: .rounded).weight(.bold))
                            .multilineTextAlignment(.center)
                            .frame(maxWidth: .infinity)

                        Text(detailCopy)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.horizontal, 8)

                        MicOrb(
                            isRecording: isRecording,
                            level: CGFloat(controller.level),
                            action: { Task { await primaryAction() } }
                        )
                        .padding(.top, 6)

                        AirNoteWaveform(
                            level: CGFloat(controller.level),
                            active: isRecording || isProcessing,
                            color: isRecording ? AirNoteDesign.danger : AirNoteDesign.accent
                        )
                        .padding(.horizontal, 20)

                        Text(subcaption)
                            .font(.footnote.weight(.medium))
                            .foregroundStyle(.secondary)

                        if showsCancel {
                            Button("Cancel") { Task { await controller.cancelRecording() } }
                                .font(.subheadline.weight(.semibold))
                                .foregroundStyle(AirNoteDesign.danger)
                                .padding(.top, 2)
                        }
                    }
                    .padding(.vertical, 28)
                    .frame(maxWidth: .infinity)
                    .background(
                        RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                            .fill(Color(.secondarySystemBackground).opacity(0.55))
                    )

                    // ── Live transcript / polish preview ───────────────
                    if !controller.interimTranscript.isEmpty || !controller.polishPreview.isEmpty {
                        AirNoteCard {
                            VStack(alignment: .leading, spacing: 12) {
                                if !controller.interimTranscript.isEmpty {
                                    PreviewRow(title: "Heard", text: controller.interimTranscript, tint: .secondary)
                                }
                                if !controller.polishPreview.isEmpty {
                                    PreviewRow(title: "Polished", text: controller.polishPreview, tint: .primary)
                                }
                            }
                        }
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                    }

                    // ── Session health ─────────────────────────────────
                    AirNoteCard {
                        VStack(alignment: .leading, spacing: 14) {
                            AirNoteSectionLabel(text: "Session health")
                            HealthRow(systemImage: "mic.fill", title: "Microphone", value: micHealth)
                            HealthRow(systemImage: "keyboard", title: "Keyboard bridge", value: "Watching commands")
                            HealthRow(systemImage: "network", title: "Gateway",
                                      value: BuildConfig.useMockGateway ? "Mock" : (BuildConfig.gatewayBaseURL.host ?? "Configured"))
                            HealthRow(systemImage: "lock.shield", title: "Privacy", value: "Final text only")
                        }
                    }
                }
                .padding(18)
            }
        }
        .navigationTitle("AirNote Session")
        .navigationBarTitleDisplayMode(.inline)
        .animation(.easeInOut(duration: 0.28), value: controller.state)
        .onAppear { controller.startCommandWatcher() }
        .onDisappear { controller.stopCommandWatcher() }
    }

    // MARK: header

    private var header: some View {
        HStack {
            AirNoteStatusPill(systemImage: statusIcon, text: statusText, color: statusColor, animated: isRecording)
            Spacer()
            Text(latencyText)
                .font(.caption.monospacedDigit().weight(.semibold))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Color(.tertiarySystemBackground), in: Capsule())
        }
    }

    // MARK: derived state

    private var isRecording: Bool {
        if case .recording = controller.state { return true }
        return false
    }
    private var isProcessing: Bool { controller.state == .processing }
    private var showsCancel: Bool {
        switch controller.state {
        case .recording, .processing, .insertReady: return true
        default: return false
        }
    }

    private var subcaption: String {
        if isProcessing { return "AirNote Gateway · polishing" }
        if isRecording { return "AirNote Gateway · Hinglish" }
        return "Tap to start · insert first, learn later"
    }

    private var headline: String {
        switch controller.state {
        case .recording: return "Listening"
        case .processing: return "Polishing your words"
        case .insertReady: return "Ready to insert"
        case .retryableError: return "Let's recover that"
        case .ready: return "Swipe back to your app"
        default: return "Start an AirNote Session"
        }
    }

    private var detailCopy: String {
        switch controller.state {
        case .recording:
            return "Speak naturally in English, Hindi, or Hinglish. Tap to stop when you're done."
        case .processing:
            return "Transcribing, polishing, and applying the Hinglish guard. Only the final text inserts."
        case .insertReady:
            return "Return to the keyboard to insert, copy, or save the polished result."
        case .retryableError(let message):
            return message
        case .ready:
            return "AirNote Keyboard will record through this session. Tap the mic to begin."
        default:
            return "Start a visible session, switch back to any app, then dictate with AirNote Keyboard."
        }
    }

    private var statusIcon: String {
        switch controller.state {
        case .recording: return "waveform"
        case .processing: return "sparkles"
        case .insertReady, .inserted: return "checkmark.circle.fill"
        case .retryableError, .stale: return "exclamationmark.triangle.fill"
        default: return "mic"
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
        case .ready: return "Ready"
        default: return "Setup"
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
        if let latency = controller.lastLatencyMS { return "\(latency) ms" }
        return BuildConfig.useMockGateway ? "Mock" : "Live"
    }

    private var micHealth: String {
        switch controller.state {
        case .recording: return "Recording"
        case .processing: return "Stopped"
        default: return "Ready"
        }
    }

    // MARK: actions

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

private struct PreviewRow: View {
    var title: String
    var text: String
    var tint: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            AirNoteSectionLabel(text: title)
            Text(text)
                .font(.body)
                .foregroundStyle(tint)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

private struct HealthRow: View {
    var systemImage: String
    var title: String
    var value: String

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .foregroundStyle(AirNoteDesign.accent)
                .frame(width: 24)
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
