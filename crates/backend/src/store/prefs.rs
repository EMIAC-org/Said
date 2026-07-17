use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{DbPool, now_ms};

fn normalize_record_hotkey(raw: &str) -> String {
    // Canonical ids must match the picker (desktop/src/lib/hotkeys.ts) and the
    // hotkey crate's `RecordHotkey::from_id`. Any sided modifier is valid — an
    // unknown value still falls back to caps_lock so a bad string never bricks
    // recording.
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
        .as_str()
    {
        "caps_lock" | "capslock" => "caps_lock".into(),
        "right_option" | "right_alt" | "rightoption" | "rightalt" => "right_option".into(),
        "fn" | "function" | "globe" => "fn".into(),
        "left_command" | "left_cmd" | "left_meta" | "left_win" | "leftwin" => "left_command".into(),
        "right_command" | "right_cmd" | "right_meta" | "right_win" | "rightwin" => {
            "right_command".into()
        }
        "left_control" | "left_ctrl" | "leftcontrol" => "left_control".into(),
        "right_control" | "right_ctrl" | "rightcontrol" => "right_control".into(),
        "left_option" | "left_alt" | "leftoption" | "leftalt" => "left_option".into(),
        "left_shift" | "leftshift" => "left_shift".into(),
        "right_shift" | "rightshift" => "right_shift".into(),
        _ => "caps_lock".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub user_id: String,
    pub selected_model: String,
    pub tone_preset: String,
    pub custom_prompt: Option<String>,
    pub language: String,
    pub output_language: String, // "hinglish" | "hindi" | "english"
    pub auto_paste: bool,
    pub edit_capture: bool,
    pub polish_text_hotkey: String,
    pub record_hotkey: String,
    pub learning_enabled: bool,
    pub server_runtime_enabled: bool,
    pub server_audio_runtime_enabled: bool,
    pub updated_at: i64,
    // API keys — stored in SQLite, never leave the device
    pub gateway_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub groq_api_key: Option<String>,
    pub deepinfra_api_key: Option<String>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex"
    pub llm_provider: String,
}

/// Partial update payload — all fields optional.
#[derive(Debug, Deserialize, Default)]
pub struct PrefsUpdate {
    pub selected_model: Option<String>,
    pub tone_preset: Option<String>,
    pub custom_prompt: Option<Option<String>>, // Some(None) = clear; None = don't touch
    pub language: Option<String>,
    pub output_language: Option<String>,
    pub auto_paste: Option<bool>,
    pub edit_capture: Option<bool>,
    pub polish_text_hotkey: Option<String>,
    pub record_hotkey: Option<String>,
    pub learning_enabled: Option<bool>,
    pub server_runtime_enabled: Option<bool>,
    pub server_audio_runtime_enabled: Option<bool>,
    // API keys — Some(None) = clear; None = don't touch; Some(Some(s)) = set
    pub gateway_api_key: Option<Option<String>>,
    pub gemini_api_key: Option<Option<String>>,
    pub groq_api_key: Option<Option<String>>,
    pub deepinfra_api_key: Option<Option<String>>,
    /// LLM provider: "gateway" | "gemini_direct" | "groq" | "openai_codex"
    pub llm_provider: Option<String>,
}

pub fn normalize_selected_model(raw: &str) -> String {
    said_core::polish::model::normalize_selected_model(raw)
}

pub fn validate_polish_model_key(raw: &str) -> String {
    said_core::polish::model::validate_polish_model_key(raw)
}

/// Server polish runtime is always enabled — polish routes through control-plane.
pub fn server_runtime_forced() -> bool {
    true
}

pub fn get_prefs(pool: &DbPool, user_id: &str) -> Option<Preferences> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT user_id, selected_model, tone_preset, custom_prompt, language,
                output_language, auto_paste, edit_capture, polish_text_hotkey, record_hotkey,
                learning_enabled, server_runtime_enabled, 0 AS server_audio_runtime_enabled, updated_at,
                gateway_api_key, gemini_api_key, llm_provider,
                groq_api_key, deepinfra_api_key
         FROM preferences WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(Preferences {
                user_id: row.get(0)?,
                selected_model: validate_polish_model_key(&row.get::<_, String>(1)?),
                tone_preset: row.get(2)?,
                custom_prompt: row.get(3)?,
                language: row.get(4)?,
                output_language: row
                    .get::<_, Option<String>>(5)?
                    .unwrap_or_else(|| "hinglish".into()),
                auto_paste: row.get::<_, i64>(6)? != 0,
                edit_capture: row.get::<_, i64>(7)? != 0,
                polish_text_hotkey: row.get(8)?,
                record_hotkey: normalize_record_hotkey(
                    &row.get::<_, Option<String>>(9)?
                        .unwrap_or_else(|| "caps_lock".into()),
                ),
                learning_enabled: row.get::<_, i64>(10)? != 0,
                server_runtime_enabled: server_runtime_forced(),
                server_audio_runtime_enabled: row.get::<_, i64>(12)? != 0,
                updated_at: row.get(13)?,
                gateway_api_key: row.get(14)?,
                gemini_api_key: row.get(15)?,
                llm_provider: {
                    let raw = row.get::<_, Option<String>>(16)?.unwrap_or_default();
                    if raw.is_empty() || raw == "gateway" {
                        "groq".into()
                    } else {
                        raw
                    }
                },
                groq_api_key: row.get(17)?,
                deepinfra_api_key: row.get(18)?,
            })
        },
    )
    .ok()
}

pub fn update_prefs(pool: &DbPool, user_id: &str, update: PrefsUpdate) -> Option<Preferences> {
    let conn = pool.get().ok()?;
    let now = now_ms();

    if let Some(v) = update.selected_model {
        let v = validate_polish_model_key(&v);
        conn.execute(
            "UPDATE preferences SET selected_model = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.tone_preset {
        conn.execute(
            "UPDATE preferences SET tone_preset = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.custom_prompt {
        conn.execute(
            "UPDATE preferences SET custom_prompt = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.language {
        conn.execute(
            "UPDATE preferences SET language = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.output_language {
        conn.execute(
            "UPDATE preferences SET output_language = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.auto_paste {
        conn.execute(
            "UPDATE preferences SET auto_paste = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v as i64, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.edit_capture {
        conn.execute(
            "UPDATE preferences SET edit_capture = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v as i64, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.polish_text_hotkey {
        conn.execute(
            "UPDATE preferences SET polish_text_hotkey = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.record_hotkey {
        let v = normalize_record_hotkey(&v);
        conn.execute(
            "UPDATE preferences SET record_hotkey = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.learning_enabled {
        conn.execute(
            "UPDATE preferences SET learning_enabled = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v as i64, now, user_id],
        )
        .ok()?;
    }
    if let Some(_v) = update.server_runtime_enabled {
        // Server runtime is always on — ignore client attempts to disable.
        conn.execute(
            "UPDATE preferences SET server_runtime_enabled = 1, updated_at = ?1 WHERE user_id = ?2",
            params![now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.server_audio_runtime_enabled {
        // Legacy field kept in API structs for old desktop builds. The SQLite
        // column was removed, and local server-audio runtime must stay disabled.
        let _ = v;
    }
    if let Some(v) = update.gateway_api_key {
        conn.execute(
            "UPDATE preferences SET gateway_api_key = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.gemini_api_key {
        conn.execute(
            "UPDATE preferences SET gemini_api_key = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.groq_api_key {
        conn.execute(
            "UPDATE preferences SET groq_api_key = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.deepinfra_api_key {
        conn.execute(
            "UPDATE preferences SET deepinfra_api_key = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.llm_provider {
        let v = match v.trim() {
            "" | "gateway" => "groq".to_string(),
            other => other.to_string(),
        };
        conn.execute(
            "UPDATE preferences SET llm_provider = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    get_prefs(pool, user_id)
}

#[cfg(test)]
mod tests {
    use super::{normalize_record_hotkey, normalize_selected_model, validate_polish_model_key};

    #[test]
    fn normalizes_smart_model_aliases_to_deepinfra_gemma_4() {
        assert_eq!(
            normalize_selected_model("smart"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            normalize_selected_model("maverick"),
            "deepinfra-gemma-4-26b-a4b"
        );
    }

    #[test]
    fn validate_replaces_retired_catalog_keys() {
        assert_eq!(
            validate_polish_model_key("phi4"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            validate_polish_model_key("groq-scout"),
            "deepinfra-gemma-4-26b-a4b"
        );
    }

    #[test]
    fn normalize_replaces_retired_catalog_keys() {
        assert_eq!(
            normalize_selected_model("phi4"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            normalize_selected_model("groq-scout"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            normalize_selected_model("groq-70b"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            normalize_selected_model("legacy-provider-model"),
            "deepinfra-gemma-4-26b-a4b"
        );
    }

    #[test]
    fn normalizes_scout_alias_to_current_key() {
        assert_eq!(
            validate_polish_model_key("scout"),
            "deepinfra-gemma-4-26b-a4b"
        );
    }

    #[test]
    fn normalizes_fast_model_aliases_to_current_key() {
        assert_eq!(
            normalize_selected_model("fast"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            normalize_selected_model("llama-3.1-8b-instant"),
            "deepinfra-gemma-4-26b-a4b"
        );
        assert_eq!(
            normalize_selected_model("deepseek"),
            "deepinfra-gemma-4-26b-a4b"
        );
    }

    #[test]
    fn normalizes_record_hotkey_values() {
        assert_eq!(normalize_record_hotkey("caps_lock"), "caps_lock");
        assert_eq!(normalize_record_hotkey("Caps Lock"), "caps_lock");
        assert_eq!(normalize_record_hotkey("right-option"), "right_option");
        assert_eq!(normalize_record_hotkey("right_alt"), "right_option");
        assert_eq!(normalize_record_hotkey("Function"), "fn");
        assert_eq!(normalize_record_hotkey("globe"), "fn");
        // Sided modifiers (the picker's expanded set) round-trip unchanged.
        assert_eq!(normalize_record_hotkey("right_shift"), "right_shift");
        assert_eq!(normalize_record_hotkey("Right Shift"), "right_shift");
        assert_eq!(normalize_record_hotkey("left_control"), "left_control");
        assert_eq!(normalize_record_hotkey("right_command"), "right_command");
        assert_eq!(normalize_record_hotkey("right_win"), "right_command");
        assert_eq!(normalize_record_hotkey("left_alt"), "left_option");
    }

    #[test]
    fn invalid_record_hotkey_falls_back_to_caps_lock() {
        assert_eq!(normalize_record_hotkey("space"), "caps_lock");
        assert_eq!(normalize_record_hotkey(""), "caps_lock");
    }
}
