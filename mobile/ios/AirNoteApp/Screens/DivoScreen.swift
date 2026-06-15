import AirNoteShared
import SwiftUI

/// Divo — enterprise AI chat. Calls /v1/divo/{chat,threads,threads/:id}. The
/// server gates this to approved EMIAC accounts (and needs Lark sign-in + an
/// active workspace); other accounts see a clear gate message.
struct DivoScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var draft = ""
    @State private var showingHistory = false
    @FocusState private var inputFocused: Bool

    var body: some View {
        ZStack {
            AirNoteBackground()
            VStack(spacing: 0) {
                messages
                inputBar
            }
        }
        .navigationTitle("Divo")
        .navigationBarTitleDisplayMode(.inline)
        .tint(AirNoteDesign.accent)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { env.newDivoThread() } label: { Image(systemName: "square.and.pencil") }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { showingHistory = true } label: { Image(systemName: "clock.arrow.circlepath") }
            }
        }
        .task { await env.refreshDivoThreads() }
        .sheet(isPresented: $showingHistory) {
            DivoHistorySheet().environmentObject(env)
        }
    }

    private var messages: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    if env.divoMessages.isEmpty && !env.divoSending {
                        EmptyStateCard(
                            systemImage: "sparkles",
                            title: "Ask Divo",
                            message: "Divo can search and act across your workspace tools. Type a request below."
                        )
                        .padding(.top, 20)
                    }
                    ForEach(env.divoMessages) { m in
                        DivoBubble(message: m).id(m.id)
                    }
                    if env.divoSending {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("Divo is thinking…").font(.caption).foregroundStyle(AirNoteDesign.muted)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .id("thinking")
                    }
                    if !env.divoStatus.isEmpty {
                        Text(env.divoStatus).font(.caption).foregroundStyle(AirNoteDesign.warning)
                    }
                }
                .padding(16)
            }
            .onChange(of: env.divoMessages.count) { _, _ in
                if let last = env.divoMessages.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
            }
        }
    }

    private var inputBar: some View {
        HStack(spacing: 10) {
            TextField("Message Divo", text: $draft, axis: .vertical)
                .textFieldStyle(AirNoteFieldStyle())
                .focused($inputFocused)
                .lineLimit(1...4)
            Button {
                let text = draft
                draft = ""
                inputFocused = false
                Task { await env.sendDivo(text) }
            } label: {
                Image(systemName: "arrow.up.circle.fill").font(.title2)
            }
            .disabled(env.divoSending || draft.trimmingCharacters(in: .whitespaces).isEmpty)
            .foregroundStyle(AirNoteDesign.accent)
        }
        .padding(12)
        .background(.ultraThinMaterial)
    }
}

private struct DivoBubble: View {
    let message: DivoMessage
    var body: some View {
        HStack {
            if message.isUser { Spacer(minLength: 40) }
            Text(message.content)
                .font(.subheadline)
                .foregroundStyle(message.isUser ? AirNoteDesign.ink : AirNoteDesign.foreground)
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
                .background(
                    message.isUser ? AirNoteDesign.accent : AirNoteDesign.surfaceRaised.opacity(0.55),
                    in: RoundedRectangle(cornerRadius: 14, style: .continuous)
                )
                .frame(maxWidth: .infinity, alignment: message.isUser ? .trailing : .leading)
            if !message.isUser { Spacer(minLength: 40) }
        }
    }
}

private struct DivoHistorySheet: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                List {
                    if env.divoThreads.isEmpty {
                        Text("No conversations yet.").font(.caption).foregroundStyle(AirNoteDesign.muted)
                    } else {
                        ForEach(env.divoThreads) { t in
                            Button {
                                Task { await env.openDivoThread(t.id); dismiss() }
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(t.displayTitle).foregroundStyle(AirNoteDesign.foreground)
                                    if let p = t.preview, !p.isEmpty {
                                        Text(p).font(.caption).foregroundStyle(AirNoteDesign.muted).lineLimit(1)
                                    }
                                }
                            }
                        }
                    }
                }
                .scrollContentBackground(.hidden)
            }
            .navigationTitle("History")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
            .task { await env.refreshDivoThreads() }
        }
    }
}
