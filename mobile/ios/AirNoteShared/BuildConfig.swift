import Foundation

public enum BuildConfig {
    public static let appGroupIdentifier = "group.com.emiac.airnote"
    public static let mainBundleIdentifier = "com.emiac.airnote.ios"
    public static let keyboardBundleIdentifier = "com.emiac.airnote.ios.keyboard"
    public static let defaultGatewayBaseURL = URL(string: "https://airnote.emiactech.com")!
    public static let maxRecordingSeconds = 60

    public static var gatewayBaseURL: URL {
        guard
            let raw = Bundle.main.object(forInfoDictionaryKey: "AIRNOTE_GATEWAY_BASE_URL") as? String,
            let url = URL(string: raw),
            !raw.isEmpty
        else {
            return defaultGatewayBaseURL
        }
        return url
    }

    public static var useMockGateway: Bool {
        guard let raw = Bundle.main.object(forInfoDictionaryKey: "AIRNOTE_USE_MOCK_GATEWAY") as? String else {
            return true
        }
        return ["1", "true", "yes", "YES"].contains(raw)
    }
}
