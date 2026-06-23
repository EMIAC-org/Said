use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{DbPool, now_ms};

fn normalize_record_hotkey(raw: &str) -> String {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
        .as_str()
    {
        "caps_lock" | "capslock" => "caps_lock".into(),
        "right_option" | "right_alt" | "rightoption" | "rightalt" => "right_option".into(),
        "fn" | "function" | "globe" => "fn".into(),
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
    pub deepgram_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub groq_api_key: Option<String>,
    pub cerebras_api_key: Option<String>,
    /// LLM routing: "gateway" | "gemini_direct" | "groq" | "cerebras" | "openai_codex"
    pub llm_provider: String,
    /// STT routing: "deepgram" | "whisper_local" | "groq_whisper"
    pub stt_provider: String,
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
    pub deepgram_api_key: Option<Option<String>>,
    pub gemini_api_key: Option<Option<String>>,
    pub groq_api_key: Option<Option<String>>,
    pub cerebras_api_key: Option<Option<String>>,
    /// LLM provider: "gateway" | "gemini_direct" | "groq" | "cerebras" | "openai_codex"
    pub llm_provider: Option<String>,
    /// STT provider: "deepgram" | "whisper_local" | "groq_whisper"
    pub stt_provider: Option<String>,
}

pub fn normalize_selected_model(raw: &str) -> String {
    let model = raw.trim().to_ascii_lowercase();
    if model == "smart" || model.contains("maverick") || model.contains("scout") {
        "smart".into()
    } else if model == "deepseek"
        || model == "fast"
        || model.contains("8b")
        || model.contains("instant")
    {
        "fast".into()
    } else {
        "fast".into()
    }
}

pub fn get_prefs(pool: &DbPool, user_id: &str) -> Option<Preferences> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT user_id, selected_model, tone_preset, custom_prompt, language,
                output_language, auto_paste, edit_capture, polish_text_hotkey, record_hotkey,
                learning_enabled, server_runtime_enabled, 0 AS server_audio_runtime_enabled, updated_at,
                gateway_api_key, deepgram_api_key, gemini_api_key, llm_provider,
                groq_api_key, cerebras_api_key, stt_provider
         FROM preferences WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(Preferences {
                user_id: row.get(0)?,
                selected_model: normalize_selected_model(&row.get::<_, String>(1)?),
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
                server_runtime_enabled: row.get::<_, i64>(11)? != 0,
                server_audio_runtime_enabled: row.get::<_, i64>(12)? != 0,
                updated_at: row.get(13)?,
                gateway_api_key: row.get(14)?,
                deepgram_api_key: row.get(15)?,
                gemini_api_key: row.get(16)?,
                llm_provider: {
                    let raw = row.get::<_, Option<String>>(17)?.unwrap_or_default();
                    if raw.is_empty() || raw == "gateway" {
                        "groq".into()
                    } else {
                        raw
                    }
                },
                groq_api_key: row.get(18)?,
                cerebras_api_key: row.get(19)?,
                stt_provider: said_core::stt::normalize_toggle_stt_provider(
                    &row.get::<_, Option<String>>(20)?
                        .unwrap_or_else(|| "deepgram".into()),
                ),
            })
        },
    )
    .ok()
}

pub fn update_prefs(pool: &DbPool, user_id: &str, update: PrefsUpdate) -> Option<Preferences> {
    let conn = pool.get().ok()?;
    let now = now_ms();

    if let Some(v) = update.selected_model {
        let v = normalize_selected_model(&v);
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
    if let Some(v) = update.server_runtime_enabled {
        conn.execute(
            "UPDATE preferences SET server_runtime_enabled = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v as i64, now, user_id],
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
    if let Some(v) = update.deepgram_api_key {
        conn.execute(
            "UPDATE preferences SET deepgram_api_key = ?1, updated_at = ?2 WHERE user_id = ?3",
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
    if let Some(v) = update.cerebras_api_key {
        conn.execute(
            "UPDATE preferences SET cerebras_api_key = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.llm_provider {
        conn.execute(
            "UPDATE preferences SET llm_provider = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }
    if let Some(v) = update.stt_provider {
        let v = said_core::stt::normalize_toggle_stt_provider(&v);
        conn.execute(
            "UPDATE preferences SET stt_provider = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }

    get_prefs(pool, user_id)
}

#[cfg(test)]
mod tests {
    use super::{normalize_record_hotkey, normalize_selected_model};

    #[test]
    fn normalizes_smart_model_aliases_to_smart() {
        assert_eq!(normalize_selected_model("smart"), "smart");
        assert_eq!(normalize_selected_model("maverick"), "smart");
        assert_eq!(
            normalize_selected_model("meta-llama/llama-4-maverick-17b-128e-instruct"),
            "smart"
        );
        assert_eq!(
            normalize_selected_model("meta-llama/llama-4-scout-17b-16e-instruct"),
            "smart"
        );
    }

    #[test]
    fn normalizes_fast_model_aliases_to_fast() {
        assert_eq!(normalize_selected_model("fast"), "fast");
        assert_eq!(normalize_selected_model("llama-3.1-8b-instant"), "fast");
        assert_eq!(normalize_selected_model("deepseek"), "fast");
    }

    #[test]
    fn normalizes_record_hotkey_values() {
        assert_eq!(normalize_record_hotkey("caps_lock"), "caps_lock");
        assert_eq!(normalize_record_hotkey("Caps Lock"), "caps_lock");
        assert_eq!(normalize_record_hotkey("right-option"), "right_option");
        assert_eq!(normalize_record_hotkey("right_alt"), "right_option");
        assert_eq!(normalize_record_hotkey("Function"), "fn");
        assert_eq!(normalize_record_hotkey("globe"), "fn");
    }

    #[test]
    fn invalid_record_hotkey_falls_back_to_caps_lock() {
        assert_eq!(normalize_record_hotkey("space"), "caps_lock");
        assert_eq!(normalize_record_hotkey(""), "caps_lock");
    }
}
