//! Per-platform filesystem locations for AirNote. Wraps the `dirs` crate with
//! AirNote-specific subfolder names and the platform-conventional log dir.
//!
//! macOS keeps logs at `~/Library/Logs/AirNote` (Apple's HIG convention).
//! Other platforms use `dirs::cache_dir()` + `AirNote/logs`.
//!
//! Data lives under `VoicePolish/` for backwards compatibility with v2.x
//! installs; renaming this data dir is deferred to a future major version that
//! owns the migration of existing user databases.

use std::path::PathBuf;

const APP_NAME: &str = "AirNote";
const LEGACY_DATA_SUBDIR: &str = "VoicePolish";

/// Directory where AirNote writes log files. Created on first write by callers.
///
/// - macOS: `~/Library/Logs/AirNote`
/// - Windows: `%LOCALAPPDATA%\AirNote\logs`
/// - Linux: `$XDG_CACHE_HOME/AirNote/logs` (or `~/.cache/AirNote/logs`)
/// - Fallback: `<tempdir>/AirNote/logs`
pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            return home.join("Library").join("Logs").join(APP_NAME);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(cache) = dirs::cache_dir() {
            return cache.join(APP_NAME).join("logs");
        }
    }
    std::env::temp_dir().join(APP_NAME).join("logs")
}

/// Per-user data directory for persistent AirNote state (SQLite DB, device id,
/// retention-managed audio snippets). Created on first write by callers.
///
/// - macOS: `~/Library/Application Support/VoicePolish`
/// - Windows: `%APPDATA%\VoicePolish`
/// - Linux: `$XDG_DATA_HOME/VoicePolish` (or `~/.local/share/VoicePolish`)
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join(LEGACY_DATA_SUBDIR)
}

/// Default SQLite database path. Backwards-compatible with the v2.x macOS layout.
pub fn default_db_path() -> PathBuf {
    data_dir().join("db.sqlite")
}

/// Path to the local whisper.cpp ggml model file for offline STT.
/// `<data_dir>/models/ggml-tiny.bin`
pub fn whisper_model_path() -> PathBuf {
    data_dir().join("models").join("ggml-tiny.bin")
}

/// Directory for the Oriserve Swift HF model weights (downloaded on demand).
/// `<data_dir>/models/oriserve-swift/`
pub fn swift_model_dir() -> PathBuf {
    data_dir().join("models").join("oriserve-swift")
}

/// Marker file indicating the Swift model download completed.
pub fn swift_model_weights_path() -> PathBuf {
    swift_model_dir().join("model.safetensors")
}

/// Stable anonymous device ID. Generated once on first call and persisted to
/// `<data_dir>/device_id`. Used to deduplicate Sentry crash reports without
/// tying them to any user-controlled identifier. Deleting the file or the
/// containing data dir resets the ID.
///
/// Best-effort: if the data dir is not writable, returns a fresh UUID per
/// call and telemetry dedup degrades.
pub fn device_id() -> String {
    let path = data_dir().join("device_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &id);
    id
}
