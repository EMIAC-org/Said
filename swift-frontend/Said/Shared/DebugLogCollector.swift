import Foundation

enum DebugLogCollector {
    private static let maxLogBytes: UInt64 = 256 * 1024
    private static let backendRunMarker = "polish-backend build="

    static func collect(backendHealthy: Bool, backendPort: Int) async -> DebugLogs {
        await Task.detached(priority: .utility) {
            RuntimeLogStore.shared.startRun()
            let app = RuntimeLogStore.shared.readLastRuns(maxBytes: maxLogBytes)
            let backendPath = backendLogPath()
            let backend = RuntimeLogStore.shared.readLastRuns(
                at: backendPath,
                marker: backendRunMarker,
                maxRuns: 3,
                maxBytes: maxLogBytes
            )

            let runtimeHeader = """
            Runtime
            - Backend: \(backendHealthy ? "healthy" : "not healthy")
            - Backend URL: http://127.0.0.1:\(backendPort)
            - App log source: \(RuntimeLogStore.shared.appLogURL.path) (last 3 app runs)
            - Backend log source: \(backendPath.path)
            - Retention: latest 3 app runs + latest 3 backend runs
            """

            let appText = qualityFilter(app.text).trimmingCharacters(in: .whitespacesAndNewlines)
            let backendText = qualityFilter(backend.text).trimmingCharacters(in: .whitespacesAndNewlines)
            let combined = """
            \(runtimeHeader)

            -- Said app --
            \(appText.isEmpty ? "(no app runtime log found yet)" : appText)

            -- said-backend --
            \(backendText.isEmpty ? "(no backend log found)" : backendText)
            """

            return DebugLogs(
                combined: combined,
                desktop: appText,
                backend: backendText,
                desktop_path: RuntimeLogStore.shared.appLogURL.path,
                backend_path: backendPath.path,
                truncated: app.truncated || backend.truncated
            )
        }.value
    }

    private static func backendLogPath() -> URL {
        URL(fileURLWithPath: NSHomeDirectory())
            .appendingPathComponent("Library/Logs/Said/backend.log")
    }

    private static func qualityFilter(_ text: String) -> String {
        var lines: [String] = []
        var repeatedWebSocketSendErrors = 0

        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            if isNoisy(line) {
                continue
            }
            if line.contains("WS send error") {
                if lines.contains(where: { $0.contains("WS send error") }) {
                    repeatedWebSocketSendErrors += 1
                    continue
                }
            }
            lines.append(line)
        }

        if repeatedWebSocketSendErrors > 0 {
            lines.append("[debug] suppressed \(repeatedWebSocketSendErrors) repeated WebSocket send error line(s)")
        }

        return lines.joined(separator: "\n")
    }

    private static func isNoisy(_ line: String) -> Bool {
        let noisyMarkers = [
            "[groq] token:",
            "[llm] token:",
            "[codex] token:",
            "[gemini_direct] token:",
            "[prefs-cache]",
            "[lexicon-cache]",
            "[embedder] GAP-4",
            "notch-view: hover:",
        ]
        return noisyMarkers.contains { line.contains($0) }
    }
}
