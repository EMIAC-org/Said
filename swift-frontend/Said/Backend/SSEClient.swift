import Foundation
import os

final class SSEClient {
    private let logger = Logger(subsystem: "com.emiac.said", category: "sse")

    func streamTokens(request: URLRequest) -> AsyncThrowingStream<PolishToken, Error> {
        AsyncThrowingStream { continuation in
            let handler = SSEStreamHandler(continuation: continuation)
            let config = URLSessionConfiguration.default
            config.timeoutIntervalForRequest = 120
            let session = URLSession(configuration: config, delegate: handler, delegateQueue: nil)
            let dataTask = session.dataTask(with: request)

            continuation.onTermination = { _ in
                dataTask.cancel()
                session.finishTasksAndInvalidate()
            }

            dataTask.resume()
        }
    }
}

private class SSEStreamHandler: NSObject, URLSessionDataDelegate {
    let continuation: AsyncThrowingStream<PolishToken, Error>.Continuation
    private var buffer = ""
    private var eventName = ""
    private var finished = false
    private var chunkCount = 0
    private var totalBytes = 0
    private var firstChunkTime: ContinuousClock.Instant?
    private let startTime = ContinuousClock.now
    private let logger = Logger(subsystem: "com.emiac.said", category: "sse-handler")

    init(continuation: AsyncThrowingStream<PolishToken, Error>.Continuation) {
        self.continuation = continuation
    }

    func urlSession(
        _ session: URLSession, dataTask: URLSessionDataTask,
        didReceive response: URLResponse,
        completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
    ) {
        let elapsed = ContinuousClock.now - startTime
        let ms = Int(elapsed.components.seconds * 1000 + elapsed.components.attoseconds / 1_000_000_000_000_000)
        if let http = response as? HTTPURLResponse {
            logger.info("[timing] SSE HTTP response: \(http.statusCode) in \(ms)ms")
            if !(200...299).contains(http.statusCode) {
                continuation.finish(throwing: BackendError.httpError(http.statusCode))
                finished = true
                completionHandler(.cancel)
                return
            }
        }
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard !finished else { return }
        if firstChunkTime == nil {
            firstChunkTime = ContinuousClock.now
            let elapsed = ContinuousClock.now - startTime
            let ms = Int(elapsed.components.seconds * 1000 + elapsed.components.attoseconds / 1_000_000_000_000_000)
            logger.info("[timing] SSE first data chunk: \(ms)ms, \(data.count) bytes")
        }
        chunkCount += 1
        totalBytes += data.count
        guard let chunk = String(data: data, encoding: .utf8) else { return }
        buffer += chunk
        processBuffer()
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        let elapsed = ContinuousClock.now - startTime
        let ms = Int(elapsed.components.seconds * 1000 + elapsed.components.attoseconds / 1_000_000_000_000_000)
        logger.info("[timing] SSE stream ended: \(ms)ms, \(self.chunkCount) chunks, \(self.totalBytes) bytes")
        guard !finished else { return }
        finished = true
        if let error {
            if (error as NSError).code == NSURLErrorCancelled { return }
            continuation.finish(throwing: error)
        } else {
            continuation.finish()
        }
        session.finishTasksAndInvalidate()
    }

    private func processBuffer() {
        while let nlRange = buffer.range(of: "\n") {
            let line = String(buffer[buffer.startIndex..<nlRange.lowerBound])
                .trimmingCharacters(in: .whitespacesAndNewlines)
            buffer = String(buffer[nlRange.upperBound...])

            if line.isEmpty {
                eventName = ""
                continue
            }

            if let name = line.stripPrefix("event: ") ?? line.stripPrefix("event:") {
                eventName = name.trimmingCharacters(in: .whitespaces)
                continue
            }

            guard let payload = line.stripPrefix("data: ") ?? line.stripPrefix("data:") else {
                continue
            }
            let trimmed = payload.trimmingCharacters(in: .whitespaces)

            if trimmed == "[DONE]" {
                finishStream()
                return
            }

            dispatchPayload(trimmed)
        }
    }

    private func dispatchPayload(_ payload: String) {
        guard !finished else { return }
        guard let data = payload.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return
        }

        switch eventName {
        case "token":
            if let token = obj["token"] as? String {
                continuation.yield(PolishToken(text: token, done: false))
            }
        case "done":
            let done = try? JSONDecoder().decode(PolishDone.self, from: data)
            finishStream(final: done)
        case "status":
            if let phase = obj["phase"] as? String {
                logger.info("[timing] SSE status: phase=\(phase)")
            }
        case "error":
            if let msg = obj["message"] as? String {
                finished = true
                continuation.finish(throwing: SSEError.backend(msg))
            }
        default:
            if let token = obj["token"] as? String {
                continuation.yield(PolishToken(text: token, done: false))
            } else if obj["phase"] != nil {
                // skip
            } else if obj["polished"] != nil {
                let done = try? JSONDecoder().decode(PolishDone.self, from: data)
                finishStream(final: done)
            }
        }
    }

    private func finishStream(final: PolishDone? = nil) {
        guard !finished else { return }
        finished = true
        continuation.yield(PolishToken(text: "", done: true, final: final))
        continuation.finish()
    }
}

private extension String {
    func stripPrefix(_ prefix: String) -> String? {
        hasPrefix(prefix) ? String(dropFirst(prefix.count)) : nil
    }
}

enum SSEError: Error, LocalizedError {
    case backend(String)

    var errorDescription: String? {
        switch self {
        case .backend(let msg): return msg
        }
    }
}
