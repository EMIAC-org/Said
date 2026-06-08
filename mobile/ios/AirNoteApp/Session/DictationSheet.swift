import AirNoteShared
import SwiftUI
import UIKit

/// The live dictation experience — a hero mic, real-time transcript + polished
/// preview, and a result you can copy or share. Reused by the Dashboard and by
/// the onboarding "first dictation" step.
struct DictationSheet: View {
    @StateObject private var controller: DictationController
    @Environment(\.dismiss) private var dismiss
    private let env: AppEnvironment
    private var onComplete: ((DictationResult) -> Void)?
    private var showsDoneButton: Bool

    init(env: AppEnvironment, showsDoneButton: Bool = true, onComplete: ((DictationResult) -> Void)? = nil) {
        self.env = env
        self.onComplete = onComplete
        self.showsDoneButton = showsDoneButton
        _controller = StateObject(wrappedValue: DictationController(env: env))
    }

    var body: some View {
        ZStack {
            AirNoteBackground(tint: controller.isRecording ? AirNoteDesign.danger : AirNoteDesign.accent)
            ScrollView {
                VStack(spacing: 22) {
                    hero
                    if controller.phase == .completed, let result = controller.result {
                        resultCard(result)
                    } else if !controller.interim.isEmpty || !controller.polishPreview.isEmpty {
                        livePreview
                    }
                    if let message = controller.errorMessage {
                        recoveryCard(message)
                    }
                }
                .padding(20)
            }
        }
        .navigationTitle("Dictate")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Close") { Task { await controller.cancel(); dismiss() } }
            }
        }
        .animation(.easeInOut(duration: 0.26), value: controller.phase)
    }

    // MARK: Hero

    private var hero: some View {
        VStack(spacing: 16) {
            Text(headline)
                .font(.system(.title, design: .rounded).weight(.bold))
                .multilineTextAlignment(.center)
                .foregroundStyle(AirNoteDesign.foreground)

            Text(subhead)
                .font(.subheadline)
                .foregroundStyle(AirNoteDesign.muted)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 6)

            MicOrb(
                isRecording: controller.isRecording,
                level: CGFloat(controller.level),
                action: { Task { await controller.toggle() } }
            )
            .padding(.top, 4)
            .disabled(controller.phase == .processing || controller.phase == .preparing)

            AirNoteWaveform(
                level: CGFloat(controller.level),
                active: controller.isRecording || controller.phase == .processing,
                color: controller.isRecording ? AirNoteDesign.danger : AirNoteDesign.accent
            )
            .padding(.horizontal, 20)

            Text(caption)
                .font(.footnote.weight(.medium))
                .foregroundStyle(AirNoteDesign.muted)

            if controller.isRecording {
                Button("Cancel") { Task { await controller.cancel() } }
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.danger)
            }
        }
        .padding(.vertical, 26)
        .frame(maxWidth: .infinity)
        .background(
            RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                .fill(AirNoteDesign.surface.opacity(0.55))
        )
    }

    private var livePreview: some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 12) {
                if !controller.interim.isEmpty {
                    previewRow(title: "Heard", text: controller.interim, tint: AirNoteDesign.muted)
                }
                if !controller.polishPreview.isEmpty {
                    previewRow(title: "Polished", text: controller.polishPreview, tint: AirNoteDesign.foreground)
                }
            }
        }
        .transition(.opacity.combined(with: .move(edge: .bottom)))
    }

    private func resultCard(_ result: DictationResult) -> some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    AirNoteStatusPill(systemImage: "checkmark.circle.fill", text: "Polished", color: AirNoteDesign.success)
                    Spacer()
                    if result.latencyMS > 0 {
                        Text("\(result.latencyMS) ms")
                            .font(.caption.monospacedDigit().weight(.semibold))
                            .foregroundStyle(AirNoteDesign.muted)
                    }
                }
                Text(result.displayText)
                    .font(.body)
                    .foregroundStyle(AirNoteDesign.foreground)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)

                Text("Copied to your clipboard.")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)

                HStack(spacing: 10) {
                    Button {
                        UIPasteboard.general.string = result.displayText
                    } label: {
                        Label("Copy", systemImage: "doc.on.doc")
                    }
                    .buttonStyle(AirNoteGhostButtonStyle())

                    ShareLink(item: result.displayText) {
                        Label("Share", systemImage: "square.and.arrow.up")
                            .font(.system(.subheadline).weight(.semibold))
                            .foregroundStyle(AirNoteDesign.foreground)
                            .frame(maxWidth: .infinity)
                            .frame(height: 44)
                            .background(AirNoteDesign.surfaceRaised.opacity(0.52), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(AirNoteDesign.borderStrong, lineWidth: 1))
                    }
                }

                Button {
                    controller.reset()
                } label: {
                    Label("Dictate again", systemImage: "mic.fill")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())

                if showsDoneButton, let onComplete {
                    Button("Done") { onComplete(result) }
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.accent)
                        .frame(maxWidth: .infinity)
                }
            }
        }
        .transition(.opacity.combined(with: .move(edge: .bottom)))
        .onAppear {
            if let onComplete, !showsDoneButton { onComplete(result) }
        }
    }

    private func recoveryCard(_ message: String) -> some View {
        let isMicDenied = controller.phase == .micDenied
        return AirNoteCard {
            VStack(alignment: .leading, spacing: 10) {
                Label(controller.phase == .unavailable ? "Almost ready" : "Let's recover that",
                      systemImage: controller.phase == .unavailable ? "clock.badge.checkmark" : "exclamationmark.triangle.fill")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(controller.phase == .unavailable ? AirNoteDesign.accent : AirNoteDesign.warning)
                Text(message)
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
                if isMicDenied {
                    Button {
                        env.permissions.openSettings()
                    } label: {
                        Label("Open Settings", systemImage: "gearshape")
                    }
                    .buttonStyle(AirNoteGhostButtonStyle())
                }
            }
        }
    }

    private func previewRow(title: String, text: String, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            AirNoteSectionLabel(text: title)
            Text(text)
                .font(.body)
                .foregroundStyle(tint)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: Copy

    private var headline: String {
        switch controller.phase {
        case .recording: return "Listening"
        case .processing: return "Polishing your words"
        case .completed: return "Here's your text"
        case .preparing: return "Getting ready"
        case .micDenied: return "Microphone is off"
        case .unavailable: return "Almost ready"
        case .failed: return "Let's try that again"
        case .idle: return "Tap to dictate"
        }
    }

    private var subhead: String {
        switch controller.phase {
        case .recording: return "Speak naturally in English, Hindi, or Hinglish. Tap the mic to stop."
        case .processing: return "Transcribing and polishing — only the clean text comes back."
        case .completed: return "Copy it, share it, or dictate again."
        case .unavailable: return "Dictation turns on automatically once your workspace finishes setup."
        case .micDenied: return "AirNote needs the microphone to hear you."
        default: return "Hold a thought, say it out loud, and AirNote writes it cleanly."
        }
    }

    private var caption: String {
        switch controller.phase {
        case .recording: return "Recording · \(env.outputLanguage.capitalized)"
        case .processing: return "Polishing…"
        case .preparing: return "Connecting…"
        default: return "Tap the mic to start"
        }
    }
}
