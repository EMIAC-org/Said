import AirNoteShared
import SwiftUI

struct AccountSignInView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var email = ""
    @State private var password = ""
    @State private var signup = false

    var body: some View {
        Form {
            Section {
                TextField("Email", text: $email)
                    .textInputAutocapitalization(.never)
                    .keyboardType(.emailAddress)
                    .textContentType(.emailAddress)
                SecureField("Password", text: $password)
                    .textContentType(signup ? .newPassword : .password)
                Toggle("Create new account", isOn: $signup)
            } header: {
                Text("Account")
            } footer: {
                Text("This account belongs to the independent AirNote Mobile Gateway, not the desktop control-plane.")
            }

            Section {
                Button {
                    Task { await environment.authenticate(email: email, password: password, signup: signup) }
                } label: {
                    Label(signup ? "Create account" : "Sign in", systemImage: "person.crop.circle.badge.checkmark")
                }
                .disabled(email.isEmpty || password.count < 8)
            }

            if let account = environment.account {
                Section("Signed in") {
                    LabeledContent("Email", value: account.email)
                    LabeledContent("Plan", value: account.licenseTier.capitalized)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("Account")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct LanguageStyleView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        Form {
            Section("Language") {
                Picker("Language", selection: $environment.languageHint) {
                    Text("Auto").tag(LanguageHint.auto)
                    Text("English").tag(LanguageHint.en)
                    Text("Hindi").tag(LanguageHint.hi)
                    Text("Hinglish").tag(LanguageHint.hinglish)
                }
                .pickerStyle(.segmented)
            }

            Section("Style") {
                Picker("Style", selection: $environment.style) {
                    Text("Direct").tag(DictationStyle.direct)
                    Text("Work").tag(DictationStyle.work)
                    Text("Casual").tag(DictationStyle.casual)
                    Text("Email").tag(DictationStyle.email)
                    Text("Notes").tag(DictationStyle.notes)
                }
                .pickerStyle(.inline)
            }

            Section {
                AirNoteInlinePreview(
                    title: "Preview",
                    copy: "Kal ka update concise bana ke Rahul ko bhej do.",
                    badge: "\(environment.languageHint.rawValue.capitalized) - \(environment.style.rawValue.capitalized)"
                )
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("Language & Style")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct HistoryView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        List {
            if environment.dictationStore.records.isEmpty {
                ContentUnavailableView(
                    "No dictations yet",
                    systemImage: "clock.arrow.circlepath",
                    description: Text("Inserted, copied, and saved results appear here for recovery.")
                )
            } else {
                ForEach(environment.dictationStore.records) { record in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(record.polished)
                            .font(.body)
                        Text(record.transcript)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        HStack {
                            Label(record.outcome.rawValue, systemImage: "checkmark.circle")
                            Spacer()
                            Text(record.createdAt, style: .time)
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 6)
                    .accessibilityElement(children: .combine)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("History")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct VocabularyView: View {
    @State private var term = ""
    @State private var alias = ""

    var body: some View {
        Form {
            Section("Add term") {
                TextField("Correct spelling", text: $term)
                TextField("Heard as / alias", text: $alias)
                Button {
                    term = ""
                    alias = ""
                } label: {
                    Label("Save vocabulary", systemImage: "text.badge.plus")
                }
                .disabled(term.isEmpty)
            }

            Section("Review") {
                AirNoteInlinePreview(title: "Macobs", copy: "Aliases: mac ops, macobs. Used in work messages.", badge: "Personal")
                AirNoteInlinePreview(title: "Rahul", copy: "Name. Keep capitalized in final text.", badge: "Name")
                AirNoteInlinePreview(title: "EMIAC", copy: "Company term from shared vocabulary.", badge: "Company")
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("Vocabulary")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct AirNoteSettingsView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @State private var diagnosticsEnabled = true
    @State private var localHistoryEnabled = true

    var body: some View {
        Form {
            Section("Privacy") {
                Toggle("Local history", isOn: $localHistoryEnabled)
                LabeledContent("Raw audio", value: "Not stored")
                LabeledContent("Raw server text", value: "Not stored by default")
                Button(role: .destructive) {
                } label: {
                    Label("Delete mobile data", systemImage: "trash")
                }
            }

            Section("Diagnostics") {
                Toggle("Redacted diagnostics", isOn: $diagnosticsEnabled)
                NavigationLink(destination: DiagnosticsView()) {
                    Label("Gateway diagnostics", systemImage: "stethoscope")
                }
            }

            Section("Account") {
                LabeledContent("Gateway", value: BuildConfig.gatewayBaseURL.host ?? "Configured")
                LabeledContent("Runtime", value: environment.runtimeStatus)
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("Settings")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct DiagnosticsView: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        List {
            Section("Build") {
                LabeledContent("App", value: "0.1.0(1)")
                LabeledContent("Gateway", value: BuildConfig.gatewayBaseURL.absoluteString)
                LabeledContent("Mode", value: BuildConfig.useMockGateway ? "Mock" : "Live")
            }

            Section("Runtime") {
                LabeledContent("Status", value: environment.runtimeStatus)
                LabeledContent("Language", value: environment.languageHint.rawValue)
                LabeledContent("Style", value: environment.style.rawValue)
            }

            Section {
                Button {
                    Task { await environment.refreshRuntimeConfig() }
                } label: {
                    Label("Refresh gateway status", systemImage: "arrow.clockwise")
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("Diagnostics")
        .navigationBarTitleDisplayMode(.inline)
    }
}

private struct AirNoteInlinePreview: View {
    var title: String
    var copy: String
    var badge: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(title)
                    .font(.headline)
                Spacer()
                Text(badge)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.accent)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(AirNoteDesign.accent.opacity(0.12), in: Capsule())
            }
            Text(copy)
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }
}
