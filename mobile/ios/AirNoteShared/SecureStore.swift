import Foundation
import Security

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

public final class KeychainSecureStore: SecureStore {
    private let service: String
    private let accessGroup: String?

    public init(service: String = "com.emiac.airnote.mobile", accessGroup: String? = nil) {
        self.service = service
        self.accessGroup = accessGroup
    }

    public func read(_ key: String) throws -> Data? {
        var query = baseQuery(key)
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        query[kSecReturnData as String] = true

        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw KeychainError(status: status)
        }
        return result as? Data
    }

    public func write(_ data: Data, for key: String) throws {
        var query = baseQuery(key)
        query[kSecValueData as String] = data
        query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly

        let status = SecItemAdd(query as CFDictionary, nil)
        if status == errSecDuplicateItem {
            var update = [String: Any]()
            update[kSecValueData as String] = data
            update[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            let updateStatus = SecItemUpdate(baseQuery(key) as CFDictionary, update as CFDictionary)
            guard updateStatus == errSecSuccess else {
                throw KeychainError(status: updateStatus)
            }
            return
        }
        guard status == errSecSuccess else {
            throw KeychainError(status: status)
        }
    }

    public func delete(_ key: String) throws {
        let status = SecItemDelete(baseQuery(key) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status)
        }
    }

    private func baseQuery(_ key: String) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key
        ]
        if let accessGroup {
            query[kSecAttrAccessGroup as String] = accessGroup
        }
        return query
    }
}

public struct KeychainError: Error, Equatable {
    public var status: OSStatus

    public init(status: OSStatus) {
        self.status = status
    }
}
