import Foundation

public protocol SecureStore {
    func read(_ key: String) throws -> Data?
    func write(_ data: Data, for key: String) throws
    func delete(_ key: String) throws
}

public final class InMemorySecureStore: SecureStore {
    private var values: [String: Data] = [:]

    public init() {}

    public func read(_ key: String) throws -> Data? {
        values[key]
    }

    public func write(_ data: Data, for key: String) throws {
        values[key] = data
    }

    public func delete(_ key: String) throws {
        values.removeValue(forKey: key)
    }
}
