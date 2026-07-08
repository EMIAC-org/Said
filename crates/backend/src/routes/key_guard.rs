use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;

use crate::store::{DbPool, openai_oauth, prefs::Preferences};

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

pub fn effective_stt_provider(prefs: &Preferences) -> String {
    // Dev/testing override mirrors said_core::stt::effective_dictation_provider so
    // AIRNOTE_FORCE_STT_PROVIDER=whisper_local routes dictation to on-device
    // whisper.cpp here in the backend too.
    if let Ok(forced) = std::env::var("AIRNOTE_FORCE_STT_PROVIDER") {
        let norm = said_core::stt::normalize_toggle_stt_provider(&forced);
        if !norm.is_empty() {
            return norm;
        }
    }
    said_core::stt::resolve_provider_from_pref(&prefs.stt_provider)
}

fn has_stt_key(prefs: &Preferences) -> bool {
    let provider = effective_stt_provider(prefs);
    if said_core::stt::is_deepgram(&provider) {
        // Cloud dictation = DeepInfra Whisper Large V3 (server-managed key).
        crate::stt::deepinfra_whisper::resolve_api_key().is_some()
    } else {
        // Local engines (whisper.cpp / Swift) need no key.
        true
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
    _pool: &DbPool,
    _user_id: &str,
    prefs: &Preferences,
    require_stt_key: bool,
) -> Vec<&'static str> {
    // The local backend only validates the cloud STT key. Polish always runs
    // server-side (Cerebras via the server runtime), so it needs no local LLM key —
    // gating on one here just blocks keyless shipped builds before the request can
    // even reach the server. (STT still needs a key when cloud Whisper is used;
    // local whisper/swift and server-managed cloud STT set require_stt_key=false.)
    let mut missing = Vec::new();
    if require_stt_key && !has_stt_key(prefs) {
        missing.push("deepgram");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(stt_provider: &str, deepgram_api_key: Option<String>) -> Preferences {
        Preferences {
            user_id: "test-user".into(),
            selected_model: "smart".into(),
            tone_preset: "professional".into(),
            custom_prompt: None,
            language: "hi".into(),
            output_language: "hinglish".into(),
            auto_paste: true,
            edit_capture: true,
            polish_text_hotkey: "option_space".into(),
            record_hotkey: "caps_lock".into(),
            learning_enabled: true,
            server_runtime_enabled: true,
            server_audio_runtime_enabled: false,
            updated_at: 0,
            gateway_api_key: None,
            deepgram_api_key,
            gemini_api_key: None,
            groq_api_key: None,
            cerebras_api_key: None,
            deepinfra_api_key: None,
            llm_provider: "groq".into(),
            stt_provider: stt_provider.into(),
        }
    }

    #[test]
    fn swift_local_stt_does_not_require_deepgram_key() {
        assert!(has_stt_key(&prefs("swift_local", None)));
    }

    #[test]
    fn cloud_stt_requires_deepinfra_key() {
        // Cloud dictation now runs on DeepInfra Whisper (server-managed key), so
        // its availability tracks DEEPINFRA_API_KEY, not a user Deepgram key.
        let env_has_deepinfra = std::env::var("DEEPINFRA_API_KEY")
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        assert_eq!(has_stt_key(&prefs("deepgram", None)), env_has_deepinfra);
    }
}
