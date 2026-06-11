use serde::{Deserialize, Serialize};

pub mod deepgram;
pub mod paths;
pub mod prefs;
pub mod reporter;
pub mod scrub;
pub mod stt;
pub mod telemetry;

// ── Gateway constants ─────────────────────────────────────────────────────────

pub const GATEWAY_BASE: &str = "https://gateway.outreachdeal.com";
pub const VOICE_URL: &str = "https://gateway.outreachdeal.com/v1/voice/polish";

// ── Mode registry ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Mode {
    pub key: &'static str,
    pub label: &'static str,
    pub model: &'static str,
    pub icon: &'static str,
}

pub const MODES: &[Mode] = &[Mode {
    key: "smart",
    label: "Llama 4 Scout (Groq)",
    model: "meta-llama/llama-4-scout-17b-16e-instruct",
    icon: "fast",
}];

pub fn current_mode() -> &'static Mode {
    &MODES[0]
}

pub fn all_modes() -> &'static [Mode] {
    MODES
}

pub fn set_mode(_key: &str) -> Result<&'static Mode, String> {
    Ok(&MODES[0])
}

pub fn mode_label() -> &'static str {
    MODES[0].label
}

/// Returns the Groq Llama 4 Scout model — the default smart model.
pub fn resolve_model(_key_or_model: &str) -> &'static str {
    "meta-llama/llama-4-scout-17b-16e-instruct"
}

pub fn api_key() -> String {
    std::env::var("GATEWAY_API_KEY").unwrap_or_default()
}

pub fn validate_api_key() {
    let key = api_key();
    if key.is_empty() {
        eprintln!("[config] GATEWAY_API_KEY not set in .env");
        std::process::exit(1);
    }
}

// ── Shared data types ─────────────────────────────────────────────────────────

/// A single persisted recording entry — stored in SQLite in Phase B+.
#[derive(Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub timestamp_ms: u64,
    pub polished: String,
    pub word_count: u32,
    pub recording_seconds: f32,
    pub model: String,
    pub transcribe_ms: u64,
    pub polish_ms: u64,
    #[serde(default)]
    pub edit_count: u32,
}

/// Result of a single polish operation.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub transcript: String,
    pub polished: String,
    pub model: String,
    pub confidence: f64,
    pub transcribe_ms: u64,
    pub polish_ms: u64,
}

/// Full state snapshot sent to the Tauri frontend on every command.
#[derive(Serialize)]
pub struct AppSnapshot {
    pub state: String,
    pub platform: String,
    pub current_mode: &'static str,
    pub current_mode_label: &'static str,
    pub current_model: &'static str,
    pub auto_paste_supported: bool,
    pub accessibility_granted: bool,
    pub microphone_granted: bool,
    pub input_monitoring_granted: bool,
    pub modes: Vec<Mode>,
    pub last_result: Option<ProcessSummary>,
    pub last_error: Option<String>,
    pub history: Vec<HistoryItem>,
    pub total_words: u64,
    pub daily_streak: u32,
    pub avg_wpm: u32,
}

// ── .env loader ───────────────────────────────────────────────────────────────

/// Load GATEWAY_API_KEY from .env — three fallback locations:
///   1. Directory of the running executable
///   2. ~/VoicePolish/.env
///   3. Current working directory
pub fn load_env() {
    // 1. Exe dir
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = &exe_dir {
        let _ = dotenvy::from_path(dir.join(".env"));
    }
    // 2. ~/VoicePolish/.env
    if std::env::var("GATEWAY_API_KEY").is_err() {
        if let Ok(home) = std::env::var("HOME") {
            let fallback = std::path::Path::new(&home).join("VoicePolish").join(".env");
            let _ = dotenvy::from_path(fallback);
        }
    }
    // 3. CWD fallback
    let _ = dotenvy::dotenv();
}
