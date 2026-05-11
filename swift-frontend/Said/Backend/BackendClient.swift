import Foundation
import os

final class BackendClient {
    private let sidecar: SidecarManager
    private let session = URLSession.shared
    private let logger = Logger(subsystem: "com.emiac.said", category: "backend-client")

    init(sidecar: SidecarManager) {
        self.sidecar = sidecar
    }

    private var baseURL: String { sidecar.baseURL }
    private var auth: String { sidecar.authHeader }

    // MARK: - Preferences

    func getPreferences() async throws -> Preferences {
        try await get("/v1/preferences")
    }

    func patchPreferences(_ update: [String: Any]) async throws -> Preferences {
        try await patch("/v1/preferences", body: update)
    }

    // MARK: - History

    func getHistory(limit: Int = 50, offset: Int = 0) async throws -> [Recording] {
        try await get("/v1/history?limit=\(limit)&offset=\(offset)")
    }

    func deleteRecording(_ id: String) async throws {
        try await delete("/v1/recordings/\(id)")
    }

    func getRecordingAudio(_ id: String) async throws -> Data {
        try await getRaw("/v1/recordings/\(id)/audio")
    }

    // MARK: - Voice

    func polishTranscript(transcript: String, audio: Data? = nil) async throws -> URLSession.AsyncBytes {
        let url = URL(string: "\(baseURL)/v1/voice/polish-transcript")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: ["transcript": transcript])
        let (bytes, response) = try await session.bytes(for: req)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw BackendError.httpError((response as? HTTPURLResponse)?.statusCode ?? 0)
        }
        return bytes
    }

    func polishVoice(wav: Data, preTranscript: String? = nil) async throws -> URLSession.AsyncBytes {
        let url = URL(string: "\(baseURL)/v1/voice/polish")!
        let boundary = UUID().uuidString
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")

        var body = Data()
        // audio field
        body.append("--\(boundary)\r\n".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"audio\"; filename=\"recording.wav\"\r\n".data(using: .utf8)!)
        body.append("Content-Type: audio/wav\r\n\r\n".data(using: .utf8)!)
        body.append(wav)
        body.append("\r\n".data(using: .utf8)!)
        // pre_transcript field (from Deepgram streaming)
        if let transcript = preTranscript, !transcript.isEmpty {
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"pre_transcript\"\r\n\r\n".data(using: .utf8)!)
            body.append(transcript.data(using: .utf8)!)
            body.append("\r\n".data(using: .utf8)!)
        }
        body.append("--\(boundary)--\r\n".data(using: .utf8)!)
        req.httpBody = body

        let (bytes, response) = try await session.bytes(for: req)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw BackendError.httpError((response as? HTTPURLResponse)?.statusCode ?? 0)
        }
        return bytes
    }

    /// Build a URLRequest for /v1/voice/polish with enriched transcript + meta.
    func buildPolishRequest(wav: Data, streamingTranscript: StreamingTranscript? = nil) -> URLRequest {
        let url = URL(string: "\(baseURL)/v1/voice/polish")!
        let boundary = UUID().uuidString
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 120

        var body = Data()
        body.append("--\(boundary)\r\n".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"audio\"; filename=\"recording.wav\"\r\n".data(using: .utf8)!)
        body.append("Content-Type: audio/wav\r\n\r\n".data(using: .utf8)!)
        body.append(wav)
        body.append("\r\n".data(using: .utf8)!)

        if let st = streamingTranscript {
            body.appendField(boundary: boundary, name: "pre_transcript", value: st.transcript)
            if let metaJSON = try? JSONEncoder().encode(st.meta),
               let metaStr = String(data: metaJSON, encoding: .utf8) {
                body.appendField(boundary: boundary, name: "pre_transcript_meta", value: metaStr)
            }
        }

        body.append("--\(boundary)--\r\n".data(using: .utf8)!)
        req.httpBody = body
        return req
    }

    /// Build a request for /v1/text/polish (polish selected text with tone).
    func buildTextPolishRequest(text: String, tone: String? = nil) -> URLRequest {
        let url = URL(string: "\(baseURL)/v1/text/polish")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 120
        var body: [String: Any] = ["text": text]
        if let t = tone { body["tone_override"] = t }
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)
        return req
    }

    /// Build a request for /v1/text/format-fix (literal dictation formatting repair).
    func buildTextFormatFixRequest(text: String) -> URLRequest {
        let url = URL(string: "\(baseURL)/v1/text/format-fix")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 120
        req.httpBody = try? JSONSerialization.data(withJSONObject: ["text": text])
        return req
    }

    func getSTTBias() async throws -> BiasPackage {
        try await get("/v1/stt/bias")
    }

    /// Fire-and-forget pre-embed to warm the backend's embedding cache.
    /// Matches Tauri's async pre-embed fired during Deepgram drain.
    func preEmbed(text: String) async {
        guard let url = URL(string: "\(baseURL)/v1/pre-embed") else { return }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.timeoutInterval = 5
        req.httpBody = try? JSONSerialization.data(withJSONObject: ["text": text])
        _ = try? await session.data(for: req)
    }

    // MARK: - Vocabulary

    func getVocabulary() async throws -> [VocabTerm] {
        try await get("/v1/vocabulary/terms")
    }

    func addVocabularyTerm(_ term: String, context: String? = nil) async throws {
        var body: [String: Any] = ["term": term]
        if let ctx = context { body["context"] = ctx }
        let _: EmptyResponse = try await post("/v1/vocabulary", body: body)
    }

    func deleteVocabularyTerm(_ term: String) async throws {
        try await delete("/v1/vocabulary/\(term.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? term)")
    }

    func starVocabularyTerm(_ term: String) async throws {
        let _: EmptyResponse = try await post(
            "/v1/vocabulary/\(term.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? term)/star",
            body: [String: Any]()
        )
    }

    // MARK: - Pending Edits

    func getPendingEdits() async throws -> PendingEditsResponse {
        try await get("/v1/pending-edits")
    }

    func resolvePendingEdit(id: Int64, action: String) async throws {
        let _: EmptyResponse = try await post("/v1/pending-edits/\(id)/resolve", body: ["action": action])
    }

    // MARK: - STT Bias

    // MARK: - OpenAI OAuth

    func getOpenAIStatus() async throws -> OpenAIStatus {
        try await get("/v1/openai-oauth/status")
    }

    func initiateOpenAIOAuth() async throws {
        let _: EmptyResponse = try await post("/v1/openai-oauth/initiate", body: [String: Any]())
    }

    func disconnectOpenAI() async throws {
        try await delete("/v1/openai-oauth/disconnect")
    }

    // MARK: - Debug

    func getDebugLogs() async throws -> DebugLogs {
        try await get("/v1/debug/logs")
    }

    // MARK: - Generic HTTP

    private func get<T: Decodable>(_ path: String) async throws -> T {
        let url = URL(string: "\(baseURL)\(path)")!
        var req = URLRequest(url: url)
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        let (data, response) = try await session.data(for: req)
        try checkResponse(response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func getRaw(_ path: String) async throws -> Data {
        let url = URL(string: "\(baseURL)\(path)")!
        var req = URLRequest(url: url)
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        let (data, response) = try await session.data(for: req)
        try checkResponse(response)
        return data
    }

    private func post<T: Decodable>(_ path: String, body: [String: Any]) async throws -> T {
        let url = URL(string: "\(baseURL)\(path)")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await session.data(for: req)
        try checkResponse(response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func patch<T: Decodable>(_ path: String, body: [String: Any]) async throws -> T {
        let url = URL(string: "\(baseURL)\(path)")!
        var req = URLRequest(url: url)
        req.httpMethod = "PATCH"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await session.data(for: req)
        try checkResponse(response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func delete(_ path: String) async throws {
        let url = URL(string: "\(baseURL)\(path)")!
        var req = URLRequest(url: url)
        req.httpMethod = "DELETE"
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        let (_, response) = try await session.data(for: req)
        try checkResponse(response)
    }

    private func checkResponse(_ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else {
            throw BackendError.noResponse
        }
        guard (200...299).contains(http.statusCode) else {
            throw BackendError.httpError(http.statusCode)
        }
    }
}

struct BiasPackage: Codable {
    var stt_mode: String
    var keyterms: [String]
    var replacements: [ReplacementRule]
}

struct ReplacementRule: Codable {
    var find: String
    var replace: String?
}

struct DebugLogs: Codable {
    var combined: String
    var desktop: String
    var backend: String
    var desktop_path: String?
    var backend_path: String?
    var truncated: Bool?
}

struct EmptyResponse: Codable {}

enum BackendError: Error, LocalizedError {
    case noResponse
    case httpError(Int)

    var errorDescription: String? {
        switch self {
        case .noResponse: return "No response from backend"
        case .httpError(let code): return "Backend returned HTTP \(code)"
        }
    }
}

private extension Data {
    mutating func appendField(boundary: String, name: String, value: String) {
        append("--\(boundary)\r\n".data(using: .utf8)!)
        append("Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n".data(using: .utf8)!)
        append(value.data(using: .utf8)!)
        append("\r\n".data(using: .utf8)!)
    }
}
