import Foundation
import os

final class DeepgramStream {
    private var task: URLSessionWebSocketTask?
    private var session: URLSession?
    private let logger = RuntimeLogger(category: "deepgram")
    private var resultChunks: [ResultChunk] = []
    private var isClosed = false
    private var chunksSent = 0
    private var sendErrorCount = 0
    private var currentSTTMode = "multi"
    private(set) var isConnected = false

    private struct ResultChunk {
        var enriched: String
        var plain: String
        var wordCount: Int
        var lowConfidenceCount: Int
        var confidenceSum: Double
        var languages: [String]
    }

    func connect(
        apiKey: String, sttMode: String = "multi", sampleRate: Int = 16000,
        keyterms: [String] = [], replacements: [ReplacementRule] = []
    ) -> Bool {
        let endpointing = sttMode == "multi" ? 100 : 500
        var urlStr = "wss://api.deepgram.com/v1/listen"
        urlStr += "?model=nova-3&language=\(sttMode)"
        urlStr += "&punctuate=true&encoding=linear16"
        urlStr += "&sample_rate=\(sampleRate)&channels=1"
        urlStr += "&interim_results=true&endpointing=\(endpointing)"
        urlStr += "&utterance_end_ms=1000"

        for term in keyterms.prefix(200) {
            if let encoded = term.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) {
                urlStr += "&keywords=\(encoded)"
            }
        }
        for rule in replacements.prefix(100) {
            if let rep = rule.replace,
               let encoded = "\(rule.find):\(rep)".addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) {
                urlStr += "&replace=\(encoded)"
            }
        }

        guard let url = URL(string: urlStr) else { return false }
        var req = URLRequest(url: url)
        req.setValue("Token \(apiKey)", forHTTPHeaderField: "Authorization")

        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 30
        config.timeoutIntervalForResource = 300
        let sess = URLSession(configuration: config)
        let ws = sess.webSocketTask(with: req)
        ws.resume()

        task = ws
        session = sess
        resultChunks = []
        isClosed = false
        chunksSent = 0
        sendErrorCount = 0
        currentSTTMode = sttMode
        isConnected = true

        startReceiveLoop()
        logger.info("[timing] DG WS started (mode=\(sttMode), keyterms=\(keyterms.count), replacements=\(replacements.count))")
        return true
    }

    func sendAudio(_ pcm: Data) {
        guard let ws = task, !isClosed else { return }
        chunksSent += 1
        ws.send(.data(pcm)) { [weak self] error in
            if let error {
                guard let self else { return }
                self.sendErrorCount += 1
                if self.sendErrorCount == 1 {
                    self.logger.error("WS send error: \(error.localizedDescription)")
                }
            }
        }
    }

    func sendCloseStream() {
        guard let ws = task, !isClosed else { return }
        ws.send(.string(#"{"type":"CloseStream"}"#)) { [weak self] error in
            if let error {
                self?.logger.error("CloseStream send failed: \(error.localizedDescription)")
            }
        }
        logger.info("[timing] CloseStream sent (chunks=\(self.chunksSent))")
    }

    func waitForClose(timeoutMs: Int = 3000) async -> StreamingTranscript? {
        let start = ContinuousClock.now
        let deadline = start + .milliseconds(timeoutMs)
        while ContinuousClock.now < deadline && !isClosed {
            try? await Task.sleep(for: .milliseconds(20))
        }
        let drainMs = Self.ms(from: start)
        logger.info("[timing] drain: \(drainMs)ms, parts=\(self.resultChunks.count), closed=\(self.isClosed), send_errors=\(self.sendErrorCount)")

        let enriched = enrichedTranscript
        let plain = plainTranscript
        guard !plain.isEmpty else { return nil }

        let totalWords = resultChunks.reduce(0) { $0 + $1.wordCount }
        let totalLowConf = resultChunks.reduce(0) { $0 + $1.lowConfidenceCount }
        let totalConfSum = resultChunks.reduce(0.0) { $0 + $1.confidenceSum }
        var allLangs: [String] = []
        for chunk in resultChunks {
            for lang in chunk.languages where !allLangs.contains(lang) {
                allLangs.append(lang)
            }
        }
        let meanConf = totalWords > 0 ? totalConfSum / Double(totalWords) : 0.95

        return StreamingTranscript(
            transcript: enriched,
            meta: TranscriptMeta(
                enriched_transcript: enriched,
                confidence: meanConf,
                mean_word_confidence: meanConf,
                low_confidence_count: totalLowConf,
                word_count: totalWords,
                languages: allLangs,
                stt_mode: currentSTTMode
            )
        )
    }

    var enrichedTranscript: String {
        resultChunks.map { $0.enriched.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }

    var plainTranscript: String {
        resultChunks.map { $0.plain.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }

    func disconnect() {
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
        isConnected = false
    }

    private func startReceiveLoop() {
        guard let ws = task else { return }
        ws.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(.string(let text)):
                if let chunk = self.extractResultChunk(text) {
                    self.resultChunks.append(chunk)
                    self.logger.info("segment: \(chunk.enriched)")
                }
                self.startReceiveLoop()
            case .success(.data):
                self.startReceiveLoop()
            case .failure(let error):
                self.logger.info("WS receive ended: \(error.localizedDescription)")
                self.isClosed = true
            @unknown default:
                break
            }
        }
    }

    private func extractResultChunk(_ json: String) -> ResultChunk? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              obj["type"] as? String == "Results",
              obj["is_final"] as? Bool == true,
              let channel = obj["channel"] as? [String: Any],
              let alts = channel["alternatives"] as? [[String: Any]],
              let first = alts.first
        else { return nil }

        guard let words = first["words"] as? [[String: Any]], !words.isEmpty else {
            guard let transcript = first["transcript"] as? String,
                  !transcript.trimmingCharacters(in: .whitespaces).isEmpty
            else { return nil }
            return ResultChunk(
                enriched: transcript, plain: transcript,
                wordCount: transcript.split(separator: " ").count,
                lowConfidenceCount: 0, confidenceSum: 0.95 * Double(transcript.split(separator: " ").count),
                languages: []
            )
        }

        var enrichedParts: [String] = []
        var plainParts: [String] = []
        var languages: [String] = []
        var confidenceSum = 0.0
        var lowConfCount = 0

        for w in words {
            let word = (w["punctuated_word"] as? String) ?? (w["word"] as? String) ?? ""
            guard !word.isEmpty else { continue }

            let conf = (w["confidence"] as? Double) ?? 1.0
            confidenceSum += conf

            if let lang = w["language"] as? String, !languages.contains(lang) {
                languages.append(lang)
            }

            if conf < LOW_CONFIDENCE_THRESHOLD {
                enrichedParts.append("[\(word)?\(Int(conf * 100))%]")
                lowConfCount += 1
            } else {
                enrichedParts.append(word)
            }
            plainParts.append(word)
        }

        if let altLangs = first["languages"] as? [String] {
            for lang in altLangs where !languages.contains(lang) {
                languages.append(lang)
            }
        }

        let enriched = enrichedParts.joined(separator: " ")
        let plain = plainParts.joined(separator: " ")
        guard !plain.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }

        return ResultChunk(
            enriched: enriched, plain: plain,
            wordCount: words.count, lowConfidenceCount: lowConfCount,
            confidenceSum: confidenceSum, languages: languages
        )
    }

    private static func ms(from start: ContinuousClock.Instant) -> Int {
        let d = ContinuousClock.now - start
        return Int(d.components.seconds * 1000
            + d.components.attoseconds / 1_000_000_000_000_000)
    }
}
