import Foundation
import Combine

public struct DictationRecord: Identifiable, Codable, Equatable {
    public var id: String
    public var transcript: String
    public var polished: String
    public var createdAt: Date
    public var outcome: TerminalOutcome

    public init(id: String = UUID().uuidString, transcript: String, polished: String, createdAt: Date = Date(), outcome: TerminalOutcome) {
        self.id = id
        self.transcript = transcript
        self.polished = polished
        self.createdAt = createdAt
        self.outcome = outcome
    }
}

public final class DictationStore: ObservableObject {
    @Published public private(set) var records: [DictationRecord] = []

    public init(records: [DictationRecord] = []) {
        self.records = records
    }

    public func append(_ record: DictationRecord) {
        records.insert(record, at: 0)
    }

    public func clear() {
        records.removeAll()
    }
}
