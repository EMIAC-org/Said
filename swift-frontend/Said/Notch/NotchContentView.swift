import SwiftUI
import os

let openNotchSize = CGSize(width: 640, height: 190)
let windowSize = CGSize(width: openNotchSize.width, height: openNotchSize.height + shadowPadding)

private let notchLogger = Logger(subsystem: "com.emiac.said", category: "notch-view")

struct NotchContentView: View {
    @ObservedObject var vm: NotchViewModel
    var onManualRecord: (() -> Void)?
    var onPolishShortcut: ((UInt8) -> Void)?
    var onRepaste: (() -> Void)?
    var onSetLanguage: ((String) -> Void)?
    var onOpenSettings: (() -> Void)?
    @State private var isHovering = false
    @State private var hoverTask: Task<Void, Never>?
    @State private var haptics = false

    private let animationSpring = Animation.interactiveSpring(
        response: 0.38, dampingFraction: 0.8, blendDuration: 0
    )

    private var topRadius: CGFloat {
        vm.notchState == .open ? notchCornerRadius.opened.top : notchCornerRadius.closed.top
    }

    private var bottomRadius: CGFloat {
        vm.notchState == .open ? notchCornerRadius.opened.bottom : notchCornerRadius.closed.bottom
    }

    private var lifecyclePhase: Int {
        switch vm.dictationState {
        case .idle: 0
        case .recording: 1
        case .processing: 2
        case .done: 3
        case .error: 4
        }
    }

    private var compactHeight: CGFloat {
        guard vm.isActiveLifecycle else { return vm.effectiveClosedNotchHeight }
        switch vm.dictationState {
        case .recording: return vm.effectiveClosedNotchHeight + 38
        case .processing: return vm.effectiveClosedNotchHeight + 32
        case .done:       return vm.effectiveClosedNotchHeight + 28
        case .error:      return vm.effectiveClosedNotchHeight + 32
        case .idle:       return vm.effectiveClosedNotchHeight
        }
    }

    var body: some View {
        ZStack(alignment: .top) {
            VStack(spacing: 0) {
                notchContent()
                    .frame(alignment: .top)
                    .padding(
                        .horizontal,
                        vm.notchState == .open ? bottomRadius : notchCornerRadius.closed.bottom
                    )
                    .padding([.horizontal, .bottom], vm.notchState == .open ? 12 : 0)
                    .background(.black)
                    .clipShape(
                        NotchShape(topCornerRadius: topRadius, bottomCornerRadius: bottomRadius)
                    )
                    .overlay(alignment: .top) {
                        Rectangle()
                            .fill(.black)
                            .frame(height: 1)
                            .padding(.horizontal, topRadius)
                    }
                    .shadow(
                        color: (vm.notchState == .open || isHovering || vm.isActiveLifecycle)
                            ? .black.opacity(0.7) : .clear,
                        radius: 6
                    )
                    .frame(height: vm.notchState == .open ? vm.notchSize.height : nil)
            }
            .animation(
                vm.notchState == .open
                    ? .spring(response: 0.42, dampingFraction: 0.8)
                    : .spring(response: 0.45, dampingFraction: 1.0),
                value: vm.notchState
            )
            .animation(animationSpring, value: lifecyclePhase)
            .contentShape(Rectangle())
            .onHover { handleHover($0) }
            .onTapGesture { doOpen() }
        }
        .padding(.bottom, 8)
        .frame(maxWidth: windowSize.width, maxHeight: windowSize.height, alignment: .top)
        .compositingGroup()
        .sensoryFeedback(.alignment, trigger: haptics)
        .preferredColorScheme(.dark)
    }

    // MARK: - Notch Content

    @ViewBuilder
    private func notchContent() -> some View {
        if vm.notchState == .open {
            openLayout
                .transition(
                    .scale(scale: 0.8, anchor: .top)
                    .combined(with: .opacity)
                    .animation(.smooth(duration: 0.35))
                )
        } else {
            compactLayout
        }
    }

    private var compactLayout: some View {
        ZStack(alignment: .bottom) {
            Color.black

            if vm.isActiveLifecycle {
                compactOverlay
                    .padding(.bottom, 6)
                    .padding(.horizontal, 8)
            }
        }
        .frame(width: vm.closedNotchSize.width - 20, height: compactHeight)
    }

    @ViewBuilder
    private var compactOverlay: some View {
        switch vm.dictationState {
        case .recording:
            LifecycleAudioBarsView(level: vm.audioLevel)
                .transition(.opacity)
        case .processing:
            VStack(spacing: 3) {
                ProcessingDotsView()
                Text(processingLabel(vm.processingPhase))
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.5))
                    .lineLimit(1)
            }
            .transition(.opacity)
        case .done:
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(Color(red: 0.19, green: 0.82, blue: 0.35))
                    .shadow(
                        color: Color(red: 0.19, green: 0.82, blue: 0.35).opacity(0.55),
                        radius: 8
                    )
                Text("Pasted")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.8))
                SuccessBarsView()
            }
            .transition(.opacity)
        case .error(let msg):
            HStack(spacing: 5) {
                ErrorPulseView()
                Text(msg.prefix(30))
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.white.opacity(0.7))
                    .lineLimit(1)
            }
            .transition(.opacity)
        case .idle:
            EmptyView()
        }
    }

    // MARK: - Open Layout

    private var openLayout: some View {
        VStack(spacing: 0) {
            notchHeader
                .padding(.horizontal, 16)
                .padding(.bottom, 8)

            // Row 2: Two-column content
            HStack(alignment: .top, spacing: 10) {
                // Left: Last Result
                lastResultPanel
                // Right: Quick Polish
                quickPolishPanel
            }
            .padding(.horizontal, 14)

            Spacer(minLength: 6)

            // Row 3: Status bar
            statusBar
                .padding(.horizontal, 14)
                .padding(.bottom, 10)
        }
    }

    // MARK: - Left Panel: Last Result

    private var lastResultPanel: some View {
        VStack(alignment: .leading, spacing: 6) {
            if vm.lastResult.isEmpty {
                VStack(spacing: 4) {
                    Image(systemName: "waveform")
                        .font(.system(size: 18))
                        .foregroundStyle(.white.opacity(0.2))
                    Text("Hold \(vm.activeHotkeyLabel) and speak")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.white.opacity(0.35))
                    Text("Polished text pastes automatically")
                        .font(.system(size: 9))
                        .foregroundStyle(.white.opacity(0.2))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Text(vm.lastResult)
                    .font(.system(size: 11, weight: .regular))
                    .foregroundStyle(.white.opacity(0.75))
                    .lineLimit(4)
                    .frame(maxWidth: .infinity, alignment: .topLeading)

                Spacer(minLength: 0)

                HStack {
                    Spacer()
                    Button {
                        onRepaste?()
                    } label: {
                        HStack(spacing: 3) {
                            Image(systemName: "doc.on.clipboard")
                                .font(.system(size: 8))
                            Text("Re-paste")
                                .font(.system(size: 9, weight: .medium))
                        }
                        .foregroundStyle(.white.opacity(0.35))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 3)
                        .background(.white.opacity(0.06))
                        .clipShape(Capsule())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.white.opacity(0.04))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }

    // MARK: - Right Panel: Quick Polish

    private var quickPolishPanel: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("QUICK POLISH")
                .font(.system(size: 8, weight: .bold))
                .foregroundStyle(.white.opacity(0.3))
                .tracking(0.8)

            Text("Select text, then:")
                .font(.system(size: 9))
                .foregroundStyle(.white.opacity(0.25))

            Spacer(minLength: 2)

            polishButton("Format Fix", shortcut: "1", action: { onPolishShortcut?(1) })
            polishButton("Professional", shortcut: "2", action: { onPolishShortcut?(2) })
            polishButton("Casual", shortcut: "3", action: { onPolishShortcut?(3) })
            polishButton("Concise", shortcut: "4", action: { onPolishShortcut?(4) })
            polishButton("Hinglish", shortcut: "5", action: { onPolishShortcut?(5) })
        }
        .padding(10)
        .frame(minWidth: 170, maxWidth: 170, maxHeight: .infinity)
        .background(.white.opacity(0.04))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }

    private func polishButton(_ label: String, shortcut: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(label)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.white.opacity(0.7))
                Spacer()
                Text("⌥\(shortcut)")
                    .font(.system(size: 9, weight: .medium).monospaced())
                    .foregroundStyle(.white.opacity(0.25))
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.white.opacity(0.04))
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    // MARK: - Notch Header

    private var notchHeader: some View {
        HStack(alignment: .bottom, spacing: 0) {
            Text("Said")
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(.white)
                .padding(.bottom, 2)
                .frame(maxWidth: .infinity, alignment: .leading)

            Rectangle()
                .fill(vm.metrics.hasNotch ? .black : .clear)
                .frame(width: vm.closedNotchSize.width)
                .clipShape(NotchShape())

            Button {
                withAnimation(animationSpring) { vm.close() }
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(.white.opacity(0.5))
                    .frame(width: 20, height: 20)
                    .background(.white.opacity(0.08))
                    .clipShape(Circle())
            }
            .buttonStyle(.plain)
            .padding(.bottom, 2)
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .frame(height: max(24, vm.effectiveClosedNotchHeight))
    }

    // MARK: - Status Bar

    private var statusBar: some View {
        HStack(spacing: 10) {
            // Status dot + text
            HStack(spacing: 4) {
                Circle()
                    .fill(vm.backendReady ? .green : .orange)
                    .frame(width: 6, height: 6)
                Text(vm.backendReady ? "Ready" : "Connecting")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.white.opacity(0.4))
            }

            // Hotkey indicator
            HStack(spacing: 3) {
                Image(systemName: "command")
                    .font(.system(size: 7))
                Text(vm.activeHotkeyLabel)
                    .font(.system(size: 9, weight: .medium))
            }
            .foregroundStyle(.white.opacity(0.3))

            Spacer()

            // Language toggle
            Menu {
                ForEach(OutputLanguage.allCases, id: \.self) { lang in
                    Button {
                        onSetLanguage?(lang.rawValue)
                        vm.outputLanguage = lang.rawValue
                    } label: {
                        HStack {
                            Text(lang.label)
                            if vm.outputLanguage == lang.rawValue {
                                Image(systemName: "checkmark")
                            }
                        }
                    }
                }
            } label: {
                HStack(spacing: 3) {
                    Text(currentLanguageLabel)
                        .font(.system(size: 9, weight: .semibold))
                    Image(systemName: "chevron.down")
                        .font(.system(size: 6, weight: .bold))
                }
                .foregroundStyle(.white.opacity(0.4))
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(.white.opacity(0.06))
                .clipShape(Capsule())
            }
            .menuStyle(.borderlessButton)
            .fixedSize()

            // Record button
            Button { onManualRecord?() } label: {
                Image(systemName: vm.isRecording ? "stop.circle.fill" : "mic.circle.fill")
                    .font(.system(size: 14))
                    .foregroundStyle(vm.isRecording ? .red : .white.opacity(0.4))
            }
            .buttonStyle(.plain)

            // Settings
            Button { onOpenSettings?() } label: {
                Image(systemName: "gear")
                    .font(.system(size: 12))
                    .foregroundStyle(.white.opacity(0.35))
            }
            .buttonStyle(.plain)
        }
    }

    private var currentLanguageLabel: String {
        OutputLanguage(rawValue: vm.outputLanguage)?.label ?? "Hinglish"
    }

    // MARK: - Helpers

    private func processingLabel(_ phase: String) -> String {
        let p = phase.lowercased()
        if p.contains("paste") { return "Pasting" }
        if p.contains("format") { return "Formatting" }
        if p.contains("polish") || p.contains("llm") || p.contains("enhanc") { return "Enhancing" }
        if p.contains("record") { return "Recording" }
        return "Transcribing"
    }

    // MARK: - Hover + Open/Close

    private func doOpen() {
        doOpen(performHaptic: true)
    }

    private func doOpen(performHaptic: Bool) {
        guard vm.notchState == .closed, !vm.isActiveLifecycle else { return }
        withAnimation(animationSpring) {
            if performHaptic { haptics.toggle() }
            vm.open()
        }
    }

    private func handleHover(_ hovering: Bool) {
        notchLogger.info("hover: \(hovering) state=\(String(describing: vm.notchState)) lifecycle=\(vm.isActiveLifecycle)")
        hoverTask?.cancel()
        guard !vm.isActiveLifecycle else {
            withAnimation(animationSpring) { isHovering = false }
            return
        }

        if hovering {
            withAnimation(animationSpring) { isHovering = true }
            if vm.notchState == .closed { haptics.toggle() }
            guard vm.notchState == .closed else { return }

            hoverTask = Task {
                try? await Task.sleep(for: .seconds(0.3))
                guard !Task.isCancelled else { return }
                await MainActor.run {
                    guard vm.notchState == .closed, isHovering else { return }
                    doOpen(performHaptic: false)
                }
            }
        } else {
            hoverTask = Task {
                try? await Task.sleep(for: .milliseconds(100))
                guard !Task.isCancelled else { return }
                await MainActor.run {
                    withAnimation(animationSpring) { isHovering = false }
                    if vm.notchState == .open { vm.close() }
                }
            }
        }
    }
}

// MARK: - Audio Bars (recording)

private struct LifecycleAudioBarsView: View {
    var level: Float

    private let levelShape: [CGFloat] = [
        0.32, 0.42, 0.58, 0.76, 0.92,
        1.00, 0.86, 0.70, 0.86, 1.00,
        0.92, 0.76, 0.58, 0.42, 0.32,
    ]

    var body: some View {
        TimelineView(.animation) { timeline in
            let time = timeline.date.timeIntervalSinceReferenceDate
            let clamped = max(0, min(1, CGFloat(level)))

            HStack(spacing: 2) {
                ForEach(levelShape.indices, id: \.self) { index in
                    let flutter = clamped > 0.015
                        ? 0.84 + 0.16 * CGFloat(sin(time * 10.5 + Double(index) * 0.72))
                        : 1
                    let barLevel = clamped * levelShape[index] * flutter
                    Capsule()
                        .fill(.white.opacity(clamped > 0.015 ? 0.56 + clamped * 0.42 : 0.42))
                        .frame(width: 3, height: 4 + barLevel * 24)
                        .animation(.easeOut(duration: 0.08), value: level)
                }
            }
            .frame(height: 28, alignment: .center)
        }
    }
}

// MARK: - Success bars (done)

private struct SuccessBarsView: View {
    var body: some View {
        TimelineView(.animation) { timeline in
            let time = timeline.date.timeIntervalSinceReferenceDate
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { index in
                    let phase = sin(time * 6.5 + Double(index) * 0.7)
                    let scale = 0.62 + 0.38 * CGFloat((phase + 1) / 2)
                    Capsule()
                        .fill(Color(red: 0.19, green: 0.82, blue: 0.35))
                        .frame(width: 4, height: index == 1 ? 17 : 10)
                        .scaleEffect(x: 1, y: scale, anchor: .center)
                        .opacity(0.68 + 0.32 * scale)
                        .shadow(
                            color: Color(red: 0.19, green: 0.82, blue: 0.35).opacity(0.45),
                            radius: 8
                        )
                }
            }
        }
        .frame(width: 22, height: 18)
    }
}

// MARK: - Error pulse (error)

private struct ErrorPulseView: View {
    var body: some View {
        TimelineView(.animation) { timeline in
            let time = timeline.date.timeIntervalSinceReferenceDate
            let pulse = 0.72 + 0.28 * CGFloat((sin(time * 5.2) + 1) / 2)
            Circle()
                .fill(Color.red.opacity(0.86))
                .frame(width: 8, height: 8)
                .scaleEffect(pulse)
                .shadow(color: .red.opacity(0.55), radius: 8)
        }
        .frame(width: 12, height: 12)
    }
}

// MARK: - Processing dots (processing)

private struct ProcessingDotsView: View {
    @State private var active = 0
    private let timer = Timer.publish(every: 0.18, on: .main, in: .common).autoconnect()

    var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<5, id: \.self) { i in
                Circle()
                    .fill(.white.opacity(i == active ? 0.86 : 0.25))
                    .frame(width: 3, height: 3)
                    .animation(.easeInOut(duration: 0.16), value: active)
            }
        }
        .onReceive(timer) { _ in
            active = (active + 1) % 5
        }
    }
}
