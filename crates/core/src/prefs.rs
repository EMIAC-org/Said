//! Cross-process desktop prefs persisted as a small JSON file at
//! `<data_dir>/desktop_prefs.json`.
//!
//! These are settings that must be read **synchronously at process startup**
//! — before the backend daemon is reachable. Today that's two of them:
//!
//!   - `sentry_disabled` — the Diagnostics toggle. Sentry must initialize
//!     before tracing-subscriber is built, which is well before the backend
//!     HTTP server comes up. So the pref lives in a file the desktop process
//!     can `read_to_string` synchronously.
//!   - `update_channel` — Tauri's `tauri-plugin-updater` reads its endpoint
//!     list at builder time during app setup, also before the backend is
//!     ready.
//!
//! Everything else (the rich preferences struct: hotkey choice, custom
//! prompts, language, lexicon, etc.) lives in the backend's SQLite DB and is
//! fetched over HTTP after the daemon comes up. Don't add fields here unless
//! they truly need to be read before the backend exists.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::paths;

const FILE_NAME: &str = "desktop_prefs.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopPrefs {
    /// When `true`, Sentry init returns `None` and zero telemetry events are
    /// sent. Defaults to `false` (opt-out semantics; on by default per the
    /// locked v3.0 decision).
    #[serde(default)]
    pub sentry_disabled: bool,

    /// Auto-updater channel: `"stable"` (default) or `"beta"`. Picks which
    /// manifest URL the updater plugin polls. Until manifests-branch
    /// publishing lands (v3.x), the `"beta"` value is functionally a no-op
    /// because GitHub Releases excludes prereleases from `/latest/download`.
    #[serde(default = "default_channel")]
    pub update_channel: String,
}

fn default_channel() -> String {
    "stable".into()
}

impl Default for DesktopPrefs {
    fn default() -> Self {
        Self {
            sentry_disabled: false,
            update_channel: default_channel(),
        }
    }
}

fn prefs_path() -> PathBuf {
    paths::data_dir().join(FILE_NAME)
}

/// Read the persisted prefs. Returns defaults if the file is missing,
/// unparseable, or unreadable — telemetry / updater paths must never panic
/// on a fresh install or a corrupted prefs file.
pub fn load() -> DesktopPrefs {
    let path = prefs_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return DesktopPrefs::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Atomically persist the prefs file. Creates the parent dir if needed.
/// Writes through a temp file + rename so a concurrent reader never sees a
/// half-written JSON document.
pub fn save(prefs: &DesktopPrefs) -> Result<(), String> {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create data dir: {e}"))?;
    }
    let text = serde_json::to_string_pretty(prefs).map_err(|e| format!("serialize prefs: {e}"))?;

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write prefs tmp: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename prefs tmp: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefs_are_opt_out_diagnostics_on() {
        let d = DesktopPrefs::default();
        assert!(
            !d.sentry_disabled,
            "Sentry is opt-out (on by default) per the v3.0 locked decision"
        );
        assert_eq!(d.update_channel, "stable");
    }

    #[test]
    fn missing_fields_round_trip_to_defaults() {
        // Older clients writing a partial file shouldn't break newer parsers.
        let partial = r#"{}"#;
        let p: DesktopPrefs = serde_json::from_str(partial).unwrap();
        assert!(!p.sentry_disabled);
        assert_eq!(p.update_channel, "stable");

        let partial = r#"{ "sentry_disabled": true }"#;
        let p: DesktopPrefs = serde_json::from_str(partial).unwrap();
        assert!(p.sentry_disabled);
        assert_eq!(p.update_channel, "stable");
    }

    #[test]
    fn full_serialization_round_trip() {
        let prefs = DesktopPrefs {
            sentry_disabled: true,
            update_channel: "beta".into(),
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let back: DesktopPrefs = serde_json::from_str(&json).unwrap();
        assert!(back.sentry_disabled);
        assert_eq!(back.update_channel, "beta");
    }
}
