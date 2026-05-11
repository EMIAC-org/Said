import AVFoundation
import Foundation
import os

final class AudioRecorder: ObservableObject {
    @Published var isRecording = false
    @Published var level: Float = 0

    private var engine: AVAudioEngine?
    private var pcmBuffers: [Data] = []
    private let logger = Logger(subsystem: "com.emiac.said", category: "audio")
    private let targetSampleRate: Double = 16000

    var onLevelUpdate: ((Float) -> Void)?
    var onChunk: ((Data) -> Void)?

    func start() {
        guard !isRecording else { return }
        pcmBuffers = []

        let engine = AVAudioEngine()
        let input = engine.inputNode
        let nativeFormat = input.outputFormat(forBus: 0)
        let nativeRate = nativeFormat.sampleRate

        logger.info("starting capture at \(nativeRate) Hz, channels=\(nativeFormat.channelCount)")

        let bufferSize: AVAudioFrameCount = 4096
        input.installTap(onBus: 0, bufferSize: bufferSize, format: nativeFormat) { [weak self] buffer, _ in
            guard let self else { return }
            let samples = self.extractSamples(buffer)
            let rms = AudioLevelComputer.rms(samples)
            let normalized = AudioLevelComputer.normalized(rms)
            DispatchQueue.main.async { self.level = normalized }
            self.onLevelUpdate?(normalized)

            let resampled = self.resampleTo16k(samples, from: nativeRate)
            let pcm = self.floatToPCM16(resampled)
            self.pcmBuffers.append(pcm)
            self.onChunk?(pcm)
        }

        do {
            try engine.start()
            self.engine = engine
            isRecording = true
            logger.info("recording started")
        } catch {
            logger.error("engine start failed: \(error)")
        }
    }

    func stop() -> Data {
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        isRecording = false
        level = 0
        logger.info("recording stopped, \(self.pcmBuffers.count) chunks")

        let combined = pcmBuffers.reduce(Data()) { $0 + $1 }
        pcmBuffers = []
        return wrapInWAV(pcm: combined, sampleRate: UInt32(targetSampleRate))
    }

    private func extractSamples(_ buffer: AVAudioPCMBuffer) -> [Float] {
        guard let channelData = buffer.floatChannelData else { return [] }
        let count = Int(buffer.frameLength)
        return Array(UnsafeBufferPointer(start: channelData[0], count: count))
    }

    private func resampleTo16k(_ samples: [Float], from sourceRate: Double) -> [Float] {
        guard sourceRate != targetSampleRate else { return samples }
        let ratio = targetSampleRate / sourceRate
        let outputCount = Int(Double(samples.count) * ratio)
        var output = [Float](repeating: 0, count: outputCount)
        for i in 0..<outputCount {
            let srcIdx = Double(i) / ratio
            let lo = Int(srcIdx)
            let hi = min(lo + 1, samples.count - 1)
            let frac = Float(srcIdx - Double(lo))
            output[i] = samples[lo] * (1 - frac) + samples[hi] * frac
        }
        return output
    }

    private func floatToPCM16(_ samples: [Float]) -> Data {
        var data = Data(capacity: samples.count * 2)
        for s in samples {
            var i16 = Int16((max(-1, min(1, s)) * 32767).rounded())
            data.append(contentsOf: withUnsafeBytes(of: &i16) { Array($0) })
        }
        return data
    }

    private func wrapInWAV(pcm: Data, sampleRate: UInt32) -> Data {
        var wav = Data()
        let totalSize = UInt32(44 + pcm.count)
        let bitsPerSample: UInt16 = 16
        let channels: UInt16 = 1
        let byteRate = sampleRate * UInt32(channels) * UInt32(bitsPerSample / 8)
        let blockAlign = channels * (bitsPerSample / 8)

        wav.append("RIFF".data(using: .ascii)!)
        wav.append(withUnsafeBytes(of: totalSize - 8) { Data($0) })
        wav.append("WAVE".data(using: .ascii)!)
        wav.append("fmt ".data(using: .ascii)!)
        wav.append(withUnsafeBytes(of: UInt32(16)) { Data($0) })
        wav.append(withUnsafeBytes(of: UInt16(1)) { Data($0) })
        wav.append(withUnsafeBytes(of: channels) { Data($0) })
        wav.append(withUnsafeBytes(of: sampleRate) { Data($0) })
        wav.append(withUnsafeBytes(of: byteRate) { Data($0) })
        wav.append(withUnsafeBytes(of: blockAlign) { Data($0) })
        wav.append(withUnsafeBytes(of: bitsPerSample) { Data($0) })
        wav.append("data".data(using: .ascii)!)
        wav.append(withUnsafeBytes(of: UInt32(pcm.count)) { Data($0) })
        wav.append(pcm)
        return wav
    }
}
