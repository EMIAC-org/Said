use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::warn;

use crate::{
    AppState,
    llm::{
        gateway, gemini_direct, groq, openai_codex,
        prompt::{
            VOICE_PROMPT_BASE_VERSION, VOICE_PROMPT_KIND, VOICE_PROMPT_TITLE, build_user_message,
            default_voice_prompt_template, render_voice_system_prompt_template,
        },
        script,
    },
    store::{
        openai_oauth,
        prompt_templates::{self, DefaultPrompt, PromptTemplate},
    },
};

const MAX_PROMPT_BYTES: usize = 80_000;

#[derive(Debug, Serialize)]
pub struct PromptTemplateResponse {
    pub kind: String,
    pub title: String,
    pub base_version: String,
    pub active_body: String,
    pub draft_body: Option<String>,
    pub default_body: String,
    pub updated_at: i64,
    pub applied_at: Option<i64>,
    pub has_draft: bool,
    pub active_is_default: bool,
}

#[derive(Debug, Deserialize)]
pub struct SaveDraftRequest {
    pub draft_body: String,
}

#[derive(Debug, Deserialize)]
pub struct TestPromptRequest {
    pub transcript: String,
    #[serde(default)]
    pub draft_body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestPromptResponse {
    pub output: String,
    pub model_used: String,
    pub latency_ms: i64,
}

pub async fn get_voice_prompt(State(state): State<AppState>) -> impl IntoResponse {
    let user_id = state.default_user_id.clone();
    let default_body = default_voice_prompt_template();
    let Some(template) =
        prompt_templates::get_or_seed(&state.pool, &user_id, default_voice_prompt(&default_body))
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(to_response(template, default_body)).into_response()
}

pub async fn save_voice_prompt_draft(
    State(state): State<AppState>,
    Json(req): Json<SaveDraftRequest>,
) -> impl IntoResponse {
    if req.draft_body.trim().is_empty() || req.draft_body.len() > MAX_PROMPT_BYTES {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": "Prompt draft is empty or too large"})),
        )
            .into_response();
    }

    let user_id = state.default_user_id.clone();
    let default_body = default_voice_prompt_template();
    let Some(template) = prompt_templates::save_draft(
        &state.pool,
        &user_id,
        default_voice_prompt(&default_body),
        &req.draft_body,
    ) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(to_response(template, default_body)).into_response()
}

pub async fn apply_voice_prompt_draft(State(state): State<AppState>) -> impl IntoResponse {
    let user_id = state.default_user_id.clone();
    let default_body = default_voice_prompt_template();
    let Some(template) =
        prompt_templates::apply_draft(&state.pool, &user_id, default_voice_prompt(&default_body))
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(to_response(template, default_body)).into_response()
}

pub async fn reset_voice_prompt(State(state): State<AppState>) -> impl IntoResponse {
    let user_id = state.default_user_id.clone();
    let default_body = default_voice_prompt_template();
    let Some(template) = prompt_templates::reset_to_default(
        &state.pool,
        &user_id,
        default_voice_prompt(&default_body),
    ) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(to_response(template, default_body)).into_response()
}

pub async fn test_voice_prompt(
    State(state): State<AppState>,
    Json(req): Json<TestPromptRequest>,
) -> impl IntoResponse {
    let transcript = req.transcript.trim().to_string();
    if transcript.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let user_id = state.default_user_id.clone();
    let pool = state.pool.clone();
    let Some(prefs) = crate::get_prefs_cached(&state.prefs_cache, &pool, &user_id).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let missing = crate::routes::key_guard::missing_text_api_keys(&pool, &user_id, &prefs);
    if !missing.is_empty() {
        return crate::routes::key_guard::missing_api_keys_response(missing);
    }

    let default_body = default_voice_prompt_template();
    let Some(template) =
        prompt_templates::get_or_seed(&pool, &user_id, default_voice_prompt(&default_body))
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let prompt_body = req
        .draft_body
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(template.draft_body.as_deref())
        .unwrap_or(&template.active_body);

    let (word_corrections, _) =
        crate::get_lexicon_cached(&state.lexicon_cache, &pool, &user_id).await;
    let relevant_corrections =
        crate::store::corrections::filter_relevant(&word_corrections, &transcript, 2, 10);
    let system_prompt =
        render_voice_system_prompt_template(prompt_body, &prefs, &[], &relevant_corrections, &[]);
    let user_message = build_user_message(&transcript, &prefs.output_language);

    match run_prompt_test(
        &state.http_client,
        &pool,
        &user_id,
        &prefs,
        system_prompt,
        user_message,
    )
    .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            warn!("[prompts] test failed: {e}");
            (StatusCode::BAD_GATEWAY, Json(json!({"message": e}))).into_response()
        }
    }
}

fn default_voice_prompt(default_body: &str) -> DefaultPrompt<'_> {
    DefaultPrompt {
        kind: VOICE_PROMPT_KIND,
        title: VOICE_PROMPT_TITLE,
        base_version: VOICE_PROMPT_BASE_VERSION,
        body: default_body,
    }
}

fn to_response(template: PromptTemplate, default_body: String) -> PromptTemplateResponse {
    let has_draft = template
        .draft_body
        .as_deref()
        .map(|s| !s.trim().is_empty() && s != template.active_body)
        .unwrap_or(false);
    let active_is_default = template.active_body == default_body;
    PromptTemplateResponse {
        kind: template.kind,
        title: template.title,
        base_version: template.base_version,
        active_body: template.active_body,
        draft_body: template.draft_body,
        default_body,
        updated_at: template.updated_at,
        applied_at: template.applied_at,
        has_draft,
        active_is_default,
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

async fn run_prompt_test(
    client: &reqwest::Client,
    pool: &crate::store::DbPool,
    user_id: &str,
    prefs: &crate::store::prefs::Preferences,
    system_prompt: String,
    user_message: String,
) -> Result<TestPromptResponse, String> {
    let llm_provider = prefs.llm_provider.clone();
    let model = if llm_provider == "openai_codex" {
        openai_codex::MODEL_MINI.to_string()
    } else if llm_provider == "gemini_direct" {
        gemini_direct::GEMINI_DIRECT_MODEL.to_string()
    } else if llm_provider == "groq" {
        if prefs.selected_model == "smart" {
            groq::GROQ_MODEL_SMART
        } else {
            groq::GROQ_MODEL_FAST
        }
        .to_string()
    } else {
        said_core::resolve_model(&prefs.selected_model).to_string()
    };

    let gateway_key = non_empty(prefs.gateway_api_key.clone())
        .or_else(|| non_empty(std::env::var("GATEWAY_API_KEY").ok()))
        .or_else(|| {
            let key = said_core::api_key();
            if key.trim().is_empty() {
                None
            } else {
                Some(key)
            }
        })
        .unwrap_or_default();
    let gemini_key = non_empty(prefs.gemini_api_key.clone())
        .or_else(|| non_empty(std::env::var("GEMINI_API_KEY").ok()))
        .unwrap_or_default();
    let groq_key = non_empty(prefs.groq_api_key.clone())
        .or_else(|| non_empty(std::env::var("GROQ_API_KEY").ok()))
        .unwrap_or_default();
    let openai_token = if llm_provider == "openai_codex" {
        openai_oauth::get_token(pool, user_id).map(|t| t.access_token)
    } else {
        None
    };

    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
    let client_c = client.clone();
    let groq_key_for_recovery = groq_key.clone();
    let provider = llm_provider.clone();
    let model_c = model.clone();
    let started = Instant::now();
    let task = tokio::spawn(async move {
        if provider == "openai_codex" {
            let access_token = openai_token.as_deref().unwrap_or("");
            if access_token.is_empty() {
                return Err(
                    "OpenAI not connected — go to Settings to connect your account".to_string(),
                );
            }
            openai_codex::stream_polish(
                &client_c,
                access_token,
                &model_c,
                &system_prompt,
                &user_message,
                token_tx,
            )
            .await
        } else if provider == "gemini_direct" {
            gemini_direct::stream_polish(
                &client_c,
                &gemini_key,
                &model_c,
                &system_prompt,
                &user_message,
                token_tx,
            )
            .await
        } else if provider == "groq" {
            groq::stream_polish(
                &client_c,
                &groq_key,
                &model_c,
                &system_prompt,
                &user_message,
                token_tx,
            )
            .await
        } else {
            gateway::stream_polish(
                &client_c,
                &gateway_key,
                &model_c,
                &system_prompt,
                &user_message,
                token_tx,
            )
            .await
        }
    });

    while token_rx.recv().await.is_some() {}

    let mut result = task
        .await
        .map_err(|e| format!("prompt test task failed: {e}"))??;
    if prefs.output_language == "hinglish" && script::contains_devanagari(&result.polished) {
        result.polished = match crate::llm::devanagari_recovery::recover(
            client,
            &groq_key_for_recovery,
            &result.polished,
        )
        .await
        {
            Ok(recovered) => recovered,
            Err(_) => script::enforce_roman_hinglish(&result.polished),
        };
    }
    // format_recover disabled — will re-enable with targeted replacement
    // result.polished = crate::llm::format_recover::recover(&result.polished);

    Ok(TestPromptResponse {
        output: result.polished,
        model_used: model,
        latency_ms: started.elapsed().as_millis() as i64,
    })
}
