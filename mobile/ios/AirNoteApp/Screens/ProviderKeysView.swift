import AirNoteShared
import SwiftUI

/// Bring-your-own-key screen: the user pastes their Deepgram (speech-to-text) and
/// Groq (AI polish) keys, which are saved to the server vault (encrypted) under
/// their account — the same model the desktop app uses. Once both exist,
/// dictation turns on automatically.
struct ProviderKeysView: View {
    @EnvironmentObject private var env: AppEnvironment

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 16) {
                    intro
                    ProviderKeyCard(
                        provider: "deepgram",
                        name: "Deepgram",
                        role: "Speech-to-text",
                        badge: "D",
                        color: Color(red: 0.04, green: 0.55, blue: 0.54),
                        getKeyURL: URL(string: "https://console.deepgram.com/signup")!,
                        placeholder: "Paste your Deepgram key"
                    )
                    ProviderKeyCard(
                        provider: "groq",
                        name: "Groq",
                        role: "AI polish",
                        badge: "G",
                        color: Color(red: 0.96, green: 0.31, blue: 0.21),
                        getKeyURL: URL(string: "https://console.groq.com/keys")!,
                        placeholder: "gsk_…"
                    )
                    if !env.credentialStatus.isEmpty {
                        Text(env.credentialStatus)
                            .font(.caption)
                            .foregroundStyle(env.credentialStatus.hasPrefix("Couldn't") || env.credentialStatus.contains("too short") ? AirNoteDesign.danger : AirNoteDesign.success)
                    }
                }
                .padding(18)
            }
        }
        .navigationTitle("Voice keys")
        .navigationBarTitleDisplayMode(.inline)
        .task { await env.refreshCredentials() }
    }

    private var intro: some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 8) {
                AirNoteSectionLabel(text: "Connect your voice")
                Text("Two free keys power AirNote")
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text("Add your own Deepgram and Groq keys — both have free tiers that cover daily use. They're stored encrypted on AirNote's servers under your account and used only for your dictation.")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
                if env.dictationAvailable {
                    Label("Dictation is on", systemImage: "checkmark.circle.fill")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.success)
                }
            }
        }
    }
}

struct ProviderKeyCard: View {
    @EnvironmentObject private var env: AppEnvironment
    @Environment(\.openURL) private var openURL

    var provider: String
    var name: String
    var role: String
    var badge: String
    var color: Color
    var getKeyURL: URL
    var placeholder: String

    @State private var key = ""
    @State private var saving = false

    private var connected: RuntimeCredential? {
        env.credentials.first { $0.provider.lowercased() == provider && $0.status.lowercased() != "revoked" }
    }

    var body: some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Text(badge)
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 26, height: 26)
                        .background(color, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(name).font(.subheadline.weight(.semibold)).foregroundStyle(AirNoteDesign.foreground)
                        Text(role).font(.caption2).foregroundStyle(AirNoteDesign.muted)
                    }
                    Spacer()
                    if let connected {
                        AirNoteChip(text: connected.status.lowercased() == "active" ? "Connected" : connected.status.capitalized,
                                    tint: connected.status.lowercased() == "active" ? AirNoteDesign.success : AirNoteDesign.warning)
                    }
                }

                if let connected {
                    HStack {
                        Text("•••• \(connected.secretLast4)")
                            .font(.callout.monospaced())
                            .foregroundStyle(AirNoteDesign.muted)
                        Spacer()
                        Button(role: .destructive) {
                            Task { await env.deleteCredential(connected) }
                        } label: {
                            Label("Remove", systemImage: "trash")
                                .font(.caption.weight(.semibold))
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(AirNoteDesign.danger)
                    }
                } else {
                    HStack {
                        Spacer()
                        Button {
                            openURL(getKeyURL)
                        } label: {
                            Label("Get free key", systemImage: "arrow.up.right.square")
                                .font(.caption.weight(.semibold))
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(AirNoteDesign.accent)
                    }
                    SecureField(placeholder, text: $key)
                        .textFieldStyle(AirNoteFieldStyle())
                        .textContentType(.password)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    Button {
                        let value = key
                        saving = true
                        Task {
                            let ok = await env.saveProviderKey(provider: provider, secret: value)
                            await MainActor.run { saving = false; if ok { key = "" } }
                        }
                    } label: {
                        Label(saving ? "Saving…" : "Save \(name) key", systemImage: "checkmark.circle")
                    }
                    .buttonStyle(AirNotePrimaryButtonStyle())
                    .disabled(saving || key.trimmingCharacters(in: .whitespaces).count < 8)
                }
            }
        }
    }
}
