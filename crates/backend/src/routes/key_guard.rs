use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;

use crate::store::{DbPool, openai_oauth, prefs::Preferences};

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

fn has_deepgram_key(prefs: &Preferences) -> bool {
    said_core::stt::resolve_deepgram_api_key(prefs.deepgram_api_key.as_deref()).is_some()
}

pub fn effective_stt_provider(prefs: &Preferences) -> String {
    said_core::stt::resolve_provider_from_pref(&prefs.stt_provider)
}

fn has_stt_key(prefs: &Preferences) -> bool {
    has_deepgram_key(prefs)
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
    require_stt_key: bool,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if require_stt_key && !has_stt_key(prefs) {
        missing.push("deepgram");
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
        vec!["deepgram"]
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
