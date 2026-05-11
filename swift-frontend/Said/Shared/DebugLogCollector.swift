import Foundation
import OSLog

enum DebugLogCollector {
    private static let launchDate = Date()
    private static let maxLogBytes: UInt64 = 768 * 1024
    private static let subsystem = "com.emiac.said"

    static func collect(backendHealthy: Bool, backendPort: Int) async -> DebugLogs {
        await Task.detached(priority: .utility) {
            let app = collectAppLogs()
            let backendPath = backendLogPath()
            let backend = readRecentLog(
                at: backendPath,
                marker: "polish-backend build="
            )

            let runtimeHeader = """
            Runtime
            - Backend: \(backendHealthy ? "healthy" : "not healthy")
            - Backend URL: http://127.0.0.1:\(backendPort)
            - App log source: Unified Logging, subsystem == "\(subsystem)", since this app launch
            - Backend log source: \(backendPath.path)
            """

            let appText = app.text.trimmingCharacters(in: .whitespacesAndNewlines)
            let backendText = backend.text.trimmingCharacters(in: .whitespacesAndNewlines)
            let combined = """
            \(runtimeHeader)

            -- Said app --
            \(appText.isEmpty ? "(no app logs found for this launch)" : appText)

            -- said-backend --
            \(backendText.isEmpty ? "(no backend log found)" : backendText)
            """

            return DebugLogs(
                combined: combined,
                desktop: appText,
                backend: backendText,
                desktop_path: "Unified Logging: subsystem == \(subsystem)",
                backend_path: backendPath.path,
                truncated: app.truncated || backend.truncated
            )
        }.value
    }

    private static func collectAppLogs() -> (text: String, truncated: Bool) {
        do {
            let store = try OSLogStore(scope: .currentProcessIdentifier)
            let position = store.position(date: launchDate)
            let entries = try store.getEntries(at: position)
            let formatter = ISO8601DateFormatter()

            var lines: [String] = []
            for entry in entries {
                guard let log = entry as? OSLogEntryLog, log.subsystem == subsystem else {
                    continue
                }
                let line = "\(formatter.string(from: log.date)) \(levelName(log.level)) \(log.category): \(log.composedMessage)"
                lines.append(line)
            }

            let text = lines.joined(separator: "\n")
            return trimToRecentBytes(text)
        } catch {
            return ("Unable to read app unified logs: \(error.localizedDescription)", false)
        }
    }

    private static func backendLogPath() -> URL {
        URL(fileURLWithPath: NSHomeDirectory())
            .appendingPathComponent("Library/Logs/Said/backend.log")
    }

    private static func readRecentLog(at url: URL, marker: String) -> (text: String, truncated: Bool) {
        guard let handle = try? FileHandle(forReadingFrom: url) else {
            return ("", false)
        }
        defer { try? handle.close() }

        let length = (try? handle.seekToEnd()) ?? 0
        let start = length > maxLogBytes ? length - maxLogBytes : 0
        do {
            try handle.seek(toOffset: start)
            let data = try handle.readToEnd() ?? Data()
            var text = String(decoding: data, as: UTF8.self)
            if let markerRange = text.range(of: marker, options: .backwards) {
                let prefix = text[..<markerRange.lowerBound]
                let lineStart = prefix.lastIndex(of: "\n").map { text.index(after: $0) } ?? text.startIndex
                text = String(text[lineStart...])
            }
            return (text, start > 0)
        } catch {
            return ("Unable to read backend log: \(error.localizedDescription)", false)
        }
    }

    private static func trimToRecentBytes(_ text: String) -> (text: String, truncated: Bool) {
        let data = Data(text.utf8)
        guard data.count > maxLogBytes else {
            return (text, false)
        }

        let suffix = data.suffix(Int(maxLogBytes))
        var trimmed = String(decoding: suffix, as: UTF8.self)
        if let newline = trimmed.firstIndex(of: "\n") {
            trimmed = String(trimmed[trimmed.index(after: newline)...])
        }
        return (trimmed, true)
    }

    private static func levelName(_ level: OSLogEntryLog.Level) -> String {
        switch level {
        case .undefined: return "UNDEF"
        case .debug: return "DEBUG"
        case .info: return "INFO"
        case .notice: return "NOTICE"
        case .error: return "ERROR"
        case .fault: return "FAULT"
        @unknown default: return "LOG"
        }
    }
}
