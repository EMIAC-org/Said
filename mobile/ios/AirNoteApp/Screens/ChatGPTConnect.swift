import AirNoteShared
import AuthenticationServices
import Network
import SwiftUI
import UIKit

/// Connects the org's ChatGPT account via OpenAI's PKCE/loopback OAuth — the same
/// flow the desktop uses, adapted for iOS. Because OpenAI's Codex client only allows
/// the fixed `http://localhost:1455/auth/callback` redirect, we run a tiny one-shot
/// HTTP listener on that port to catch the auth code, then exchange it server-side.
///
/// We use `ASWebAuthenticationSession` (not Safari) on purpose: the auth UI is a
/// modal OVER AirNote, so the app stays foregrounded and the loopback listener keeps
/// running to receive the redirect. The listener is started only for the duration of
/// a connect attempt and torn down immediately after — it touches nothing else.
@MainActor
final class ChatGPTConnectController: NSObject, ObservableObject {
    @Published private(set) var status: OpenAIConnectionStatus?
    @Published private(set) var isBusy = false
    @Published var errorMessage: String?

    private let gateway: any MobileGatewayClient
    private var authSession: ASWebAuthenticationSession?
    private var listener: NWListener?
    private var expectedState: String?
    private var codeVerifier: String?
    private var didFinish = false

    /// OpenAI's Codex client only permits this loopback redirect — must match the
    /// server's `OPENAI_REDIRECT_URI` (`codex_client.rs`).
    private static let loopbackPort: NWEndpoint.Port = 1455

    init(gateway: any MobileGatewayClient) {
        self.gateway = gateway
    }

    var isConnected: Bool { status?.connected == true }

    func refreshStatus() async {
        status = (try? await gateway.openaiStatus())
    }

    func connect() async {
        guard !isBusy else { return }
        isBusy = true
        errorMessage = nil
        didFinish = false
        do {
            let info = try await gateway.openaiConnect()
            codeVerifier = info.codeVerifier
            expectedState = info.state
            try startLoopback()
            guard let url = URL(string: info.authUrl) else {
                throw ConnectError.message("The sign-in link from the server was invalid.")
            }
            startAuthSession(url: url)
            // isBusy stays true until the loopback callback (or cancel) resolves it.
        } catch {
            teardown()
            isBusy = false
            errorMessage = friendly(error)
        }
    }

    func disconnect() async {
        guard !isBusy else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            try await gateway.openaiDisconnect()
        } catch {
            errorMessage = friendly(error)
        }
        await refreshStatus()
    }

    // MARK: Loopback HTTP listener (one-shot, port 1455)

    private func startLoopback() throws {
        let params = NWParameters.tcp
        params.allowLocalEndpointReuse = true
        let listener = try NWListener(using: params, on: Self.loopbackPort)
        listener.newConnectionHandler = { [weak self] connection in
            self?.handle(connection)
        }
        listener.start(queue: .main)
        self.listener = listener
    }

    private nonisolated func handle(_ connection: NWConnection) {
        connection.start(queue: .global())
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16_384) { [weak self] data, _, _, _ in
            guard let self else { connection.cancel(); return }
            let request = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            let (code, state) = Self.parseCallback(request)
            let body = """
            <html><head><meta name="viewport" content="width=device-width,initial-scale=1"></head>\
            <body style="font-family:-apple-system;text-align:center;padding:48px 24px;color:#111">\
            <h2>\(code != nil ? "✅ ChatGPT connected" : "⚠️ Couldn’t connect")</h2>\
            <p>You can return to AirNote.</p></body></html>
            """
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n" +
                "Content-Length: \(body.utf8.count)\r\nConnection: close\r\n\r\n\(body)"
            connection.send(content: response.data(using: .utf8), completion: .contentProcessed { _ in
                connection.cancel()
            })
            Task { @MainActor in await self.finish(code: code, state: state) }
        }
    }

    /// Parse the auth `code` + `state` out of the `GET /auth/callback?...` request line.
    private nonisolated static func parseCallback(_ request: String) -> (code: String?, state: String?) {
        guard let line = request.split(separator: "\r\n").first,
              let pathPart = line.split(separator: " ").dropFirst().first,
              let comps = URLComponents(string: "http://localhost\(pathPart)")
        else { return (nil, nil) }
        let items = comps.queryItems ?? []
        return (items.first { $0.name == "code" }?.value,
                items.first { $0.name == "state" }?.value)
    }

    private func finish(code: String?, state: String?) async {
        guard !didFinish else { return }
        didFinish = true
        teardown()
        defer { isBusy = false }

        guard let code, !code.isEmpty, state == expectedState, let verifier = codeVerifier else {
            errorMessage = "Sign-in didn’t complete. Please try again."
            await refreshStatus()
            return
        }
        do {
            try await gateway.openaiComplete(code: code, codeVerifier: verifier)
        } catch {
            errorMessage = friendly(error)
        }
        await refreshStatus()
    }

    // MARK: ASWebAuthenticationSession

    private func startAuthSession(url: URL) {
        // The real callback is the loopback (http), not a custom scheme, so the
        // session never auto-completes — the loopback listener does, and we cancel
        // the session from `finish`. The scheme here is just a required placeholder.
        let session = ASWebAuthenticationSession(url: url, callbackURLScheme: "airnoteoai") { [weak self] _, _ in
            // Fires only if the USER cancels the sheet — treat as an aborted connect.
            Task { @MainActor in await self?.finish(code: nil, state: nil) }
        }
        session.presentationContextProvider = self
        session.prefersEphemeralWebBrowserSession = false
        authSession = session
        session.start()
    }

    private func teardown() {
        listener?.cancel()
        listener = nil
        authSession?.cancel()
        authSession = nil
    }

    private enum ConnectError: Error { case message(String) }

    private func friendly(_ error: Error) -> String {
        if case ConnectError.message(let m) = error { return m }
        if let g = error as? GatewayError {
            if case .server(403, _, _) = g { return "Only an org admin can connect ChatGPT." }
            return g.userMessage
        }
        return "Something went wrong. Please try again."
    }
}

extension ChatGPTConnectController: ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for _: ASWebAuthenticationSession) -> ASPresentationAnchor {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first { $0.isKeyWindow } ?? ASPresentationAnchor()
    }
}

/// Settings section mirroring the desktop's "Connect ChatGPT" card.
struct ChatGPTConnectSection: View {
    @StateObject private var controller: ChatGPTConnectController
    @State private var loaded = false

    init(gateway: any MobileGatewayClient) {
        _controller = StateObject(wrappedValue: ChatGPTConnectController(gateway: gateway))
    }

    var body: some View {
        Section {
            HStack {
                Label("ChatGPT", systemImage: "sparkles")
                Spacer()
                if controller.isBusy {
                    ProgressView().controlSize(.small)
                } else {
                    Text(controller.isConnected ? "Connected" : "Not connected")
                        .font(.callout)
                        .foregroundStyle(controller.isConnected ? AirNoteDesign.success : AirNoteDesign.muted)
                }
            }
            if controller.isConnected {
                if let label = controller.status?.label, !label.isEmpty {
                    LabeledContent("Account", value: label)
                }
                Button(role: .destructive) {
                    Task { await controller.disconnect() }
                } label: {
                    Label("Disconnect ChatGPT", systemImage: "xmark.circle")
                }
                .disabled(controller.isBusy)
            } else {
                Button {
                    Task { await controller.connect() }
                } label: {
                    Label("Connect ChatGPT", systemImage: "link")
                }
                .disabled(controller.isBusy)
            }
            if let err = controller.errorMessage {
                Label(err, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.warning)
            }
        } header: {
            Text("ChatGPT")
        } footer: {
            Text("Optional. When connected, your dictation is polished with your org's ChatGPT — otherwise it uses Groq. Only an org admin can connect it.")
        }
        .task {
            guard !loaded else { return }
            loaded = true
            await controller.refreshStatus()
        }
    }
}
