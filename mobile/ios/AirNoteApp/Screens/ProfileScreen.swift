import AirNoteShared
import SwiftUI

/// Premium profile: identity hero, usage stats, and the personal customizations
/// (display name, avatar color, voice tone/language, learning). Pushed from the
/// Dashboard header avatar — lives inside the Dashboard's NavigationStack, so it
/// must NOT introduce its own.
struct ProfileScreen: View {
    @EnvironmentObject private var env: AppEnvironment

    // Local mirrors of the cosmetic prefs so the hero updates live as you edit.
    @State private var displayName = SharedStore.profileDisplayName
    @State private var accentIndex = SharedStore.profileAccentIndex
    @FocusState private var nameFocused: Bool

    private var email: String { env.account?.email ?? "you@airnote.app" }
    private var accent: Color { ProfileAccent.color(accentIndex) }
    private var resolvedName: String {
        let trimmed = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { return trimmed }
        // Fall back to the email's local part, title-cased.
        return email.split(separator: "@").first.map { $0.prefix(1).uppercased() + $0.dropFirst() } ?? "You"
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 18) {
                    hero
                    stats
                    customizeCard
                    voiceCard
                    accountCard
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 32)
            }
            .scrollDismissesKeyboard(.interactively)
        }
        .navigationTitle("Profile")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            displayName = env.profileDisplayName
            accentIndex = env.profileAccentIndex
        }
        .onChange(of: accentIndex) { _, value in env.setProfileAccentIndex(value) }
        .onChange(of: nameFocused) { _, focused in if !focused { commitName() } }
    }

    // MARK: Hero

    private var hero: some View {
        AirNoteCard {
            VStack(spacing: 12) {
                AccountAvatar(email: email, size: 84, name: resolvedName, tint: accent)
                    .overlay(
                        Circle().strokeBorder(accent.opacity(0.35), lineWidth: 2)
                            .frame(width: 92, height: 92)
                    )
                VStack(spacing: 3) {
                    Text(resolvedName)
                        .font(.title3.weight(.bold))
                        .foregroundStyle(AirNoteDesign.foreground)
                    Text(email)
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                }
                tierBadge
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 6)
        }
    }

    private var tierBadge: some View {
        let tier = (env.account?.licenseTier ?? "free").capitalized
        return HStack(spacing: 5) {
            Image(systemName: "sparkles")
                .font(.system(size: 11, weight: .bold))
            Text("\(tier) plan")
                .font(.caption2.weight(.bold))
        }
        .foregroundStyle(accent)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(Capsule().fill(accent.opacity(0.14)))
    }

    // MARK: Stats

    private var stats: some View {
        HStack(spacing: 10) {
            StatTile(value: "\(env.history.count)", label: "dictations", systemImage: "waveform", tint: accent)
            StatTile(value: "\(env.vocabTermCount)", label: "words", systemImage: "textformat.abc", tint: accent)
            StatTile(value: "\(env.vocabAliasCount)", label: "corrections", systemImage: "arrow.2.squarepath", tint: accent)
        }
    }

    // MARK: Customize

    private var customizeCard: some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 16) {
                AirNoteSectionLabel(text: "Make it yours")

                VStack(alignment: .leading, spacing: 7) {
                    Text("Display name")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.muted)
                    TextField("Your name", text: $displayName)
                        .focused($nameFocused)
                        .submitLabel(.done)
                        .onSubmit { commitName() }
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                        .background(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .fill(AirNoteDesign.surfaceRaised.opacity(0.5))
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .strokeBorder(nameFocused ? accent.opacity(0.6) : AirNoteDesign.border, lineWidth: 1)
                        )
                }

                VStack(alignment: .leading, spacing: 9) {
                    Text("Avatar color")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(AirNoteDesign.muted)
                    HStack(spacing: 12) {
                        ForEach(ProfileAccent.palette.indices, id: \.self) { idx in
                            Button {
                                accentIndex = idx
                            } label: {
                                Circle()
                                    .fill(ProfileAccent.palette[idx])
                                    .frame(width: 30, height: 30)
                                    .overlay(
                                        Circle().strokeBorder(.white, lineWidth: accentIndex == idx ? 2.5 : 0)
                                    )
                                    .overlay(
                                        Circle().strokeBorder(AirNoteDesign.border, lineWidth: accentIndex == idx ? 0 : 1)
                                    )
                                    .shadow(color: accentIndex == idx ? ProfileAccent.palette[idx].opacity(0.5) : .clear, radius: 4)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Avatar color \(idx + 1)")
                            .accessibilityAddTraits(accentIndex == idx ? .isSelected : [])
                        }
                    }
                }
            }
        }
    }

    // MARK: Voice & style

    private var voiceCard: some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 14) {
                AirNoteSectionLabel(text: "Voice & style")

                HStack {
                    Label("Language", systemImage: "globe")
                        .font(.subheadline)
                        .foregroundStyle(AirNoteDesign.foreground)
                    Spacer()
                    Picker("Language", selection: languageBinding) {
                        Text("Hinglish").tag("hinglish")
                        Text("English").tag("english")
                    }
                    .pickerStyle(.menu)
                    .tint(accent)
                    .labelsHidden()
                }

                Divider().overlay(AirNoteDesign.border)

                HStack {
                    Label("Tone", systemImage: "text.alignleft")
                        .font(.subheadline)
                        .foregroundStyle(AirNoteDesign.foreground)
                    Spacer()
                    Picker("Tone", selection: toneBinding) {
                        ForEach(AirNoteTone.all, id: \.key) { tone in
                            Text(tone.label).tag(tone.key)
                        }
                    }
                    .pickerStyle(.menu)
                    .tint(accent)
                    .labelsHidden()
                }

                Divider().overlay(AirNoteDesign.border)

                Toggle(isOn: learningBinding) {
                    Label("Learn from my corrections", systemImage: "brain.head.profile")
                        .font(.subheadline)
                        .foregroundStyle(AirNoteDesign.foreground)
                }
                .tint(accent)
            }
        }
    }

    // MARK: Account

    private var accountCard: some View {
        AirNoteCard {
            VStack(alignment: .leading, spacing: 14) {
                AirNoteSectionLabel(text: "Account")
                HStack {
                    Text("Signed in as")
                        .font(.subheadline)
                        .foregroundStyle(AirNoteDesign.muted)
                    Spacer()
                    Text(email)
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(AirNoteDesign.foreground)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Button(role: .destructive) {
                    env.signOut()
                } label: {
                    Label("Sign out", systemImage: "rectangle.portrait.and.arrow.right")
                        .font(.subheadline.weight(.semibold))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .tint(AirNoteDesign.danger)
            }
        }
    }

    // MARK: Bindings (single source of truth = AppEnvironment / SharedStore)

    private func commitName() {
        let trimmed = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        displayName = trimmed
        env.setProfileDisplayName(trimmed)
    }

    private var languageBinding: Binding<String> {
        Binding(get: { env.outputLanguage }, set: { value in Task { await env.setOutputLanguage(value) } })
    }
    private var toneBinding: Binding<String> {
        Binding(get: { env.tonePreset }, set: { value in Task { await env.setTonePreset(value) } })
    }
    private var learningBinding: Binding<Bool> {
        Binding(get: { env.learningEnabled }, set: { value in Task { await env.setLearningEnabled(value) } })
    }
}
