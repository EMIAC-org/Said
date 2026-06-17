import Foundation

public enum AppGroupBridgeError: Error, Equatable {
    case missingContainer(String)
    case missingFile(AppGroupFile)
}

public final class AppGroupBridge {
    public let containerURL: URL
    private let fileManager: FileManager
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder

    public init(appGroupIdentifier: String = BuildConfig.appGroupIdentifier, fileManager: FileManager = .default) throws {
        guard let containerURL = fileManager.containerURL(forSecurityApplicationGroupIdentifier: appGroupIdentifier) else {
            throw AppGroupBridgeError.missingContainer(appGroupIdentifier)
        }
        self.containerURL = containerURL
        self.fileManager = fileManager
        self.encoder = JSONEncoder()
        self.decoder = JSONDecoder()
        self.encoder.dateEncodingStrategy = .iso8601
        self.decoder.dateDecodingStrategy = .iso8601
    }

    public func fileURL(_ file: AppGroupFile) -> URL {
        containerURL.appendingPathComponent(file.relativePath)
    }

    public func write<T: Encodable>(_ value: T, to file: AppGroupFile) throws {
        let url = fileURL(file)
        try fileManager.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try encoder.encode(value)
        try data.write(to: url, options: [.atomic])
    }

    public func read<T: Decodable>(_ type: T.Type, from file: AppGroupFile) throws -> T {
        let url = fileURL(file)
        guard fileManager.fileExists(atPath: url.path) else {
            throw AppGroupBridgeError.missingFile(file)
        }
        let data = try Data(contentsOf: url)
        return try decoder.decode(type, from: data)
    }

    public func clear(_ file: AppGroupFile) throws {
        let url = fileURL(file)
        if fileManager.fileExists(atPath: url.path) {
            try fileManager.removeItem(at: url)
        }
    }

    public func clearAllBridgeFiles() throws {
        for file in AppGroupFile.allCases {
            try clear(file)
        }
    }
}
