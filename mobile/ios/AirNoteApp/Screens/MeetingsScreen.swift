import AirNoteShared
import SwiftUI

/// Enterprise Meetings. Calls the existing control-plane /v1/meetings endpoints
/// (list / detail / create / start / end / guest-link). Requires an active
/// workspace — meetings are org-scoped. This is management + viewing (transcript,
/// summary, tasks, decisions); live in-meeting capture stays on the desktop/web.
struct MeetingsScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var showingCreate = false

    var body: some View {
        ZStack {
            AirNoteBackground()
            if env.personalMode {
                needsWorkspace
            } else {
                content
            }
        }
        .navigationTitle("Meetings")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            if !env.personalMode {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { showingCreate = true } label: { Image(systemName: "plus") }
                }
            }
        }
        .tint(AirNoteDesign.accent)
        .task { await env.refreshMeetings() }
        .refreshable { await env.refreshMeetings() }
        .sheet(isPresented: $showingCreate) {
            CreateMeetingSheet().environmentObject(env)
        }
    }

    private var content: some View {
        ScrollView {
            VStack(spacing: 12) {
                if !env.meetingsStatus.isEmpty {
                    Text(env.meetingsStatus)
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.warning)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if env.meetingsLoading && env.meetings.isEmpty {
                    InlineLoading(text: "Loading meetings…")
                } else if env.meetings.isEmpty {
                    EmptyStateCard(
                        systemImage: "person.2.wave.2",
                        title: "No meetings yet",
                        message: "Tap + to create one for this workspace, or start one from the desktop app."
                    )
                } else {
                    ForEach(env.meetings) { meeting in
                        NavigationLink {
                            MeetingDetailView(meeting: meeting).environmentObject(env)
                        } label: {
                            MeetingRow(meeting: meeting)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 28)
        }
    }

    private var needsWorkspace: some View {
        VStack(spacing: 14) {
            EmptyStateCard(
                systemImage: "building.2",
                title: "Meetings need a workspace",
                message: "Switch to an enterprise workspace to create, view, and manage meetings."
            )
            NavigationLink(destination: WorkspaceSwitcherView()) {
                Label("Choose a workspace", systemImage: "arrow.right.circle")
            }
            .buttonStyle(AirNotePrimaryButtonStyle())
        }
        .padding(18)
    }
}

struct MeetingStatusBadge: View {
    let status: String
    var body: some View {
        let (label, tint): (String, Color) = {
            switch status {
            case "live": return ("Live", AirNoteDesign.danger)
            case "scheduled": return ("Scheduled", AirNoteDesign.accent)
            case "ended": return ("Ended", AirNoteDesign.muted)
            default: return (status.capitalized, AirNoteDesign.muted)
            }
        }()
        return Text(label)
            .font(.caption2.weight(.bold))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.14), in: Capsule())
    }
}

private struct MeetingRow: View {
    let meeting: Meeting
    var body: some View {
        AirNoteCard(padding: 14) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(meeting.title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                        .lineLimit(2)
                    Spacer(minLength: 8)
                    MeetingStatusBadge(status: meeting.status)
                }
                if let agenda = meeting.agenda, !agenda.isEmpty {
                    Text(agenda).font(.caption).foregroundStyle(AirNoteDesign.muted).lineLimit(2)
                }
                if let created = meeting.createdDate {
                    Text(created, format: .relative(presentation: .named))
                        .font(.caption2)
                        .foregroundStyle(AirNoteDesign.muted)
                }
            }
        }
    }
}

// MARK: - Detail

struct MeetingDetailView: View {
    @EnvironmentObject private var env: AppEnvironment
    let meeting: Meeting

    @State private var detail: MeetingDetail?
    @State private var loading = true
    @State private var working = false
    @State private var shareStatus = ""

    private var current: Meeting { detail?.meeting ?? meeting }

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 16) {
                    header
                    if loading && detail == nil {
                        InlineLoading(text: "Loading meeting…")
                    } else if let detail {
                        if let summary = detail.summary, !summary.isEmpty {
                            card("Summary") { Text(summary).font(.subheadline).foregroundStyle(AirNoteDesign.foreground) }
                        }
                        if !detail.tasks.isEmpty {
                            card("Action items") {
                                VStack(alignment: .leading, spacing: 8) {
                                    ForEach(detail.tasks) { task in
                                        HStack(alignment: .top, spacing: 8) {
                                            Image(systemName: "checkmark.circle").font(.caption).foregroundStyle(AirNoteDesign.accent)
                                            VStack(alignment: .leading, spacing: 1) {
                                                Text(task.title).font(.subheadline).foregroundStyle(AirNoteDesign.foreground)
                                                if let a = task.assignee, !a.isEmpty {
                                                    Text(a).font(.caption2).foregroundStyle(AirNoteDesign.muted)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if !detail.decisions.isEmpty {
                            card("Decisions") {
                                VStack(alignment: .leading, spacing: 6) {
                                    ForEach(detail.decisions) { d in
                                        Label(d.text, systemImage: "flag").font(.subheadline).foregroundStyle(AirNoteDesign.foreground)
                                    }
                                }
                            }
                        }
                        if !detail.transcript.isEmpty {
                            card("Transcript") {
                                VStack(alignment: .leading, spacing: 8) {
                                    ForEach(Array(detail.transcript.enumerated()), id: \.offset) { _, chunk in
                                        VStack(alignment: .leading, spacing: 1) {
                                            if let s = chunk.speakerName, !s.isEmpty {
                                                Text(s).font(.caption2.weight(.semibold)).foregroundStyle(AirNoteDesign.accent)
                                            }
                                            Text(chunk.text).font(.subheadline).foregroundStyle(AirNoteDesign.foreground)
                                        }
                                    }
                                }
                            }
                        }
                        if !detail.participants.isEmpty {
                            card("Participants") {
                                VStack(alignment: .leading, spacing: 6) {
                                    ForEach(detail.participants) { p in
                                        HStack {
                                            Text(p.name ?? "Member").font(.subheadline).foregroundStyle(AirNoteDesign.foreground)
                                            Spacer()
                                            if let st = p.status { Text(st.capitalized).font(.caption2).foregroundStyle(AirNoteDesign.muted) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 14)
            }
        }
        .navigationTitle(current.title)
        .navigationBarTitleDisplayMode(.inline)
        .tint(AirNoteDesign.accent)
        .task { await load() }
    }

    private var header: some View {
        AirNoteCard(padding: 16) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    MeetingStatusBadge(status: current.status)
                    Spacer()
                    if let created = current.createdDate {
                        Text(created, format: .relative(presentation: .named)).font(.caption2).foregroundStyle(AirNoteDesign.muted)
                    }
                }
                if let agenda = current.agenda, !agenda.isEmpty {
                    Text(agenda).font(.subheadline).foregroundStyle(AirNoteDesign.muted)
                }
                HStack(spacing: 10) {
                    if current.isScheduled {
                        Button { Task { working = true; await env.startMeeting(current.id); await load(); working = false } } label: {
                            Label("Start", systemImage: "play.fill")
                        }.buttonStyle(AirNotePrimaryButtonStyle()).disabled(working)
                    }
                    if current.isLive {
                        Button(role: .destructive) { Task { working = true; await env.endMeeting(current.id); await load(); working = false } } label: {
                            Label("End", systemImage: "stop.fill")
                        }.buttonStyle(.bordered).tint(AirNoteDesign.danger).disabled(working)
                    }
                    Button { Task { await share() } } label: {
                        Label("Guest link", systemImage: "link")
                    }.buttonStyle(.bordered).disabled(working || current.isEnded)
                }
                if !shareStatus.isEmpty {
                    Text(shareStatus).font(.caption).foregroundStyle(AirNoteDesign.success)
                }
            }
        }
    }

    private func card<Content: View>(_ title: String, @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeader(title)
            AirNoteCard(padding: 14) { content().frame(maxWidth: .infinity, alignment: .leading) }
        }
    }

    private func load() async {
        loading = true
        defer { loading = false }
        detail = await env.meetingDetail(meeting.id)
    }

    private func share() async {
        working = true
        defer { working = false }
        if let url = await env.meetingShareURL(meeting.id) {
            UIPasteboard.general.string = url.absoluteString
            shareStatus = "Guest link copied to clipboard."
        } else {
            shareStatus = "Couldn't create a guest link."
        }
    }
}

// MARK: - Create

private struct CreateMeetingSheet: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.dismiss) private var dismiss

    @State private var title = ""
    @State private var members: [OrgMember] = []
    @State private var selected = Set<String>()
    @State private var loadingMembers = true
    @State private var creating = false
    @State private var error = ""

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                Form {
                    Section("Title") {
                        TextField("e.g. Weekly sync", text: $title)
                            .textFieldStyle(AirNoteFieldStyle())
                    }
                    Section {
                        if loadingMembers {
                            InlineLoading(text: "Loading workspace members…")
                        } else if members.isEmpty {
                            Text("No other members in this workspace.")
                                .font(.caption).foregroundStyle(AirNoteDesign.muted)
                        } else {
                            ForEach(members) { m in
                                Button {
                                    if selected.contains(m.accountId) { selected.remove(m.accountId) } else { selected.insert(m.accountId) }
                                } label: {
                                    HStack {
                                        VStack(alignment: .leading, spacing: 1) {
                                            Text(m.displayName).foregroundStyle(AirNoteDesign.foreground)
                                            Text(m.role.capitalized).font(.caption2).foregroundStyle(AirNoteDesign.muted)
                                        }
                                        Spacer()
                                        if selected.contains(m.accountId) {
                                            Image(systemName: "checkmark.circle.fill").foregroundStyle(AirNoteDesign.success)
                                        }
                                    }
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    } header: {
                        Text("Participants")
                    } footer: {
                        Text("You're added automatically. Pick at least one other participant.")
                    }
                    if !error.isEmpty {
                        Section { Text(error).font(.caption).foregroundStyle(AirNoteDesign.danger) }
                    }
                }
                .scrollContentBackground(.hidden)
            }
            .navigationTitle("New meeting")
            .navigationBarTitleDisplayMode(.inline)
            .tint(AirNoteDesign.accent)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(creating ? "Creating…" : "Create") { Task { await create() } }
                        .disabled(creating || title.trimmingCharacters(in: .whitespaces).isEmpty || selected.isEmpty)
                }
            }
            .task {
                loadingMembers = true
                members = (await env.orgMembers()).filter { $0.accountId != (env.account?.id ?? "") }
                loadingMembers = false
            }
        }
    }

    private func create() async {
        creating = true
        defer { creating = false }
        let made = await env.createMeeting(title: title, participantIDs: Array(selected))
        if made != nil { dismiss() } else { error = env.meetingsStatus.isEmpty ? "Couldn't create the meeting." : env.meetingsStatus }
    }
}
