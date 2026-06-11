use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;

use crate::store::{DbPool, openai_oauth, prefs::Preferences};

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

fn has_deepgram_key(prefs: &Preferences) -> bool {
    said_core::stt::resolve_deepgram_api_key(prefs.deepgram_api_key.as_deref()).is_some()
}

fn has_sarvam_key(prefs: &Preferences) -> bool {
    said_core::stt::resolve_sarvam_api_key(prefs.sarvam_api_key.as_deref()).is_some()
}

pub fn effective_stt_provider(prefs: &Preferences) -> String {
    said_core::stt::resolve_effective_stt_provider(
        &prefs.stt_provider,
        has_sarvam_key(prefs),
        has_deepgram_key(prefs),
    )
}

fn has_stt_key(prefs: &Preferences) -> bool {
    let effective = effective_stt_provider(prefs);
    if said_core::stt::is_sarvam(&effective) {
        has_sarvam_key(prefs)
    } else {
        has_deepgram_key(prefs)
    }
}

fn has_llm_credential(pool: &DbPool, user_id: &str, prefs: &Preferences) -> bool {
    let gateway_key = non_empty(prefs.gateway_api_key.clone())
        .or_else(|| non_empty(std::env::var("GATEWAY_API_KEY").ok()))
        .or_else(|| {
            let key = said_core::api_key();
            if key.trim().is_empty() {
                None
            } else {
                Some(key)
            }
        });
    let gemini_key = non_empty(prefs.gemini_api_key.clone())
        .or_else(|| non_empty(std::env::var("GEMINI_API_KEY").ok()));
    let groq_key = non_empty(prefs.groq_api_key.clone())
        .or_else(|| non_empty(std::env::var("GROQ_API_KEY").ok()));

    gateway_key.is_some()
        || gemini_key.is_some()
        || groq_key.is_some()
        || openai_oauth::get_token(pool, user_id).is_some()
}

pub fn missing_voice_api_keys(
    pool: &DbPool,
    user_id: &str,
    prefs: &Preferences,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !has_stt_key(prefs) {
        let effective = effective_stt_provider(prefs);
        missing.push(if said_core::stt::is_sarvam(&effective) {
            "sarvam"
        } else {
            "deepgram"
        });
    }
    if !has_llm_credential(pool, user_id, prefs) {
        missing.push("llm");
    }
    missing
}

pub fn missing_message_polish_voice_keys(prefs: &Preferences) -> Vec<&'static str> {
    if has_stt_key(prefs) {
        Vec::new()
    } else {
        let effective = effective_stt_provider(prefs);
        if said_core::stt::is_sarvam(&effective) {
            vec!["sarvam"]
        } else {
            vec!["deepgram"]
        }
    }
}

pub fn missing_text_api_keys(
    pool: &DbPool,
    user_id: &str,
    prefs: &Preferences,
) -> Vec<&'static str> {
    if has_llm_credential(pool, user_id, prefs) {
        Vec::new()
    } else {
        vec!["llm"]
    }
}

pub fn missing_api_keys_response(missing: Vec<&'static str>) -> axum::response::Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "error_code": "missing_api_keys",
            "message": "API keys required",
            "missing": missing,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::prefs::Preferences;

    fn base_prefs(stt_provider: &str) -> Preferences {
        Preferences {
            user_id: "u".into(),
            selected_model: "fast".into(),
            tone_preset: "neutral".into(),
            custom_prompt: None,
            language: "auto".into(),
            output_language: "hinglish".into(),
            auto_paste: true,
            edit_capture: true,
            polish_text_hotkey: "cmd+shift+p".into(),
            record_hotkey: "caps_lock".into(),
            learning_enabled: true,
            server_runtime_enabled: false,
            server_audio_runtime_enabled: false,
            updated_at: 0,
            gateway_api_key: Some("gsk_test".into()),
            deepgram_api_key: Some("dg_test".into()),
            sarvam_api_key: None,
            gemini_api_key: None,
            groq_api_key: None,
            cerebras_api_key: None,
            llm_provider: "groq".into(),
            stt_provider: stt_provider.into(),
        }
    }

    #[test]
    fn sarvam_pref_with_key_stays_sarvam() {
        let mut prefs = base_prefs("sarvam");
        prefs.sarvam_api_key = Some("sk_test".into());
        assert_eq!(effective_stt_provider(&prefs), "sarvam");
    }
}
