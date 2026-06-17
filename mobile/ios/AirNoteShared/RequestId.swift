import Foundation

public enum RequestId {
    public static func make() -> String {
        UUID().uuidString
    }
}
