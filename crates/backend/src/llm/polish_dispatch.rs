//! Plug-and-play polish dispatch — one module per provider.
//!
//! Providers: `groq`, `cerebras`, `deepinfra`, `gateway`, `gemini_direct`, `openai_codex`.
//! Model routing is resolved in `said_core::polish::model`.

use reqwest::Client;
use tokio::sync::mpsc;
use tracing::info;

use super::{PolishResult, cerebras, deepinfra, gateway, gemini_direct, groq, openai_codex};
use said_core::polish::model::{PolishRoute, resolve_polish_route};

pub fn voice_polish_route(selected_model: &str) -> PolishRoute {
    resolve_polish_route(selected_model)
}

pub async fn stream_polish_routed(
    client: &Client,
    route: &PolishRoute,
    groq_key: &str,
    gateway_key: &str,
    gemini_key: &str,
    cerebras_key: &str,
    deepinfra_key: &str,
    openai_access_token: Option<&str>,
    llm_provider: &str,
    system_prompt: &str,
    user_message: &str,
    token_tx: mpsc::Sender<String>,
) -> Result<PolishResult, String> {
    if llm_provider == "openai_codex" {
        let access_token = openai_access_token.unwrap_or("");
        if access_token.is_empty() {
            return Err(
                "OpenAI not connected — go to Settings to connect your account".to_string(),
            );
        }
        return openai_codex::stream_polish(
            client,
            access_token,
            openai_codex::MODEL_MINI,
            system_prompt,
            user_message,
            token_tx,
        )
        .await;
    }
    if llm_provider == "gemini_direct" {
        return gemini_direct::stream_polish(
            client,
            gemini_key,
            gemini_direct::GEMINI_DIRECT_MODEL,
            system_prompt,
            user_message,
            token_tx,
        )
        .await;
    }

    info!(
        "[polish] routing → {} model={} (llm_provider={llm_provider})",
        route.provider, route.model
    );

    match route.provider {
        "groq" => {
            groq::stream_polish(
                client,
                groq_key,
                &route.model,
                system_prompt,
                user_message,
                token_tx,
            )
            .await
        }
        "cerebras" => {
            cerebras::stream_polish(
                client,
                cerebras_key,
                &route.model,
                system_prompt,
                user_message,
                token_tx,
            )
            .await
        }
        "deepinfra" => {
            deepinfra::stream_polish(
                client,
                deepinfra_key,
                &route.model,
                system_prompt,
                user_message,
                token_tx,
            )
            .await
        }
        "openrouter" => {
            // The PRIMARY polish path runs on the control-plane, which serves the
            // Gemma model through OpenRouter. This local dispatch is only the
            // OFFLINE fallback and the desktop ships no OpenRouter key, so degrade
            // gracefully to the previous prod model (Cerebras gpt-oss) here.
            cerebras::stream_polish(
                client,
                cerebras_key,
                said_core::polish::model::CEREBRAS_POLISH_MODEL_GPT_OSS,
                system_prompt,
                user_message,
                token_tx,
            )
            .await
        }
        _ => {
            gateway::stream_polish(
                client,
                gateway_key,
                &route.model,
                system_prompt,
                user_message,
                token_tx,
            )
            .await
        }
    }
}
