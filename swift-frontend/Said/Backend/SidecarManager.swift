import Foundation
import os

final class SidecarManager: ObservableObject {
    @Published var isHealthy = false
    @Published var port: Int = 48484

    private var process: Process?
    private var healthTimer: Timer?
    private let logger = Logger(subsystem: "com.emiac.said", category: "sidecar")
    private let sharedSecret: String

    var baseURL: String { "http://127.0.0.1:\(port)" }

    init() {
        sharedSecret = UUID().uuidString
    }

    func start() {
        guard process == nil else { return }
        guard let binary = findBinary() else {
            logger.error("said-backend binary not found")
            return
        }

        let proc = Process()
        proc.executableURL = binary
        proc.arguments = ["--port", "\(port)"]
        proc.environment = ProcessInfo.processInfo.environment.merging([
            "POLISH_SHARED_SECRET": sharedSecret,
            "RUST_LOG": "info",
        ]) { _, new in new }

        if let envPath = Bundle.main.url(forResource: ".env", withExtension: nil, subdirectory: nil) ??
            Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent(".env"),
           FileManager.default.fileExists(atPath: envPath.path) {
            let lines = (try? String(contentsOf: envPath, encoding: .utf8))?.split(separator: "\n") ?? []
            for line in lines {
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                if trimmed.isEmpty || trimmed.hasPrefix("#") { continue }
                let parts = trimmed.split(separator: "=", maxSplits: 1)
                if parts.count == 2 {
                    proc.environment?[String(parts[0])] = String(parts[1])
                }
            }
        }

        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [logger] handle in
            let data = handle.availableData
            if !data.isEmpty, let line = String(data: data, encoding: .utf8) {
                logger.info("[said-backend] \(line.trimmingCharacters(in: .whitespacesAndNewlines))")
            }
        }

        do {
            try proc.run()
            process = proc
            logger.info("said-backend started (pid=\(proc.processIdentifier), port=\(self.port))")
            startHealthPolling()
        } catch {
            logger.error("failed to start said-backend: \(error)")
        }
    }

    func stop() {
        healthTimer?.invalidate()
        healthTimer = nil
        if let proc = process, proc.isRunning {
            proc.terminate()
            logger.info("said-backend terminated")
        }
        process = nil
        isHealthy = false
    }

    var authHeader: String { "Bearer \(sharedSecret)" }

    private func findBinary() -> URL? {
        if let bundled = Bundle.main.executableURL?
            .deletingLastPathComponent()
            .appendingPathComponent("said-backend"),
           FileManager.default.isExecutableFile(atPath: bundled.path) {
            return bundled
        }

        // #filePath = .../lahore/swift-frontend/Said/Backend/SidecarManager.swift
        // Walk up 5 levels to reach lahore/ (the repo root)
        let sourceRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // Backend/
            .deletingLastPathComponent() // Said/
            .deletingLastPathComponent() // swift-frontend/
            .deletingLastPathComponent() // lahore/ (repo root)

        let searchRoots = [
            sourceRoot,
            sourceRoot.deletingLastPathComponent(), // one more level up just in case
            URL(fileURLWithPath: FileManager.default.currentDirectoryPath),
            URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("conductor/workspaces/said/lahore"),
        ]

        let relativePaths = [
            "target/release/said-backend",
            "target/debug/said-backend",
            "target/aarch64-apple-darwin/release/said-backend",
            "target/aarch64-apple-darwin/debug/said-backend",
        ]

        for root in searchRoots {
            for rel in relativePaths {
                let url = root.appendingPathComponent(rel)
                if FileManager.default.isExecutableFile(atPath: url.path) {
                    logger.info("found said-backend at \(url.path)")
                    return url
                }
            }
        }

        let whichResult = Process()
        whichResult.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        whichResult.arguments = ["said-backend"]
        let pipe = Pipe()
        whichResult.standardOutput = pipe
        try? whichResult.run()
        whichResult.waitUntilExit()
        let path = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if let path, !path.isEmpty, FileManager.default.isExecutableFile(atPath: path) {
            return URL(fileURLWithPath: path)
        }

        return nil
    }

    private func startHealthPolling() {
        healthTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.checkHealth()
        }
    }

    private func checkHealth() {
        guard let url = URL(string: "\(baseURL)/v1/health") else { return }
        var req = URLRequest(url: url)
        req.timeoutInterval = 2
        URLSession.shared.dataTask(with: req) { [weak self] data, response, _ in
            let ok = (response as? HTTPURLResponse)?.statusCode == 200
            DispatchQueue.main.async {
                if self?.isHealthy != ok {
                    self?.isHealthy = ok
                    if ok {
                        self?.logger.info("said-backend healthy")
                        self?.healthTimer?.invalidate()
                        self?.healthTimer = Timer.scheduledTimer(withTimeInterval: 10.0, repeats: true) { [weak self] _ in
                            self?.checkHealth()
                        }
                    }
                }
            }
        }.resume()
    }

    deinit {
        stop()
    }
}
