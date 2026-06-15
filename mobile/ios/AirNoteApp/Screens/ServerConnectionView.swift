import AirNoteShared
import SwiftUI

/// Enterprise / self-hosted server connection. Mirrors the desktop's workspace
/// connect flow (desktop/src/lib/enterprise.ts validateServer): enter a
/// control-plane URL, validate `{url}/v1/health` (ok == true), then persist it
/// in the App Group so the app AND the keyboard extension both connect there.
struct ServerConnectionView: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var urlText: String = SharedStore.customGatewayURL ?? ""
    @State private var status: Status = .idle
    @State private var checking = false

    enum Status: Equatable {
        case idle
        case ok(String)
        case failed(String)
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            Form {
                Section {
                    TextField("https://your-server.example.com", text: $urlText)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .font(.callout.monospaced())
                    Button {
                        Task { await testConnection() }
                    } label: {
                        HStack(spacing: 8) {
                            if checking { ProgressView().controlSize(.small) }
                            Text(checking ? "Testing…" : "Test connection")
                        }
                    }
                    .disabled(checking || normalizedURL == nil)
                } header: {
                    Text("Server URL")
                } footer: {
                    statusFooter
                }

                Section {
                    Button {
                        save()
                    } label: {
                        Label("Use this server", systemImage: "checkmark.circle")
                    }
                    .disabled(normalizedURL == nil)
                    Button(role: .destructive) {
                        reset()
                    } label: {
                        Label("Reset to AirNote default", systemImage: "arrow.uturn.backward")
                    }
                } footer: {
                    Text("Currently connected to: \(BuildConfig.gatewayBaseURL.absoluteString)\n\nThe app and the keyboard both use this server. After changing it, sign out and sign back in so your session matches the new server.")
                }
            }
            .scrollContentBackground(.hidden)
        }
        .navigationTitle("Server")
        .navigationBarTitleDisplayMode(.inline)
        .tint(AirNoteDesign.accent)
    }

    /// Normalize the typed URL: default to https, strip trailing slashes, require a host.
    private var normalizedURL: URL? {
        var s = urlText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty else { return nil }
        if !s.lowercased().hasPrefix("http://"), !s.lowercased().hasPrefix("https://") {
            s = "https://" + s
        }
        while s.hasSuffix("/") { s.removeLast() }
        guard let url = URL(string: s), url.host != nil else { return nil }
        return url
    }

    @ViewBuilder private var statusFooter: some View {
        switch status {
        case .idle:
            Text("Enter your AirNote control-plane URL, then test it before saving.")
        case .ok(let message):
            Label(message, systemImage: "checkmark.circle.fill")
                .foregroundStyle(AirNoteDesign.success)
        case .failed(let message):
            Label(message, systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(AirNoteDesign.warning)
        }
    }

    private func testConnection() async {
        guard let base = normalizedURL else { return }
        checking = true
        defer { checking = false }
        var request = URLRequest(url: base.appendingPathComponent("v1/health"))
        request.timeoutInterval = 8
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            let code = (response as? HTTPURLResponse)?.statusCode ?? 0
            let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            if code == 200, (body?["ok"] as? Bool) == true {
                let version = body?["version"] as? String
                status = .ok("Connected" + (version.map { " · v\($0)" } ?? ""))
            } else {
                status = .failed("Server replied \(code) without a valid health check.")
            }
        } catch {
            status = .failed("Couldn't reach \(base.host ?? "server"): \(error.localizedDescription)")
        }
    }

    private func save() {
        guard let base = normalizedURL else { return }
        SharedStore.customGatewayURL = base.absoluteString
        status = .ok("Saved — \(base.absoluteString). Sign out and back in to finish.")
    }

    private func reset() {
        SharedStore.customGatewayURL = nil
        urlText = ""
        status = .idle
    }
}
