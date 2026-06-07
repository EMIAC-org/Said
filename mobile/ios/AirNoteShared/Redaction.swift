import Foundation

public enum Redaction {
    public static func safeHostAppLabel(_ value: String?) -> String? {
        guard let value else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return String(trimmed.prefix(120))
    }

    public static func containsSensitiveKey(_ value: String) -> Bool {
        let lowered = value.lowercased()
        return ["token", "authorization", "api_key", "secret", "password"].contains { lowered.contains($0) }
    }
}
