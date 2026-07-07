use serde::{Deserialize, Serialize};

pub mod deepgram;
pub mod dictation_trace;
pub mod paths;
pub mod polish;
pub mod prefs;
pub mod preprocess;
pub mod redecode_flagging;
pub mod reporter;
pub mod script;
pub mod scrub;
pub mod stt;
pub mod telemetry;
pub mod text;

// ── Gateway constants ─────────────────────────────────────────────────────────

pub const GATEWAY_BASE: &str = "https://gateway.outreachdeal.com";
pub const VOICE_URL: &str = "https://gateway.outreachdeal.com/v1/voice/polish";

// One build-time switch for the AirNote control-plane used by desktop/server
// runtime defaults. For this dev-connected desktop build, point at dev.
pub const AIRNOTE_DEFAULT_CONTROL_PLANE_URL: &str = "https://airnote-dev.103.180.163.41.sslip.io";

// ── Mode registry ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Mode {
    pub key: &'static str,
    pub label: &'static str,
    pub model: &'static str,
    pub icon: &'static str,
}

pub const MODES: &[Mode] = &[Mode {
    key: "cerebras-gpt-oss",
    label: "GPT OSS 120B (Cerebras)",
    model: polish::model::CEREBRAS_POLISH_MODEL_GPT_OSS,
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

/// Returns the polish model route (Groq GPT OSS 120B for smart tier).
pub fn resolve_model(key_or_model: &str) -> String {
    polish::model::resolve_polish_route(key_or_model).label()
}

/// Gateway/LLM key baked into the build at compile time (set `GATEWAY_API_KEY`
/// in the build environment; `build-dmg.sh` exports it from `.env`). The shipped
/// app ships with a working key so end users never enter one. Captured at
/// compile time, never written to a tracked file — never committed to git.
const BUNDLED_GATEWAY_API_KEY: Option<&str> = option_env!("GATEWAY_API_KEY");

/// Gateway/LLM key: runtime env (dev/server) → build-time bundled key (shipped app).
pub fn api_key() -> String {
    std::env::var("GATEWAY_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            BUNDLED_GATEWAY_API_KEY
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_default()
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
#[derive(Clone, Serialize)]
pub struct AppSnapshot {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    pub platform: String,
    pub current_mode: &'static str,
    pub current_mode_label: &'static str,
    pub current_model: &'static str,
    #[serde(default)]
    pub message_polish_mode: bool,
    pub auto_paste_supported: bool,
    pub accessibility_granted: bool,
    pub microphone_granted: bool,
    pub input_monitoring_granted: bool,
    /// macOS Screen Recording (gates ScreenCaptureKit → meeting system-audio
    /// capture). Always true on platforms without this permission.
    #[serde(default)]
    pub screen_recording_granted: bool,
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
