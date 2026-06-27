import AirNoteShared
import SwiftUI

/// Enterprise / self-hosted server connection. Mirrors the desktop's workspace
/// connect flow (desktop/src/lib/enterprise.ts validateServer): enter a
/// control-plane URL, validate `{url}/v1/health` (ok == true), then persist it
/// in the App Group so the app AND the keyboard extension both connect there.
/// Also offers recent servers (one-tap reconnect) and a paste-a-token sign-in
/// fallback, matching the desktop connect form.
struct ServerConnectionView: View {
    @EnvironmentObject private var env: AppEnvironment
    @State private var urlText: String = SharedStore.customGatewayURL ?? ""
    @State private var status: Status = .idle
    @State private var checking = false
    @State private var recents: [String] = SharedStore.recentGatewayURLs
    @State private var tokenText = ""
    @State private var signingIn = false
    @State private var tokenStatus: Status = .idle

    enum Status: Equatable {
        case idle
        case ok(String)
        case failed(String)
    }

    var body: some View {
        ZStack {
            AirNoteBackground()
            Form {
                urlSection
                if !recents.isEmpty { recentsSection }
                actionsSection
                tokenSection
            }
            .scrollContentBackground(.hidden)
        }
        .navigationTitle("Server")
        .navigationBarTitleDisplayMode(.inline)
        .tint(AirNoteDesign.accent)
        .onAppear { recents = SharedStore.recentGatewayURLs }
    }

    private var urlSection: some View {
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
    }

    private var recentsSection: some View {
        Section {
            ForEach(recents, id: \.self) { url in
                Button {
                    urlText = url
                    status = .idle
                } label: {
                    HStack {
                        Image(systemName: "clock.arrow.circlepath")
                            .foregroundStyle(AirNoteDesign.muted)
                        Text(url)
                            .font(.callout)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                        if url.caseInsensitiveCompare(BuildConfig.gatewayBaseURL.absoluteString) == .orderedSame {
                            Image(systemName: "checkmark").foregroundStyle(AirNoteDesign.success)
                        }
                    }
                }
                .tint(AirNoteDesign.foreground)
            }
            .onDelete { offsets in
                recents.remove(atOffsets: offsets)
                SharedStore.recentGatewayURLs = recents
            }
        } header: {
            Text("Recent servers")
        }
    }

    private var actionsSection: some View {
        Section {
            Button {
                Task { await saveValidated() }
            } label: {
                HStack(spacing: 8) {
                    if checking { ProgressView().controlSize(.small) }
                    Label(checking ? "Checking…" : "Use this server", systemImage: "checkmark.circle")
                }
            }
            .disabled(checking || normalizedURL == nil)
            Button(role: .destructive) {
                reset()
            } label: {
                Label("Reset to AirNote default", systemImage: "arrow.uturn.backward")
            }
        } footer: {
            Text("Currently connected to: \(BuildConfig.gatewayBaseURL.absoluteString)\n\nThe app and the keyboard both use this server. Saving signs you out so you can sign in to the new server.")
        }
    }

    private var tokenSection: some View {
        Section {
            SecureField("Paste a sign-in token", text: $tokenText)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(.callout.monospaced())
            Button {
                Task { await signInWithToken() }
            } label: {
                HStack(spacing: 8) {
                    if signingIn { ProgressView().controlSize(.small) }
                    Text(signingIn ? "Signing in…" : "Sign in with token")
                }
            }
            .disabled(signingIn || tokenText.trimmingCharacters(in: .whitespaces).count < 8)
        } header: {
            Text("Advanced — sign in with a token")
        } footer: {
            tokenFooter
        }
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

    @ViewBuilder private var tokenFooter: some View {
        switch tokenStatus {
        case .idle:
            Text("If Lark sign-in can't open, paste a session token issued by your workspace to connect to the server above.")
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

    /// Validate the server is actually reachable BEFORE pointing the app (and the
    /// keyboard) at it. Saving a dead URL would sign the user out onto an
    /// unreachable server with no way back — so we require a passing health check,
    /// mirroring the desktop's validate-then-persist flow.
    private func saveValidated() async {
        guard let base = normalizedURL else { return }
        checking = true
        defer { checking = false }
        var request = URLRequest(url: base.appendingPathComponent("v1/health"))
        request.timeoutInterval = 8
        let healthy: Bool
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            let code = (response as? HTTPURLResponse)?.statusCode ?? 0
            let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            healthy = code == 200 && (body?["ok"] as? Bool) == true
        } catch {
            healthy = false
        }
        guard healthy else {
            status = .failed("Couldn't reach \(base.host ?? "that server"). Check the URL and your connection before saving.")
            return
        }
        SharedStore.customGatewayURL = base.absoluteString
        SharedStore.rememberGatewayURL(base.absoluteString)
        recents = SharedStore.recentGatewayURLs
        status = .ok("Connected to \(base.host ?? "the new server"). Sign in to continue.")
        // Clients read the URL fresh; signing out lands on sign-in for the new
        // server, so no app relaunch is needed.
        env.signOut()
    }

    private func signInWithToken() async {
        signingIn = true
        defer { signingIn = false }
        // If a URL is typed, point at it so the token validates against the
        // intended server — but remember the previous target so a FAILED attempt
        // doesn't silently leave the app pointed at an unvalidated server.
        let previous = SharedStore.customGatewayURL
        if let base = normalizedURL {
            SharedStore.customGatewayURL = base.absoluteString
        }
        let ok = await env.signInWithToken(tokenText)
        if ok {
            if let base = normalizedURL {
                SharedStore.rememberGatewayURL(base.absoluteString)
                recents = SharedStore.recentGatewayURLs
            }
            tokenStatus = .ok("Signed in to \(BuildConfig.gatewayBaseURL.host ?? "the server").")
            tokenText = ""
        } else {
            SharedStore.customGatewayURL = previous   // revert: a failed token must not re-point the app
            tokenStatus = .failed(env.authError ?? "That token wasn't accepted.")
        }
    }

    private func reset() {
        SharedStore.customGatewayURL = nil
        urlText = ""
        status = .idle
    }
}
