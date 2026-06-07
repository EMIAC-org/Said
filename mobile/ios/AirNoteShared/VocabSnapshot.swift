import Foundation

public enum VocabScope: String, Codable {
    case personal
    case org
}

public struct VocabTerm: Codable, Equatable, Identifiable {
    public var id: String { "\(scope.rawValue):\(term.lowercased())" }
    public var term: String
    public var spokenAliases: [String]
    public var termType: String
    public var scope: VocabScope
    public var priority: Double

    enum CodingKeys: String, CodingKey {
        case term
        case spokenAliases = "spoken_aliases"
        case termType = "term_type"
        case scope
        case priority
    }
}

public struct VocabSnapshot: Codable, Equatable {
    public var hash: String
    public var terms: [VocabTerm]
}
