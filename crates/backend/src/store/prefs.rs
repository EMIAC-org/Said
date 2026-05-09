use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{DbPool, now_ms};

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
    pub updated_at: i64,
    // API keys — stored in SQLite, never leave the device
    pub gateway_api_key: Option<String>,
    pub deepgram_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub groq_api_key: Option<String>,
    /// LLM routing: "gateway" (default) | "gemini_direct" | "groq" | "openai_codex"
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
    // API keys — Some(None) = clear; None = don't touch; Some(Some(s)) = set
    pub gateway_api_key: Option<Option<String>>,
    pub deepgram_api_key: Option<Option<String>>,
    pub gemini_api_key: Option<Option<String>>,
    pub groq_api_key: Option<Option<String>>,
    /// LLM provider: "gateway" | "gemini_direct" | "groq" | "openai_codex"
    pub llm_provider: Option<String>,
}

pub fn get_prefs(pool: &DbPool, user_id: &str) -> Option<Preferences> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT user_id, selected_model, tone_preset, custom_prompt, language,
                output_language, auto_paste, edit_capture, polish_text_hotkey, record_hotkey,
                learning_enabled, updated_at,
                gateway_api_key, deepgram_api_key, gemini_api_key, llm_provider, groq_api_key
         FROM preferences WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(Preferences {
                user_id: row.get(0)?,
                selected_model: row.get(1)?,
                tone_preset: row.get(2)?,
                custom_prompt: row.get(3)?,
                language: row.get(4)?,
                output_language: row
                    .get::<_, Option<String>>(5)?
                    .unwrap_or_else(|| "hinglish".into()),
                auto_paste: row.get::<_, i64>(6)? != 0,
                edit_capture: row.get::<_, i64>(7)? != 0,
                polish_text_hotkey: row.get(8)?,
                record_hotkey: row
                    .get::<_, Option<String>>(9)?
                    .unwrap_or_else(|| "caps_lock".into()),
                learning_enabled: row.get::<_, i64>(10)? != 0,
                updated_at: row.get(11)?,
                gateway_api_key: row.get(12)?,
                deepgram_api_key: row.get(13)?,
                gemini_api_key: row.get(14)?,
                llm_provider: row
                    .get::<_, Option<String>>(15)?
                    .unwrap_or_else(|| "openai_codex".into()),
                groq_api_key: row.get(16)?,
            })
        },
    )
    .ok()
}

pub fn update_prefs(pool: &DbPool, user_id: &str, update: PrefsUpdate) -> Option<Preferences> {
    let conn = pool.get().ok()?;
    let now = now_ms();

    if let Some(v) = update.selected_model {
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
    if let Some(v) = update.llm_provider {
        conn.execute(
            "UPDATE preferences SET llm_provider = ?1, updated_at = ?2 WHERE user_id = ?3",
            params![v, now, user_id],
        )
        .ok()?;
    }

    get_prefs(pool, user_id)
}
