import Foundation
import AppKit

/// stdin/stdout JSON-lines bridge to the Tauri (Rust) host.
/// - Reads inbound messages on a background thread, dispatches to main.
/// - Writes outbound actions on a serial queue.
/// - Logs to stderr so stdout stays a clean protocol channel.
/// - EOF on stdin (parent gone) terminates the app.
final class Bridge {
    private let onMessage: (InboundMessage) -> Void
    private let out = FileHandle.standardOutput
    private let writeQueue = DispatchQueue(label: "notch.bridge.write")

    init(onMessage: @escaping (InboundMessage) -> Void) {
        self.onMessage = onMessage
    }

    func start() {
        let input = FileHandle.standardInput
        Thread.detachNewThread { [weak self] in
            guard let self else { return }
            var buffer = Data()
            while true {
                let chunk = input.availableData
                if chunk.isEmpty {
                    // EOF: the Tauri host exited. Tear down on the main thread.
                    DispatchQueue.main.async { NSApplication.shared.terminate(nil) }
                    return
                }
                buffer.append(chunk)
                while let nl = buffer.firstIndex(of: 0x0A) {
                    let line = buffer.subdata(in: buffer.startIndex..<nl)
                    buffer.removeSubrange(buffer.startIndex...nl)
                    guard !line.isEmpty else { continue }
                    do {
                        let msg = try JSONDecoder.wire.decode(InboundMessage.self, from: line)
                        DispatchQueue.main.async { self.onMessage(msg) }
                    } catch {
                        self.log("decode failed: \(String(data: line, encoding: .utf8) ?? "<bin>") — \(error)")
                    }
                }
            }
        }
    }

    func send(_ action: OutboundAction) {
        writeQueue.async { [weak self] in
            guard let self else { return }
            guard var data = try? JSONEncoder.wire.encode(action) else { return }
            data.append(0x0A)
            do {
                try self.out.write(contentsOf: data)
            } catch {
                self.log("write failed: \(error)")
            }
        }
    }

    func sendReady() { send(OutboundAction(type: "ready")) }

    func log(_ s: String) {
        FileHandle.standardError.write(Data("[notch] \(s)\n".utf8))
    }
}
