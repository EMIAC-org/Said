import Foundation

public enum AppGroupFile: String, CaseIterable {
    case session = "bridge/session.json"
    case command = "bridge/command.json"
    case result = "bridge/result.json"
    case ack = "bridge/ack.json"
    case health = "bridge/health.json"

    public var relativePath: String { rawValue }
}
