import Combine
import SwiftUI

enum NotchViewState: Equatable {
    case closed
    case open
}

final class NotchViewModel: NSObject, ObservableObject {
    @Published var notchState: NotchViewState = .closed
    @Published var notchSize: CGSize = .zero
    @Published var closedNotchSize: CGSize = .zero
    @Published var dictationState: NotchState = .idle
    @Published var audioLevel: Float = 0
    @Published var polishedText: String = ""
    @Published var transcriptPreview: String = ""
    @Published var processingPhase: String = "Transcribing"
    @Published var isExpanded = false
    @Published var lastResult: String = ""
    @Published var activeHotkeyLabel: String = "Caps Lock"
    @Published var outputLanguage: String = "hinglish"
    @Published var backendReady: Bool = false

    var metrics: NotchMetrics
    private var collapseWorkItem: DispatchWorkItem?

    var isRecording: Bool {
        if case .recording = dictationState { return true }
        return false
    }

    var isActiveLifecycle: Bool {
        switch dictationState {
        case .idle:
            return false
        case .recording, .processing, .done, .error:
            return true
        }
    }

    var effectiveClosedNotchHeight: CGFloat {
        closedNotchSize.height
    }

    override init() {
        metrics = NotchDetector.detect()
        super.init()
        closedNotchSize = metrics.closedSize
        notchSize = closedNotchSize
    }

    func refreshMetrics(screen: NSScreen? = nil) {
        metrics = NotchDetector.detect(screen: screen)
        closedNotchSize = metrics.closedSize
        if notchState == .closed && !isExpanded {
            notchSize = closedNotchSize
        }
    }

    func open() {
        notchSize = CGSize(width: 640, height: 190)
        notchState = .open
    }

    func close() {
        guard !isExpanded else { return }
        notchSize = closedNotchSize
        notchState = .closed
    }

    func startRecording() {
        collapseWorkItem?.cancel()
        polishedText = ""
        transcriptPreview = ""
        processingPhase = "Recording"
        dictationState = .recording
        isExpanded = true
    }

    func startProcessing(phase: String = "Transcribing") {
        collapseWorkItem?.cancel()
        polishedText = ""
        processingPhase = phase
        dictationState = .processing
        isExpanded = true
    }

    func appendToken(_ token: String) {
        polishedText += token
    }

    func setProcessingTranscript(_ transcript: String, phase: String = "Enhancing") {
        transcriptPreview = transcript
        processingPhase = phase
        if case .processing = dictationState {
            isExpanded = true
        }
    }

    func setProcessingPhase(_ phase: String) {
        processingPhase = phase
        if case .processing = dictationState {
            isExpanded = true
        }
    }

    func finish(text finalText: String? = nil) {
        collapseWorkItem?.cancel()
        if let finalText {
            polishedText = finalText
        }
        let text = polishedText
        lastResult = text
        dictationState = .done(text)
        let workItem = DispatchWorkItem { [weak self] in
            guard let self, case .done = self.dictationState else { return }
            self.dictationState = .idle
            self.isExpanded = false
            self.transcriptPreview = ""
        }
        collapseWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0, execute: workItem)
    }

    func showError(_ message: String) {
        collapseWorkItem?.cancel()
        dictationState = .error(message)
        isExpanded = true
        let workItem = DispatchWorkItem { [weak self] in
            guard let self, case .error = self.dictationState else { return }
            self.dictationState = .idle
            self.isExpanded = false
            self.transcriptPreview = ""
        }
        collapseWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.0, execute: workItem)
    }
}
