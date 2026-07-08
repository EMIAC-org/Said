//! Polish model catalog API for Settings UI.

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};

use crate::store::prefs::Preferences;
use crate::{AppState, get_prefs_cached};

#[derive(Debug, Deserialize)]
pub struct ListModelsQuery {
    #[serde(default)]
    pub beta: bool,
}

#[derive(Debug, Serialize)]
pub struct PolishModelEntry {
    pub key: String,
    pub label: String,
    pub provider: String,
    pub model_id: String,
    pub beta_only: bool,
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<PolishModelEntry>,
    pub selected_model: String,
}

fn model_selectable(_provider: &str, _prefs: &Preferences) -> bool {
    // Polish always runs server-side; provider keys live on control-plane .env.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_prefs() -> Preferences {
        Preferences {
            user_id: "u".into(),
            selected_model: said_core::polish::model::DEFAULT_POLISH_MODEL_KEY.into(),
            tone_preset: "neutral".into(),
            custom_prompt: None,
            language: "auto".into(),
            output_language: "hinglish".into(),
            auto_paste: true,
            edit_capture: true,
            polish_text_hotkey: "cmd+shift+p".into(),
            record_hotkey: "caps_lock".into(),
            learning_enabled: true,
            server_runtime_enabled: true,
            server_audio_runtime_enabled: false,
            updated_at: 0,
            gateway_api_key: None,
            gemini_api_key: None,
            groq_api_key: None,
            cerebras_api_key: None,
            deepinfra_api_key: None,
            llm_provider: "groq".into(),
        }
    }

    #[test]
    fn all_catalog_models_selectable() {
        let prefs = empty_prefs();
        assert!(model_selectable("groq", &prefs));
        assert!(model_selectable("deepinfra", &prefs));
        assert!(model_selectable("cerebras", &prefs));
    }
}

pub async fn list_models(
    State(state): State<AppState>,
    Query(query): Query<ListModelsQuery>,
) -> Json<ListModelsResponse> {
    let user_id = state.default_user_id.clone();
    let prefs = get_prefs_cached(&state.prefs_cache, &state.pool, &user_id)
        .await
        .unwrap_or_else(|| Preferences {
            user_id: user_id.to_string(),
            selected_model: said_core::polish::model::DEFAULT_POLISH_MODEL_KEY.to_string(),
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
            gateway_api_key: None,
            gemini_api_key: None,
            groq_api_key: None,
            cerebras_api_key: None,
            deepinfra_api_key: None,
            llm_provider: "groq".into(),
        });

    let models = said_core::polish::model::list_polish_models(query.beta)
        .into_iter()
        .map(|spec| PolishModelEntry {
            key: spec.key.to_string(),
            label: spec.label.to_string(),
            provider: spec.provider.to_string(),
            model_id: spec.model_id.to_string(),
            beta_only: spec.beta_only,
            available: model_selectable(spec.provider, &prefs),
        })
        .collect();

    Json(ListModelsResponse {
        models,
        selected_model: prefs.selected_model,
    })
}
