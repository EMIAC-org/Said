import Foundation
import SwiftUI

/// Observable state of the on-device speech model, for the UI: is it loaded,
/// how fast was the last transcription, and how much has it transcribed in total.
/// `LocalWhisperTranscriber` reports into this; screens only read it.
@MainActor
final class OnDeviceModelStatus: ObservableObject {
    static let shared = OnDeviceModelStatus()

    enum State: Equatable {
        case notLoaded
        case loading
        case ready
        case failed(String)
    }

    // Static facts about the bundled model (Marquestra/clario-hinglish-stt-base-q8).
    static let modelName = "Clario Hinglish"
    static let modelBase = "Whisper base, fine-tuned"
    static let quantization = "8-bit (q8_0)"
    static let revision = "31322d7"
    static let languages = "Hindi, English, Hinglish"
    static let engine = "whisper.cpp 1.9.2 · CPU"
    static let trainingHours = "148 hours of Hinglish speech"
    static let vadName = "Silero VAD 5.1"

    @Published private(set) var state: State = .notLoaded
    @Published private(set) var loadMS: Int?
    @Published private(set) var lastDecodeMS: Int?
    @Published private(set) var lastAudioSeconds: Double?
    @Published private(set) var totalRuns: Int
    @Published private(set) var totalAudioSeconds: Double

    let modelSizeBytes: Int64

    private let defaults = UserDefaults.standard
    private enum Key {
        static let runs = "airnote.ondevice.runs"
        static let seconds = "airnote.ondevice.audioSeconds"
    }

    private init() {
        totalRuns = defaults.integer(forKey: Key.runs)
        totalAudioSeconds = defaults.double(forKey: Key.seconds)
        let path = Bundle.main.path(forResource: LocalWhisperTranscriber.modelResource, ofType: "bin")
        let size = path.flatMap { try? FileManager.default.attributesOfItem(atPath: $0)[.size] as? NSNumber }
        modelSizeBytes = size?.int64Value ?? 0
    }

    // MARK: Reporting (called by LocalWhisperTranscriber)

    func didStartLoading() {
        if state != .ready { state = .loading }
    }

    func didLoad(ms: Int) {
        loadMS = ms
        state = .ready
    }

    func didFail(_ message: String) {
        state = .failed(message)
    }

    func didTranscribe(decodeMS: Int, audioSeconds: Double) {
        state = .ready
        lastDecodeMS = decodeMS
        lastAudioSeconds = audioSeconds
        totalRuns += 1
        totalAudioSeconds += audioSeconds
        defaults.set(totalRuns, forKey: Key.runs)
        defaults.set(totalAudioSeconds, forKey: Key.seconds)
    }

    // MARK: Display helpers

    var isBundled: Bool { modelSizeBytes > 0 }

    var sizeLabel: String {
        guard modelSizeBytes > 0 else { return "Not installed" }
        return ByteCountFormatter.string(fromByteCount: modelSizeBytes, countStyle: .file)
    }

    var stateLabel: String {
        switch state {
        case .notLoaded: return isBundled ? "Standing by" : "Missing"
        case .loading: return "Warming up"
        case .ready: return "Ready"
        case .failed: return "Unavailable"
        }
    }

    var stateColor: Color {
        switch state {
        case .ready: return AirNoteDesign.onDevice
        case .loading, .notLoaded: return isBundled ? AirNoteDesign.warning : AirNoteDesign.danger
        case .failed: return AirNoteDesign.danger
        }
    }

    /// "4.3 s of speech in 1.2 s"
    var lastRunLabel: String? {
        guard let ms = lastDecodeMS, let audio = lastAudioSeconds else { return nil }
        return String(format: "%.1f s of speech in %.1f s", audio, Double(ms) / 1000)
    }

    /// How many times faster than real time the last run was (audio ÷ compute).
    var realtimeFactor: Double? {
        guard let ms = lastDecodeMS, ms > 0, let audio = lastAudioSeconds, audio > 0 else { return nil }
        return audio / (Double(ms) / 1000)
    }

    var totalAudioLabel: String {
        let minutes = totalAudioSeconds / 60
        if minutes < 1 { return String(format: "%.0f s", totalAudioSeconds) }
        if minutes < 60 { return String(format: "%.0f min", minutes) }
        return String(format: "%.1f h", minutes / 60)
    }

    #if DEBUG
    /// Demo-mode values so screenshots show a used, warmed-up model.
    func seedDemo() {
        state = .ready
        loadMS = 420
        lastDecodeMS = 1180
        lastAudioSeconds = 4.3
        totalRuns = 128
        totalAudioSeconds = 1_420
    }
    #endif
}
