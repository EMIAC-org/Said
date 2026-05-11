import Foundation

struct Preferences: Codable {
    var user_id: String
    var selected_model: String
    var tone_preset: String
    var custom_prompt: String?
    var language: String
    var output_language: String
    var auto_paste: Bool
    var edit_capture: Bool
    var polish_text_hotkey: String
    var record_hotkey: String
    var learning_enabled: Bool
    var deepgram_api_key: String?
    var gemini_api_key: String?
    var gateway_api_key: String?
    var groq_api_key: String?
    var llm_provider: String
    var updated_at: Int64
}

struct AppSnapshot: Codable {
    var accessibility_granted: Bool
    var input_monitoring_granted: Bool
    var microphone_granted: Bool
    var auto_paste_supported: Bool
    var mode: String
    var recording: Bool
    var processing: Bool
}

struct Recording: Codable, Identifiable {
    var id: String
    var timestamp_ms: Int64
    var transcript: String?
    var polished: String?
    var final_text: String?
    var word_count: Int?
    var recording_seconds: Double?
    var model_used: String?
    var confidence: Double?
    var target_app: String?
    var edit_count: Int?
    var source: String?
    var audio_id: String?

    var formattedDate: String {
        let date = Date(timeIntervalSince1970: Double(timestamp_ms) / 1000)
        let fmt = DateFormatter()
        fmt.dateStyle = .medium
        fmt.timeStyle = .short
        return fmt.string(from: date)
    }
}

struct VocabTerm: Codable, Identifiable {
    var id: String { term }
    var term: String
    var example_context: String?
    var term_type: String?
    var meaning: String?
    var starred: Bool?
    var source: String?
}

struct PendingEdit: Codable, Identifiable {
    var id: Int64
    var wrong: String
    var right: String
    var context: String?
    var created_at: String?
}

struct PendingEditsResponse: Codable {
    var edits: [PendingEdit]
    var total: Int
}

struct OpenAIStatus: Codable {
    var connected: Bool
    var email: String?
}

struct HealthResponse: Codable {
    var status: String
}

struct PolishToken {
    var text: String
    var done: Bool
    var final: PolishDone? = nil
}

struct PolishDone: Codable {
    var recording_id: String
    var transcript: String
    var polished: String
    var model_used: String
    var confidence: Double?
    var audio_id: String?
    var source: String?
    var target_app: String?
    var output_language: String?
    var enriched_transcript: String?
    var examples_used: UInt32
    var latency_ms: PolishLatency
}

struct PolishLatency: Codable {
    var transcribe: Int64
    var embed: Int64
    var retrieve: Int64
    var polish: Int64
    var total: Int64
}

struct TranscriptMeta: Codable {
    var enriched_transcript: String = ""
    var confidence: Double = 0.95
    var mean_word_confidence: Double = 0.95
    var low_confidence_count: Int = 0
    var word_count: Int = 0
    var languages: [String] = []
    var stt_mode: String = "multi"
}

struct StreamingTranscript {
    var transcript: String
    var meta: TranscriptMeta
}

let STREAM_RESET_SENTINEL = "\u{1F}__RESET__\u{1F}"
let LOW_CONFIDENCE_THRESHOLD = 0.85

enum RecordHotkey: String, Codable, CaseIterable {
    case capsLock = "caps_lock"
    case fn = "fn"
    case rightOption = "right_option"

    var label: String {
        switch self {
        case .capsLock: return "Caps Lock"
        case .fn: return "Fn"
        case .rightOption: return "Right Option"
        }
    }
}

enum NotchState {
    case idle
    case recording
    case processing
    case done(String)
    case error(String)
}

enum OutputLanguage: String, CaseIterable {
    case hinglish
    case hindi
    case english

    var label: String {
        switch self {
        case .hinglish: return "Hinglish"
        case .hindi: return "हिंदी"
        case .english: return "English"
        }
    }
}
