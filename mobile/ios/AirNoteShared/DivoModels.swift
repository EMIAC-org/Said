import Foundation

/// One Divo chat thread (history list item). Server wraps these in `{ data: { threads: [...] } }`.
public struct DivoThreadSummary: Codable, Identifiable, Equatable {
    public let id: String
    public let title: String?
    public let preview: String?

    enum CodingKeys: String, CodingKey { case id, title, preview }

    public var displayTitle: String { title?.isEmpty == false ? title! : "Untitled" }
}

/// One message in a Divo thread.
public struct DivoMessage: Codable, Identifiable, Equatable {
    public let id: String
    public let role: String     // "user" | "assistant"
    public let content: String

    enum CodingKeys: String, CodingKey { case id, role, content }

    public init(id: String, role: String, content: String) {
        self.id = id
        self.role = role
        self.content = content
    }

    public var isUser: Bool { role == "user" }
}

/// A full Divo thread with messages. Server wraps it in `{ data: {...} }`.
public struct DivoThread: Codable, Equatable {
    public let id: String
    public let title: String?
    public let messages: [DivoMessage]

    public init(id: String, title: String?, messages: [DivoMessage]) {
        self.id = id
        self.title = title
        self.messages = messages
    }
}

/// Final result of a Divo chat turn (after the SSE `done` frame).
public struct DivoChatResult: Equatable {
    public let content: String
    public let threadID: String?

    public init(content: String, threadID: String?) {
        self.content = content
        self.threadID = threadID
    }
}
