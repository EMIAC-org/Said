import AirNoteShared
import SwiftUI

/// Manage a workspace's members — view everyone, and (for admins) add a member
/// by email or change a member's role. Backed by the additive control-plane
/// routes POST/PATCH /v1/orgs/:org_id/members[/:account_id]. Non-admins see a
/// read-only list.
struct WorkspaceMembersView: View {
    @EnvironmentObject private var env: AppEnvironment
    let orgID: String
    let isAdmin: Bool

    @State private var members: [OrgMember] = []
    @State private var loading = true
    @State private var working = false
    @State private var errorMessage: String?
    @State private var showAddSheet = false

    static let roles = ["MEMBER", "MANAGER", "COMPANY_ADMIN"]
    static func roleLabel(_ role: String) -> String {
        switch role.uppercased() {
        case "COMPANY_ADMIN": return "Admin"
        case "MANAGER": return "Manager"
        default: return "Member"
        }
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            Form {
                if let errorMessage {
                    Section {
                        Text(errorMessage).font(.caption).foregroundStyle(AirNoteDesign.warning)
                    }
                }
                Section {
                    if loading {
                        HStack { Spacer(); ProgressView().tint(AirNoteDesign.accent); Spacer() }
                    } else if members.isEmpty {
                        Text("No members yet.").font(.caption).foregroundStyle(AirNoteDesign.muted)
                    } else {
                        ForEach(members) { member in memberRow(member) }
                    }
                } header: {
                    Text("\(members.count) member\(members.count == 1 ? "" : "s")")
                } footer: {
                    Text(isAdmin
                         ? "Add teammates by the email on their AirNote account — they must have signed up first."
                         : "Only workspace admins can add members or change roles.")
                }
            }
            .scrollContentBackground(.hidden)
            .disabled(working)
        }
        .navigationTitle("Members")
        .navigationBarTitleDisplayMode(.inline)
        .tint(AirNoteDesign.accent)
        .toolbar {
            if isAdmin {
                ToolbarItem(placement: .primaryAction) {
                    Button { showAddSheet = true } label: {
                        Image(systemName: "person.badge.plus")
                    }
                    .disabled(working)
                }
            }
        }
        .sheet(isPresented: $showAddSheet) {
            AddMemberSheet(orgID: orgID) { await reload() }
                .environmentObject(env)
        }
        .task { await reload() }
    }

    @ViewBuilder private func memberRow(_ member: OrgMember) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(member.larkName ?? member.email)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text(member.email)
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
            }
            Spacer(minLength: 0)
            if isAdmin {
                Menu {
                    ForEach(Self.roles, id: \.self) { role in
                        Button {
                            Task { await changeRole(member, to: role) }
                        } label: {
                            if member.role.uppercased() == role {
                                Label(Self.roleLabel(role), systemImage: "checkmark")
                            } else {
                                Text(Self.roleLabel(role))
                            }
                        }
                    }
                } label: {
                    Text(Self.roleLabel(member.role))
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.accent)
                }
            } else {
                Text(Self.roleLabel(member.role))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.muted)
            }
        }
    }

    private func reload() async {
        errorMessage = nil
        loading = true
        defer { loading = false }
        do {
            members = try await env.gateway.listOrgMembers(orgID: orgID)
        } catch {
            errorMessage = (error as? GatewayError)?.userMessage ?? "Couldn't load members."
        }
    }

    private func changeRole(_ member: OrgMember, to role: String) async {
        guard member.role.uppercased() != role else { return }
        errorMessage = nil
        working = true
        defer { working = false }
        do {
            _ = try await env.gateway.setMemberRole(orgID: orgID, accountID: member.accountId, role: role)
            await reload()
        } catch {
            errorMessage = (error as? GatewayError)?.userMessage ?? "Couldn't change role."
        }
    }
}

/// Admin-only sheet to add an existing AirNote account to the workspace by email.
private struct AddMemberSheet: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.dismiss) private var dismiss
    let orgID: String
    let onAdded: () async -> Void

    @State private var email = ""
    @State private var role = "MEMBER"
    @State private var working = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                Form {
                    Section {
                        TextField("teammate@company.com", text: $email)
                            .textInputAutocapitalization(.never)
                            .keyboardType(.emailAddress)
                            .autocorrectionDisabled()
                        Picker("Role", selection: $role) {
                            Text("Member").tag("MEMBER")
                            Text("Manager").tag("MANAGER")
                            Text("Admin").tag("COMPANY_ADMIN")
                        }
                    } footer: {
                        if let errorMessage {
                            Text(errorMessage).foregroundStyle(AirNoteDesign.warning)
                        } else {
                            Text("They must already have an AirNote account with this email.")
                        }
                    }
                }
                .scrollContentBackground(.hidden)
                .disabled(working)
            }
            .navigationTitle("Add member")
            .navigationBarTitleDisplayMode(.inline)
            .tint(AirNoteDesign.accent)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { Task { await add() } }
                        .disabled(working || email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }

    private func add() async {
        errorMessage = nil
        working = true
        defer { working = false }
        do {
            _ = try await env.gateway.addOrgMember(
                orgID: orgID,
                email: email.trimmingCharacters(in: .whitespacesAndNewlines),
                role: role
            )
            await onAdded()
            dismiss()
        } catch {
            errorMessage = (error as? GatewayError)?.userMessage ?? "Couldn't add member."
        }
    }
}
