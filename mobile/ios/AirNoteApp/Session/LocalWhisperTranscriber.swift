import Foundation
import whisper

/// On-device speech-to-text: whisper.cpp running the Clario Hinglish model
/// (`Marquestra/clario-hinglish-stt-base-q8`, revision 31322d70) bundled in the app.
///
/// Decode settings mirror the flags the model was benchmarked with:
/// `-l hi -bs 5 -nth 0.75 -lpt -0.35 -et 3.0 -sns -tpi 0.2 -nt`
/// plus Silero VAD (`-vt 0.5 -vsd 100 -vp 250`).
///
/// Runs on the CPU only. The warm keyboard session transcribes while AirNote
/// is in the background, and iOS rejects Metal (GPU) work from background apps.
final class LocalWhisperTranscriber: @unchecked Sendable {
    static let shared = LocalWhisperTranscriber()

    static let modelResource = "clario-hinglish-stt-base-q8"
    static let vadResource = "ggml-silero-v5.1.2"
    static let modelName = "on-device/clario-hinglish-stt-base-q8"

    enum TranscriberError: LocalizedError {
        case modelMissing
        case modelLoadFailed
        case decodeFailed(Int32)

        var errorDescription: String? {
            switch self {
            case .modelMissing: return "The on-device speech model is missing from this build."
            case .modelLoadFailed: return "The on-device speech model couldn't be loaded."
            case .decodeFailed(let code): return "On-device transcription failed (\(code))."
            }
        }
    }

    /// Serial: whisper contexts are not thread-safe, and one decode at a time is
    /// all a dictation app needs.
    private let queue = DispatchQueue(label: "com.emiac.airnote.local-whisper", qos: .userInitiated)
    private var context: OpaquePointer?
    private let vadPath: String? = Bundle.main.path(forResource: vadResource, ofType: "bin")

    private init() {}

    deinit {
        if let context { whisper_free(context) }
    }

    /// Load the model ahead of the first dictation so it doesn't pay the load cost.
    func prewarm() {
        queue.async { [weak self] in _ = try? self?.loadIfNeeded() }
    }

    /// Transcribe 16 kHz mono float PCM. Returns trimmed text ("" when no speech).
    func transcribe(_ samples: [Float]) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { [weak self] in
                guard let self else {
                    continuation.resume(returning: "")
                    return
                }
                do {
                    continuation.resume(returning: try self.decode(samples))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    // MARK: Private (queue only)

    private func loadIfNeeded() throws -> OpaquePointer {
        if let context { return context }
        guard let path = Bundle.main.path(forResource: Self.modelResource, ofType: "bin") else {
            report { $0.didFail("Model missing from this build") }
            throw TranscriberError.modelMissing
        }
        report { $0.didStartLoading() }
        let started = Date()
        var params = whisper_context_default_params()
        params.use_gpu = false
        params.flash_attn = false
        guard let ctx = whisper_init_from_file_with_params(path, params) else {
            report { $0.didFail("Model couldn't be loaded") }
            throw TranscriberError.modelLoadFailed
        }
        context = ctx
        let loadMS = Int(Date().timeIntervalSince(started) * 1000)
        report { $0.didLoad(ms: loadMS) }
        return ctx
    }

    private func decode(_ samples: [Float]) throws -> String {
        guard !samples.isEmpty else { return "" }
        let ctx = try loadIfNeeded()
        let started = Date()

        var params = whisper_full_default_params(WHISPER_SAMPLING_BEAM_SEARCH)
        params.beam_search.beam_size = 5
        params.print_progress = false
        params.print_realtime = false
        params.print_timestamps = false
        params.print_special = false
        params.translate = false
        params.no_timestamps = true
        params.single_segment = false
        params.suppress_blank = true
        params.suppress_nst = true
        params.temperature = 0
        params.temperature_inc = 0.2
        params.entropy_thold = 3.0
        params.logprob_thold = -0.35
        params.no_speech_thold = 0.75
        params.n_threads = Int32(max(1, min(4, ProcessInfo.processInfo.activeProcessorCount - 2)))

        var vadParams = whisper_vad_default_params()
        vadParams.threshold = 0.5
        vadParams.min_silence_duration_ms = 100
        vadParams.speech_pad_ms = 250
        params.vad_params = vadParams

        let status: Int32 = "hi".withCString { language in
            params.language = language
            if let vadPath {
                return vadPath.withCString { vad in
                    params.vad = true
                    params.vad_model_path = vad
                    return samples.withUnsafeBufferPointer { buffer in
                        whisper_full(ctx, params, buffer.baseAddress, Int32(buffer.count))
                    }
                }
            }
            params.vad = false
            return samples.withUnsafeBufferPointer { buffer in
                whisper_full(ctx, params, buffer.baseAddress, Int32(buffer.count))
            }
        }
        guard status == 0 else { throw TranscriberError.decodeFailed(status) }
        let decodeMS = Int(Date().timeIntervalSince(started) * 1000)
        let audioSeconds = Double(samples.count) / 16_000
        report { $0.didTranscribe(decodeMS: decodeMS, audioSeconds: audioSeconds) }

        var text = ""
        for index in 0..<whisper_full_n_segments(ctx) {
            if let segment = whisper_full_get_segment_text(ctx, index) {
                text += String(cString: segment)
            }
        }
        return Self.clean(text)
    }

    private func report(_ update: @escaping @MainActor (OnDeviceModelStatus) -> Void) {
        Task { @MainActor in update(OnDeviceModelStatus.shared) }
    }

    /// Trim whitespace and the stray edge ellipses whisper sometimes emits.
    static func clean(_ text: String) -> String {
        var value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        while value.hasPrefix("…") || value.hasPrefix("...") {
            value = String(value.drop(while: { $0 == "." || $0 == "…" }))
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
        while value.hasSuffix("…") || value.hasSuffix("...") {
            value = String(value.reversed().drop(while: { $0 == "." || $0 == "…" }).reversed())
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return value
    }
}
