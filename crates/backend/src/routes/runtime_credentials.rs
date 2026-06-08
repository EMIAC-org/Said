//! Local-to-server runtime credential sync.
//!
//! The local app still stores API keys in SQLite for the current desktop
//! runtime. When server audio runtime is enabled, this sync pushes those keys
//! to the AirNote control plane vault so server-side STT/polish can use the
//! same per-user credentials. The control plane stores only encrypted secrets.

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use serde_json::json;
use tracing::{debug, info, warn};

use crate::{
    AppState,
    store::{
        prefs::{self, Preferences},
        users,
    },
};

const DEFAULT_CONTROL_PLANE_URL: &str = "https://airnote.emiactech.com";

#[derive(Debug, Clone, Serialize, Default)]
pub struct CredentialSyncResponse {
    pub connected: bool,
    pub server_url: Option<String>,
    pub attempted: usize,
    pub synced: usize,
    pub skipped: usize,
    pub failed: usize,
    pub reason: Option<String>,
}

#[derive(Debug)]
struct ProviderSecret {
    provider: &'static str,
    display_name: &'static str,
    secret: String,
}

pub async fn sync(
    State(state): State<AppState>,
) -> Result<Json<CredentialSyncResponse>, StatusCode> {
    sync_saved_provider_credentials(state)
        .await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

pub async fn sync_saved_provider_credentials(
    state: AppState,
) -> Result<CredentialSyncResponse, String> {
    let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
        return Ok(CredentialSyncResponse {
            reason: Some("local user not found".into()),
            ..Default::default()
        });
    };
    let Some(token) = user
        .cloud_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return Ok(CredentialSyncResponse {
            reason: Some("not signed in".into()),
            ..Default::default()
        });
    };

    let server_url = user
        .enterprise_server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AIRNOTE_CONTROL_PLANE_URL").ok())
        .or_else(|| std::env::var("CLOUD_API_URL").ok())
        .unwrap_or_else(|| DEFAULT_CONTROL_PLANE_URL.to_string());

    let Some(prefs) = prefs::get_prefs(&state.pool, &state.default_user_id) else {
        return Ok(CredentialSyncResponse {
            connected: true,
            server_url: Some(server_url),
            reason: Some("preferences not found".into()),
            ..Default::default()
        });
    };

    let credentials = provider_secrets(&prefs);
    if credentials.is_empty() {
        return Ok(CredentialSyncResponse {
            connected: true,
            server_url: Some(server_url),
            reason: Some("no local provider keys to sync".into()),
            ..Default::default()
        });
    }

    let mut response = CredentialSyncResponse {
        connected: true,
        server_url: Some(server_url.clone()),
        attempted: credentials.len(),
        ..Default::default()
    };
    let url = format!(
        "{}/v1/runtime/credentials",
        server_url.trim_end_matches('/')
    );

    for credential in credentials {
        let body = json!({
            "provider": credential.provider,
            "scope": "user",
            "display_name": credential.display_name,
            "secret": credential.secret,
        });
        let result = state
            .http_client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                response.synced += 1;
                debug!(
                    "[runtime-credentials] synced provider={} to server vault",
                    credential.provider
                );
            }
            Ok(resp) => {
                response.failed += 1;
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                warn!(
                    "[runtime-credentials] sync failed provider={} status={} body={}",
                    credential.provider,
                    status,
                    body.chars().take(160).collect::<String>()
                );
            }
            Err(err) => {
                response.failed += 1;
                warn!(
                    "[runtime-credentials] sync request failed provider={}: {err}",
                    credential.provider
                );
            }
        }
    }

    response.skipped = response
        .attempted
        .saturating_sub(response.synced + response.failed);
    if response.synced > 0 {
        info!(
            "[runtime-credentials] startup sync complete synced={} failed={} server={}",
            response.synced, response.failed, server_url
        );
    }
    Ok(response)
}

fn provider_secrets(prefs: &Preferences) -> Vec<ProviderSecret> {
    let mut out = Vec::new();
    if let Some(secret) = clean_secret(prefs.deepgram_api_key.as_deref()) {
        out.push(ProviderSecret {
            provider: "deepgram",
            display_name: "Deepgram API key",
            secret,
        });
    }
    if let Some(secret) = clean_secret(prefs.groq_api_key.as_deref()) {
        out.push(ProviderSecret {
            provider: "groq",
            display_name: "Groq API key",
            secret,
        });
    }
    if let Some(secret) = clean_secret(prefs.gateway_api_key.as_deref()) {
        out.push(ProviderSecret {
            provider: "gateway",
            display_name: "Gateway API key",
            secret: secret.clone(),
        });
        if clean_secret(prefs.groq_api_key.as_deref()).is_none() && secret.starts_with("gsk_") {
            out.push(ProviderSecret {
                provider: "groq",
                display_name: "Groq API key",
                secret,
            });
        }
    }
    if let Some(secret) = clean_secret(prefs.gemini_api_key.as_deref()) {
        out.push(ProviderSecret {
            provider: "gemini",
            display_name: "Gemini API key",
            secret,
        });
    }
    out
}

fn clean_secret(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
