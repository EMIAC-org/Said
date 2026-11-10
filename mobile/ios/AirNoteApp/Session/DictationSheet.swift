import AirNoteShared
import SwiftUI
import UIKit

/// The live dictation experience: a hero mic, a stage tracker that shows what
/// runs on the iPhone vs the cloud (with timings), and a result you can compare,
/// copy or share. Reused by Home, onboarding, and the keyboard handoff.
struct DictationSheet: View {
    @StateObject private var controller: DictationController
    @ObservedObject private var model = OnDeviceModelStatus.shared
    @Environment(\.dismiss) private var dismiss
    @State private var didAutoStart = false
    @State private var showHeard = false
    @State private var copied = false
    private let env: AppEnvironment
    private var onComplete: ((DictationResult) -> Void)?
    private var showsDoneButton: Bool
    /// True when launched by the keyboard (airnote://dictate): auto-starts and
    /// tells the user to swipe back so the keyboard can insert the result.
    private var handoffMode: Bool

    init(env: AppEnvironment, showsDoneButton: Bool = true, handoffMode: Bool = false, onComplete: ((DictationResult) -> Void)? = nil) {
        self.env = env
        self.onComplete = onComplete
        self.showsDoneButton = showsDoneButton
        self.handoffMode = handoffMode
        _controller = StateObject(wrappedValue: DictationController(env: env))
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 18) {
                    hero
                    if controller.stage >= .transcribing || controller.phase == .completed {
                        trackerCard
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                    }
                    if controller.phase == .completed, let result = controller.result {
                        resultCard(result)
                    }
                    if let message = controller.errorMessage {
                        recoveryCard(message)
                    }
                    privacyFootnote
                }
                .padding(.horizontal, 18)
                .padding(.top, 6)
                .padding(.bottom, 28)
            }
        }
        .navigationTitle("Dictate")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Close") { Task { await controller.cancel(); dismiss() } }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Text(env.outputLanguage == "english" ? "English" : "Hinglish")
                    .font(.system(.caption, design: .rounded).weight(.semibold))
                    .foregroundStyle(AirNoteDesign.muted)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(AirNoteDesign.surfaceRaised, in: Capsule())
            }
        }
        .animation(.spring(response: 0.4, dampingFraction: 0.85), value: controller.phase)
        .animation(.spring(response: 0.4, dampingFraction: 0.85), value: controller.stage)
        .onDisappear { Task { await controller.cancel() } }
        .task {
            // Keyboard handoff: the app is foreground, so start recording right away.
            if handoffMode, !didAutoStart {
                didAutoStart = true
                await controller.start()
            }
            #if DEBUG
            if AppEnvironment.demoRoute == "dictation", !didAutoStart {
                didAutoStart = true
                await controller.start()
            }
            #endif
        }
    }

    // MARK: Hero

    private var hero: some View {
        VStack(spacing: 18) {
            VStack(spacing: 6) {
                Text(headline)
                    .font(.system(.title2, design: .rounded).weight(.bold))
                    .foregroundStyle(AirNoteDesign.foreground)
                    .contentTransition(.opacity)
                Text(subhead)
                    .font(.subheadline)
                    .foregroundStyle(AirNoteDesign.muted)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 10)
            }

            HeroMicButton(
                isRecording: controller.isRecording,
                isBusy: controller.phase == .processing || controller.phase == .preparing,
                level: CGFloat(controller.level),
                size: controller.phase == .completed ? 76 : 128,
                action: { Task { await controller.toggle() } }
            )
            .disabled(controller.phase == .processing || controller.phase == .preparing)
            .padding(.vertical, controller.phase == .completed ? 4 : 18)

            if controller.isRecording {
                VStack(spacing: 10) {
                    recordingTimer
                    AirNoteWaveform(level: CGFloat(controller.level), active: true, barCount: 15, color: AirNoteDesign.danger)
                        .frame(height: 40)
                        .padding(.horizontal, 30)
                    Button("Cancel") {
                        Haptics.tap()
                        Task { await controller.cancel() }
                    }
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.muted)
                }
                .transition(.opacity)
            } else if controller.phase == .idle || controller.phase == .failed {
                HStack(spacing: 6) {
                    PlaceBadge(place: .onDevice, text: "Speech to text on iPhone")
                    PlaceBadge(place: .cloud, text: "Polish in cloud")
                }
                .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 10)
    }

    private var recordingTimer: some View {
        TimelineView(.periodic(from: .now, by: 0.1)) { context in
            let elapsed = controller.recordingStartedAtPublic.map { context.date.timeIntervalSince($0) } ?? 0
            HStack(spacing: 8) {
                StatusDot(color: AirNoteDesign.danger, pulsing: true)
                Text(String(format: "%d:%02d", Int(elapsed) / 60, Int(elapsed) % 60))
                    .font(.system(.title3, design: .rounded).weight(.semibold).monospacedDigit())
                    .foregroundStyle(AirNoteDesign.foreground)
            }
        }
    }

    // MARK: Stage tracker

    private var trackerCard: some View {
        AirNoteCard(padding: 16) {
            StageTracker(
                stage: controller.stage,
                audioSeconds: controller.audioSeconds,
                sttMS: controller.sttMS,
                polishMS: controller.polishMS,
                polishSkipped: controller.polishSkipped
            )
        }
    }

    // MARK: Result

    private func resultCard(_ result: DictationResult) -> some View {
        AirNoteCard(padding: 18) {
            VStack(alignment: .leading, spacing: 16) {
                if handoffMode {
                    HStack(spacing: 10) {
                        Image(systemName: "arrow.uturn.backward.circle.fill")
                            .font(.title3)
                            .foregroundStyle(AirNoteDesign.accent)
                        Text("Swipe back to your app — AirNote will paste this at your cursor.")
                            .font(.footnote.weight(.medium))
                            .foregroundStyle(AirNoteDesign.foreground)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(AirNoteDesign.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                }

                Picker("Version", selection: $showHeard) {
                    Text(result.polishSkipped ? "Your words" : "Polished").tag(false)
                    Text("Heard on iPhone").tag(true)
                }
                .pickerStyle(.segmented)

                Text(showHeard ? HinglishScript.enforceRomanHinglish(result.transcript) : result.displayText)
                    .font(.system(size: 19, weight: .regular))
                    .lineSpacing(3)
                    .foregroundStyle(AirNoteDesign.foreground)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .contentTransition(.opacity)
                    .animation(.easeInOut(duration: 0.2), value: showHeard)
                    .frame(maxWidth: .infinity, alignment: .leading)

                HStack(spacing: 6) {
                    Image(systemName: copied ? "checkmark.circle.fill" : "doc.on.clipboard")
                        .contentTransition(.symbolEffect(.replace))
                    Text(copied ? "Copied" : "Already on your clipboard")
                }
                .font(.caption.weight(.medium))
                .foregroundStyle(copied ? AirNoteDesign.success : AirNoteDesign.muted)

                HStack(spacing: 10) {
                    Button {
                        UIPasteboard.general.string = showHeard ? result.transcript : result.displayText
                        Haptics.success()
                        withAnimation { copied = true }
                    } label: {
                        Label("Copy", systemImage: "doc.on.doc")
                    }
                    .buttonStyle(AirNoteGhostButtonStyle())

                    ShareLink(item: result.displayText) {
                        Label("Share", systemImage: "square.and.arrow.up")
                            .font(.system(.body, design: .rounded).weight(.semibold))
                            .foregroundStyle(AirNoteDesign.foreground)
                            .frame(maxWidth: .infinity)
                            .frame(height: 50)
                            .background(AirNoteDesign.surfaceRaised.opacity(0.7), in: RoundedRectangle(cornerRadius: AirNoteDesign.buttonRadius, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: AirNoteDesign.buttonRadius, style: .continuous).strokeBorder(AirNoteDesign.border, lineWidth: 1))
                    }
                }

                Button {
                    Haptics.tap()
                    copied = false
                    showHeard = false
                    controller.reset()
                } label: {
                    Label("Dictate again", systemImage: "mic.fill")
                }
                .buttonStyle(AirNotePrimaryButtonStyle())

                if showsDoneButton, let onComplete {
                    Button("Done") { onComplete(result) }
                        .font(.system(.body, design: .rounded).weight(.semibold))
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
        return AirNoteCard(padding: 18) {
            HStack(alignment: .top, spacing: 12) {
                IconTile(systemImage: isMicDenied ? "mic.slash.fill" : "exclamationmark.triangle.fill", tint: AirNoteDesign.warning, size: 38)
                VStack(alignment: .leading, spacing: 8) {
                    Text(isMicDenied ? "Microphone is off" : "Let's try that again")
                        .font(.system(.subheadline, design: .rounded).weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
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
    }

    private var privacyFootnote: some View {
        HStack(spacing: 6) {
            Image(systemName: "lock.fill")
            Text("Your audio stays on this iPhone. Only text is sent for polish.")
        }
        .font(.caption2.weight(.medium))
        .foregroundStyle(AirNoteDesign.muted)
        .multilineTextAlignment(.center)
        .padding(.top, 4)
    }

    // MARK: Copy

    private var headline: String {
        switch controller.phase {
        case .recording: return "Listening…"
        case .processing:
            return controller.stage == .polishing ? "Polishing" : "Transcribing on iPhone"
        case .completed: return "Done"
        case .preparing: return "Getting ready"
        case .micDenied: return "Microphone is off"
        case .unavailable: return "Almost ready"
        case .failed: return "Let's try that again"
        case .idle: return "Tap to dictate"
        }
    }

    private var subhead: String {
        switch controller.phase {
        case .recording: return "Speak naturally. Pause for a few seconds or tap to finish."
        case .processing:
            return controller.stage == .polishing
                ? "Adding punctuation, your tone and your words."
                : "\(OnDeviceModelStatus.modelName) is turning your voice into text."
        case .completed:
            if let result = controller.result, result.polishSkipped {
                return "Polish wasn't reachable, so here are your exact words."
            }
            return "Copied to your clipboard and saved to History."
        case .unavailable: return "AirNote couldn't reach its polish service."
        case .micDenied: return "AirNote needs the microphone to hear you."
        default: return "English, Hindi or Hinglish — say it the way you'd say it."
        }
    }
}
