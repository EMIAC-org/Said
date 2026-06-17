import Foundation

/// Lenient ISO8601 parsing for server timestamps (Rust chrono may include
/// fractional seconds, which Foundation's default .iso8601 rejects). Timestamps
/// are decoded as String to never break a list/detail decode, then parsed here
/// for display.
public enum MeetingDate {
    private static let withFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private static let plain: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    public static func parse(_ s: String?) -> Date? {
        guard let s, !s.isEmpty else { return nil }
        return withFraction.date(from: s) ?? plain.date(from: s)
    }
}

/// A meeting (list item / detail header). Mirrors control-plane meetings.rs.
/// Timestamps are Strings (see MeetingDate) so a partial start/end response or a
/// fractional-second timestamp can never break decoding.
public struct Meeting: Codable, Identifiable, Equatable {
    public let id: String
    public let title: String
    public let agenda: String?
    public let status: String          // scheduled | live | ended
    public let createdBy: String?
    public let startedAt: String?
    public let endedAt: String?
    public let createdAt: String?
    public let scheduledAt: String?
    public let durationMinutes: Int?

    enum CodingKeys: String, CodingKey {
        case id, title, agenda, status
        case createdBy = "created_by"
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case createdAt = "created_at"
        case scheduledAt = "scheduled_at"
        case durationMinutes = "duration_minutes"
    }

    public var isLive: Bool { status == "live" }
    public var isScheduled: Bool { status == "scheduled" }
    public var isEnded: Bool { status == "ended" }
    public var createdDate: Date? { MeetingDate.parse(createdAt) }
}

public struct MeetingParticipant: Codable, Identifiable, Equatable {
    public let id: String
    public let accountId: String?
    public let status: String?
    public let name: String?

    enum CodingKeys: String, CodingKey {
        case id, status, name
        case accountId = "account_id"
    }
}

public struct MeetingTask: Codable, Identifiable, Equatable {
    public let id: String
    public let title: String
    public let assignee: String?
    public let status: String?
}

public struct MeetingDecision: Codable, Identifiable, Equatable {
    public let id: String
    public let text: String
}

public struct MeetingTranscriptChunk: Codable, Equatable {
    public let speakerId: String?
    public let speakerName: String?
    public let text: String
    public let chunkIndex: Int

    enum CodingKeys: String, CodingKey {
        case text
        case speakerId = "speaker_id"
        case speakerName = "speaker_name"
        case chunkIndex = "chunk_index"
    }
}

/// GET /v1/meetings/:id
public struct MeetingDetail: Codable, Equatable {
    public let meeting: Meeting
    public let participants: [MeetingParticipant]
    public let summary: String?
    public let tasks: [MeetingTask]
    public let decisions: [MeetingDecision]
    public let transcript: [MeetingTranscriptChunk]
}

/// GET /v1/meetings
public struct MeetingsListResponse: Codable, Equatable {
    public let meetings: [Meeting]
}

/// POST /v1/meetings (create) — full meeting under `meeting`.
public struct MeetingResponse: Codable, Equatable {
    public let meeting: Meeting
}

/// POST /v1/meetings/:id/guest-link — tolerant of either field name; we build the
/// share URL from `token` regardless.
public struct GuestLinkResponse: Codable, Equatable {
    public let token: String
    public let inviteURL: String?
    public let guestLink: String?
    public let expiresAt: String?

    enum CodingKeys: String, CodingKey {
        case token
        case inviteURL = "invite_url"
        case guestLink = "guest_link"
        case expiresAt = "expires_at"
    }
}

/// GET /v1/orgs/:org_id/members (participant picker for creating a meeting).
public struct OrgMember: Codable, Identifiable, Equatable {
    public let id: String
    public let accountId: String
    public let email: String
    public let role: String
    public let larkName: String?

    enum CodingKeys: String, CodingKey {
        case id, email, role
        case accountId = "account_id"
        case larkName = "lark_name"
    }

    public var displayName: String { larkName ?? email }
}

public struct OrgMembersResponse: Codable, Equatable {
    public let members: [OrgMember]
}
