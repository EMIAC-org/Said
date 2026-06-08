import Foundation

/// Typed gateway failures so the UI can react precisely:
/// - `.unauthorized` → token expired/invalid → sign the user out and re-auth.
/// - `.credentialMissing` → the server has no Deepgram/Groq credential provisioned
///   yet → show a calm "dictation is being set up" state instead of a hard error.
public enum GatewayError: Error, Equatable {
    case unauthorized
    case credentialMissing
    case rateLimited
    case server(status: Int, code: String?, message: String?)
    case network(String)
    case invalidResponse

    public var isUnauthorized: Bool {
        if case .unauthorized = self { return true }
        return false
    }

    public var isCredentialMissing: Bool {
        if case .credentialMissing = self { return true }
        return false
    }

    /// A short, user-facing message suitable for inline display.
    public var userMessage: String {
        switch self {
        case .unauthorized:
            return "Your session expired. Please sign in again."
        case .credentialMissing:
            return "Dictation isn't available on this workspace yet."
        case .rateLimited:
            return "You've hit today's limit. Try again later."
        case let .server(_, _, message):
            return message ?? "Something went wrong on the server."
        case let .network(message):
            return message.isEmpty ? "No internet connection." : message
        case .invalidResponse:
            return "Unexpected response from the server."
        }
    }

    /// Server error codes that indicate provider credentials are not provisioned.
    public static let credentialMissingCodes: Set<String> = [
        "deepgram_credential_missing",
        "groq_credential_missing",
        "credential_missing",
        "provider_credential_missing",
    ]

    /// Build a `GatewayError` from an HTTP status + optional decoded error body.
    public static func from(status: Int, code: String?, message: String?) -> GatewayError {
        if status == 401 || status == 403 {
            return .unauthorized
        }
        if status == 429 {
            return .rateLimited
        }
        if let code, credentialMissingCodes.contains(code) {
            return .credentialMissing
        }
        if let message, credentialMissingCodes.contains(where: { message.lowercased().contains($0) }) {
            return .credentialMissing
        }
        return .server(status: status, code: code, message: message)
    }
}
