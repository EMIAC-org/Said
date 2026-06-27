import AirNoteShared
import SwiftUI

/// Switch the active workspace (org), mirroring the desktop's workspace switcher
/// (enterprise.ts activateWorkspace / deactivateWorkspace). Meetings and Divo
/// require an active workspace; personal mode uses your own account runtime.
struct WorkspaceSwitcherView: View {
    @EnvironmentObject private var env: AppEnvironment

    var body: some View {
        ZStack {
            AirNoteBackground()
            Form {
                Section {
                    row(title: "Personal",
                        subtitle: "Your own account — dictation, history, vocabulary",
                        active: env.personalMode) {
                        Task { await env.usePersonalMode() }
                    }
                } header: {
                    Text("Mode")
                } footer: {
                    Text("Meetings and Divo require an active workspace. Personal mode uses your own account runtime.")
                }

                Section {
                    if env.orgs.isEmpty {
                        Text("You're not a member of any workspace yet. Ask an admin to add you, or connect via your enterprise server.")
                            .font(.caption)
                            .foregroundStyle(AirNoteDesign.muted)
                    } else {
                        ForEach(env.orgs) { org in
                            row(title: org.name,
                                subtitle: org.role.capitalized,
                                active: !env.personalMode && env.activeOrgID == org.id) {
                                Task { await env.activateWorkspace(org.id) }
                            }
                        }
                    }
                } header: {
                    Text("Workspaces")
                }

                if !env.personalMode, let org = env.activeOrg {
                    Section {
                        NavigationLink {
                            WorkspaceMembersView(orgID: org.id, isAdmin: Self.isAdmin(org.role))
                        } label: {
                            Label("Members", systemImage: "person.2")
                        }
                    } header: {
                        Text("Manage")
                    } footer: {
                        Text("See who's in this workspace. Admins can add teammates and change roles.")
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .disabled(env.workspaceWorking)
        }
        .navigationTitle("Workspace")
        .navigationBarTitleDisplayMode(.inline)
        .tint(AirNoteDesign.accent)
        .task { await env.refreshOrgs() }
        .overlay {
            if env.workspaceWorking {
                ProgressView().controlSize(.large).tint(AirNoteDesign.accent)
            }
        }
    }

    static func isAdmin(_ role: String) -> Bool {
        ["admin", "company_admin", "manager"].contains(role.lowercased())
    }

    private func row(title: String, subtitle: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                }
                Spacer(minLength: 0)
                if active {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(AirNoteDesign.success)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(active)
    }
}
