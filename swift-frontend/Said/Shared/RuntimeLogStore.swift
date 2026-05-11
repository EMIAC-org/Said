import Darwin
import Foundation
import OSLog

final class RuntimeLogStore: @unchecked Sendable {
    static let shared = RuntimeLogStore()

    private let lock = NSLock()
    private let fileManager = FileManager.default
    private let formatter = ISO8601DateFormatter()
    private let appRunMarker = "===== Said app run started"
    private let maxRunsToKeep = 3
    private var didStartRun = false
    private var didRedirectStreams = false
    private let debugEnabled = ProcessInfo.processInfo.environment["SAID_SWIFT_DEBUG"] == "1"

    let logsDirectory: URL
    let appLogURL: URL

    private init() {
        logsDirectory = URL(fileURLWithPath: NSHomeDirectory())
            .appendingPathComponent("Library/Logs/Said")
        appLogURL = logsDirectory.appendingPathComponent("said-swift.log")
    }

    func startRun() {
        lock.lock()
        defer { lock.unlock() }

        guard !didStartRun else { return }
        didStartRun = true

        createLogsDirectory()
        trimToRecentRunsLocked(marker: appRunMarker, previousRunsToKeep: maxRunsToKeep - 1)

        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "dev"
        let header = """

        \(appRunMarker) \(formatter.string(from: Date())) pid=\(ProcessInfo.processInfo.processIdentifier) version=\(version) build=\(build) =====
        """
        appendLocked(header)
        redirectStandardStreamsLocked()
    }

    func append(level: String, category: String, message: String) {
        lock.lock()
        defer { lock.unlock() }

        if !didStartRun {
            didStartRun = true
            createLogsDirectory()
            trimToRecentRunsLocked(marker: appRunMarker, previousRunsToKeep: maxRunsToKeep - 1)
            let header = "\n\(appRunMarker) \(formatter.string(from: Date())) pid=\(ProcessInfo.processInfo.processIdentifier) =====\n"
            appendLocked(header)
            redirectStandardStreamsLocked()
        }

        for line in message.split(whereSeparator: \.isNewline) {
            appendLocked("\(formatter.string(from: Date())) \(level) \(category): \(line)\n")
        }
    }

    func appendDebug(category: String, message: String) {
        guard debugEnabled else { return }
        append(level: "DEBUG", category: category, message: message)
    }

    func readLastRuns(maxBytes: UInt64) -> (text: String, truncated: Bool) {
        readLastRuns(at: appLogURL, marker: appRunMarker, maxRuns: maxRunsToKeep, maxBytes: maxBytes)
    }

    func readLastRuns(at url: URL, marker: String, maxRuns: Int, maxBytes: UInt64) -> (text: String, truncated: Bool) {
        guard let data = try? Data(contentsOf: url), !data.isEmpty else {
            return ("", false)
        }

        var text = String(decoding: data, as: UTF8.self)
        var truncated = false
        if let trimmed = suffixFromLastRuns(text, marker: marker, maxRuns: maxRuns) {
            truncated = trimmed.count < text.count
            text = trimmed
        }

        let trimmedBytes = trimToRecentBytes(text, maxBytes: maxBytes)
        return (trimmedBytes.text, truncated || trimmedBytes.truncated)
    }

    private func createLogsDirectory() {
        try? fileManager.createDirectory(at: logsDirectory, withIntermediateDirectories: true)
    }

    private func redirectStandardStreamsLocked() {
        guard !didRedirectStreams else { return }
        didRedirectStreams = true

        let fd = open(appLogURL.path, O_WRONLY | O_CREAT | O_APPEND, S_IRUSR | S_IWUSR | S_IRGRP | S_IROTH)
        guard fd >= 0 else { return }
        dup2(fd, STDOUT_FILENO)
        dup2(fd, STDERR_FILENO)
        close(fd)
    }

    private func appendLocked(_ text: String) {
        guard let data = text.data(using: .utf8) else { return }
        if !fileManager.fileExists(atPath: appLogURL.path) {
            fileManager.createFile(atPath: appLogURL.path, contents: nil)
        }
        guard let handle = try? FileHandle(forWritingTo: appLogURL) else { return }
        defer { try? handle.close() }
        _ = try? handle.seekToEnd()
        try? handle.write(contentsOf: data)
    }

    private func trimToRecentRunsLocked(marker: String, previousRunsToKeep: Int) {
        guard let data = try? Data(contentsOf: appLogURL), !data.isEmpty else {
            return
        }
        let text = String(decoding: data, as: UTF8.self)
        guard let trimmed = suffixFromLastRuns(text, marker: marker, maxRuns: previousRunsToKeep),
              trimmed.count < text.count,
              let out = trimmed.data(using: .utf8)
        else {
            return
        }
        try? out.write(to: appLogURL, options: .atomic)
    }

    private func suffixFromLastRuns(_ text: String, marker: String, maxRuns: Int) -> String? {
        guard maxRuns > 0 else { return "" }

        var positions: [String.Index] = []
        var searchStart = text.startIndex
        while let range = text.range(of: marker, range: searchStart..<text.endIndex) {
            let lineStart = text[..<range.lowerBound].lastIndex(of: "\n").map { text.index(after: $0) } ?? text.startIndex
            positions.append(lineStart)
            searchStart = range.upperBound
        }

        guard positions.count > maxRuns else { return nil }
        return String(text[positions[positions.count - maxRuns]...])
    }

    private func trimToRecentBytes(_ text: String, maxBytes: UInt64) -> (text: String, truncated: Bool) {
        let data = Data(text.utf8)
        guard data.count > maxBytes else {
            return (text, false)
        }

        let suffix = data.suffix(Int(maxBytes))
        var trimmed = String(decoding: suffix, as: UTF8.self)
        if let newline = trimmed.firstIndex(of: "\n") {
            trimmed = String(trimmed[trimmed.index(after: newline)...])
        }
        return (trimmed, true)
    }
}

struct RuntimeLogger {
    private let logger: Logger
    private let category: String

    init(category: String) {
        self.category = category
        logger = Logger(subsystem: "com.emiac.said", category: category)
    }

    func debug(_ message: String) {
        logger.debug("\(message, privacy: .public)")
        RuntimeLogStore.shared.appendDebug(category: category, message: message)
    }

    func info(_ message: String) {
        logger.info("\(message, privacy: .public)")
        RuntimeLogStore.shared.append(level: "INFO", category: category, message: message)
    }

    func warning(_ message: String) {
        logger.warning("\(message, privacy: .public)")
        RuntimeLogStore.shared.append(level: "WARN", category: category, message: message)
    }

    func error(_ message: String) {
        logger.error("\(message, privacy: .public)")
        RuntimeLogStore.shared.append(level: "ERROR", category: category, message: message)
    }
}
