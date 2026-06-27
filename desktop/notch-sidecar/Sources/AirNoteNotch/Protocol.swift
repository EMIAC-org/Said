import Foundation

// ── Wire protocol ────────────────────────────────────────────────────────────
// Rust → Swift: one JSON object per line on stdin. snake_case keys.
// Swift → Rust: one JSON object per line on stdout. snake_case keys.
// Logs go to stderr so stdout stays a clean protocol channel.

/// A correction candidate inside a `review` message.
struct Candidate: Decodable, Equatable {
    let original: String
    let corrected: String
    let tag: String?
    let learnable: Bool?
}

/// One recent dictation for the hover-open recents panel.
struct RecentDTO: Decodable, Equatable {
    let text: String
    let ago: String?
}

/// Inbound message. A single flat struct with optional fields keyed off `type`
/// — simpler and more forgiving than a Codable enum, and tolerant of new fields.
struct InboundMessage: Decodable {
    let type: String

    // state
    let kind: String?            // "idle" | "recording" | "processing"
    // level
    let value: Double?
    // status / transcript
    let phase: String?
    let transcript: String?
    let text: String?
    let partial: Bool?
    // output
    let status: String?          // "pasted" | "manual_paste"
    let message: String?
    // error
    let runId: String?
    let audioId: String?
    let errorCode: String?
    let rawError: String?
    let diagnostic: String?
    let autoHideMs: Double?
    // vocab / learning
    let term: String?
    let original: String?
    let context: String?
    let recordingId: String?
    let wrongReplacement: String?
    let remaining: Int?
    let email: String?
    let durationS: Double?
    let candidates: [Candidate]?
    // system
    let version: String?
    let reason: String?
    let enabled: Bool?
    let recents: [RecentDTO]?
}

/// Outbound user action. Optional fields are omitted when nil.
struct OutboundAction: Encodable {
    var type: String
    var decision: String? = nil          // confirm: "learn" | "skip"
    var term: String? = nil
    var original: String? = nil
    var recordingId: String? = nil
    var variant: String? = nil           // block
    var wrongReplacement: String? = nil
    var audioId: String? = nil           // retry
    var items: [BatchItem]? = nil        // confirm_batch
    var x: Double? = nil                 // reposition
    var y: Double? = nil
    var text: String? = nil              // copy_recent
}

struct BatchItem: Encodable {
    let original: String
    let corrected: String
}

extension JSONDecoder {
    static let wire: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()
}

extension JSONEncoder {
    static let wire: JSONEncoder = {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }()
}
