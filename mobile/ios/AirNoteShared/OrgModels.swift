import Foundation

/// One workspace (org) the signed-in account belongs to. Mirrors the
/// control-plane `tenant::OrgMembership` serialization (GET /v1/orgs).
public struct OrgMembership: Codable, Identifiable, Equatable {
    public let id: String
    public let name: String
    public let slug: String
    public let role: String
    public let isActive: Bool

    enum CodingKeys: String, CodingKey {
        case id, name, slug, role
        case isActive = "is_active"
    }

    public init(id: String, name: String, slug: String, role: String, isActive: Bool) {
        self.id = id
        self.name = name
        self.slug = slug
        self.role = role
        self.isActive = isActive
    }
}

/// Response of GET /v1/orgs.
public struct OrgsResponse: Codable, Equatable {
    public let orgs: [OrgMembership]
    public let activeOrgID: String?
    public let personalMode: Bool

    enum CodingKeys: String, CodingKey {
        case orgs
        case activeOrgID = "active_org_id"
        case personalMode = "personal_mode"
    }
}

/// Response of POST /v1/orgs/:id/activate and POST /v1/orgs/deactivate.
public struct OrgActivateResponse: Codable, Equatable {
    public let activeOrgID: String?
    public let personalMode: Bool

    enum CodingKeys: String, CodingKey {
        case activeOrgID = "active_org_id"
        case personalMode = "personal_mode"
    }
}
