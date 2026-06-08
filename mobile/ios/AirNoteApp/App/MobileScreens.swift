import AirNoteShared
import SwiftUI
import UIKit

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
            Section {
                Button {
                    Task { await environment.refreshHistory() }
                } label: {
                    Label("Refresh server history", systemImage: "arrow.clockwise")
                }
                LabeledContent("Status", value: environment.historyStatus)
            }

            if environment.learningItem != nil {
                LearningReviewSection()
            }

            if environment.serverHistory.isEmpty {
                ContentUnavailableView(
                    "No dictations yet",
                    systemImage: "clock.arrow.circlepath",
                    description: Text("Inserted, copied, and saved results appear here for recovery.")
                )
            } else {
                ForEach(environment.serverHistory) { record in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(record.displayText)
                            .font(.body)
                        if !record.transcript.isEmpty && record.transcript != record.displayText {
                            Text(record.transcript)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        HStack {
                            Label(record.source, systemImage: "checkmark.circle")
                            Spacer()
                            Text(record.createdAt, style: .time)
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 6)
                    .accessibilityElement(children: .combine)
                    .swipeActions(edge: .trailing) {
                        Button(role: .destructive) {
                            Task { await environment.deleteHistoryItem(record) }
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                    .swipeActions(edge: .leading) {
                        Button {
                            environment.startLearningReview(record)
                        } label: {
                            Label("Learn", systemImage: "checkmark.seal")
                        }
                        .tint(AirNoteDesign.success)

                        Button {
                            UIPasteboard.general.string = record.displayText
                        } label: {
                            Label("Copy", systemImage: "doc.on.doc")
                        }
                        .tint(AirNoteDesign.accent)
                    }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(AirNoteBackground())
        .tint(AirNoteDesign.accent)
        .navigationTitle("History")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            await environment.refreshHistory()
        }
    }
}

private struct LearningReviewSection: View {
    @EnvironmentObject private var environment: AppEnvironment

    var body: some View {
        Section("Learning review") {
            TextEditor(text: $environment.learningDraftText)
                .frame(minHeight: 92)
                .font(.body)
                .overlay(alignment: .topLeading) {
                    if environment.learningDraftText.isEmpty {
                        Text("Kept text")
                            .foregroundStyle(.secondary)
                            .padding(.top, 8)
                            .padding(.leading, 5)
                    }
                }

            if let item = environment.learningItem,
               !item.transcript.isEmpty,
               item.transcript != item.displayText {
                Text(item.transcript)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Text(environment.learningStatus)
                .font(.caption)
                .foregroundStyle(environment.learningStatus.hasPrefix("Could not") ? AirNoteDesign.danger : AirNoteDesign.muted)

            ForEach(environment.learningCandidates) { candidate in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(candidate.corrected.isEmpty ? "Candidate" : candidate.corrected)
                            .font(.subheadline.weight(.semibold))
                        Spacer()
                        Text(candidate.termType)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(AirNoteDesign.accent)
                    }
                    Text(candidate.original)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 4)
            }

            HStack {
                Button {
                    environment.cancelLearningReview()
                } label: {
                    Label("Close", systemImage: "xmark")
                }
                .disabled(environment.learningWorking)

                Spacer()

                Button {
                    Task { await environment.analyzeLearningEdit() }
                } label: {
                    Label(environment.learningWorking ? "Analyzing" : "Analyze", systemImage: "magnifyingglass")
                }
                .disabled(environment.learningWorking)

                Button {
                    Task { await environment.confirmLearning() }
                } label: {
                    Label("Learn", systemImage: "checkmark.seal.fill")
                }
                .disabled(environment.learningWorking || environment.learningCandidates.isEmpty)
            }
            .font(.caption.weight(.semibold))
        }
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
    @AppStorage("airnotePreferredAppearance") private var appearance = AirNoteAppearance.system.rawValue
    @State private var diagnosticsEnabled = true

    var body: some View {
        Form {
            Section("Appearance") {
                AirNoteAppearancePicker()
                LabeledContent("Current choice", value: (AirNoteAppearance(rawValue: appearance) ?? .system).detail)
            }

            Section("Privacy") {
                LabeledContent("History", value: "Server")
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
                LabeledContent("Mode", value: BuildConfig.useMockGateway ? "Preview" : "Live")
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

#Preview("Account") {
    NavigationStack { AccountSignInView().environmentObject(AppEnvironment()) }
}

#Preview("Language & Style") {
    NavigationStack { LanguageStyleView().environmentObject(AppEnvironment()) }
}

#Preview("History") {
    NavigationStack { HistoryView().environmentObject(AppEnvironment()) }
}

#Preview("Vocabulary") {
    NavigationStack { VocabularyView() }
}

#Preview("Settings") {
    NavigationStack { AirNoteSettingsView().environmentObject(AppEnvironment()) }
}

#Preview("Diagnostics") {
    NavigationStack { DiagnosticsView().environmentObject(AppEnvironment()) }
}
