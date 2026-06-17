import Foundation

public enum BuildConfig {
    public static let appGroupIdentifier = "group.com.emiac.airnote"
    public static let mainBundleIdentifier = "com.emiac.airnote.ios"
    public static let keyboardBundleIdentifier = "com.emiac.airnote.ios.keyboard"
    public static let defaultGatewayBaseURL = URL(string: "https://airnote.emiactech.com")!
    public static let maxRecordingSeconds = 60

    public static var gatewayBaseURL: URL {
        // 1) Runtime override (enterprise / self-hosted / local testing), set in
        //    Settings and persisted in the App Group so the app AND the keyboard
        //    extension both talk to the chosen server.
        if let custom = SharedStore.customGatewayURL,
           let url = URL(string: custom), url.scheme != nil, url.host != nil {
            return url
        }
        // 2) Build-time override (Info.plist), else the built-in default.
        guard
            let raw = Bundle.main.object(forInfoDictionaryKey: "AIRNOTE_GATEWAY_BASE_URL") as? String,
            let url = URL(string: raw),
            !raw.isEmpty
        else {
            return defaultGatewayBaseURL
        }
        return url
    }
}
