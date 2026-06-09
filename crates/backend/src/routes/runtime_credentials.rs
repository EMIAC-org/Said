//! Local-to-server runtime credential sync.
//!
//! Desktop stores API keys in local SQLite. This module mirrors them into the
//! control-plane encrypted vault (`runtime_provider_credentials`) so server-side
//! polish/STT can use per-user keys. Also proxies vault status for the UI.

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::{
    AppState,
    store::{
        prefs::{self, Preferences},
        users,
    },
};

const DEFAULT_CONTROL_PLANE_URL: &str = "https://airnote.emiactech.com";
const SYNC_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Serialize, Default)]
pub struct CredentialSyncResult {
    pub provider: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CredentialSyncResponse {
    pub connected: bool,
    pub server_url: Option<String>,
    pub attempted: usize,
    pub synced: usize,
    pub skipped: usize,
    pub failed: usize,
    pub revoked: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<CredentialSyncResult>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ServerCredentialSummary {
    pub id: String,
    pub provider: String,
    pub scope: String,
    pub display_name: String,
    pub secret_last4: String,
    pub status: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CredentialVaultStatus {
    pub signed_in: bool,
    pub server_url: Option<String>,
    pub encryption_configured: bool,
    #[serde(default)]
    pub server_credentials: Vec<ServerCredentialSummary>,
    #[serde(default)]
    pub local_providers: Vec<String>,
}

#[derive(Debug)]
struct ProviderSecret {
    provider: &'static str,
    display_name: &'static str,
    secret: String,
}

#[derive(Debug, Deserialize)]
struct ServerCredentialRow {
    id: String,
    provider: String,
    scope: String,
    display_name: String,
    secret_last4: String,
    status: String,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuntimeStatusPayload {
    #[serde(default)]
    credential_encryption_configured: bool,
}

struct AuthContext {
    token: String,
    server_url: String,
}

pub async fn sync(
    State(state): State<AppState>,
) -> Result<Json<CredentialSyncResponse>, StatusCode> {
    sync_saved_provider_credentials(state)
        .await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

pub async fn status(State(state): State<AppState>) -> Json<CredentialVaultStatus> {
    Json(fetch_vault_status(&state).await)
}

pub async fn sync_saved_provider_credentials(
    state: AppState,
) -> Result<CredentialSyncResponse, String> {
    let Some(auth) = resolve_auth(&state) else {
        return Ok(CredentialSyncResponse {
            reason: Some("not signed in".into()),
            ..Default::default()
        });
    };

    let Some(prefs) = prefs::get_prefs(&state.pool, &state.default_user_id) else {
        return Ok(CredentialSyncResponse {
            connected: true,
            server_url: Some(auth.server_url.clone()),
            reason: Some("preferences not found".into()),
            ..Default::default()
        });
    };

    let local = provider_secrets(&prefs);
    let local_providers: Vec<String> = local.iter().map(|p| p.provider.to_string()).collect();
    let server_rows = fetch_server_credentials(&state.http_client, &auth).await?;
    let server_by_provider: std::collections::HashMap<String, ServerCredentialRow> = server_rows
        .into_iter()
        .filter(|row| row.scope == "user" && row.status == "active")
        .map(|row| (row.provider.clone(), row))
        .collect();

    let mut response = CredentialSyncResponse {
        connected: true,
        server_url: Some(auth.server_url.clone()),
        ..Default::default()
    };

    for credential in local {
        response.attempted += 1;
        let url = format!(
            "{}/v1/runtime/credentials",
            auth.server_url.trim_end_matches('/')
        );
        let body = json!({
            "provider": credential.provider,
            "scope": "user",
            "display_name": credential.display_name,
            "secret": credential.secret,
        });
        match state
            .http_client
            .post(&url)
            .bearer_auth(&auth.token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(SYNC_TIMEOUT_SECS))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                response.synced += 1;
                response.results.push(CredentialSyncResult {
                    provider: credential.provider.to_string(),
                    action: "synced".into(),
                    error: None,
                });
            }
            Ok(resp) => {
                response.failed += 1;
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let err = extract_error_message(&body, status.as_u16());
                warn!(
                    "[runtime-credentials] sync failed provider={} status={} err={}",
                    credential.provider, status, err
                );
                response.results.push(CredentialSyncResult {
                    provider: credential.provider.to_string(),
                    action: "failed".into(),
                    error: Some(err),
                });
            }
            Err(err) => {
                response.failed += 1;
                let msg = err.to_string();
                warn!(
                    "[runtime-credentials] sync request failed provider={}: {msg}",
                    credential.provider
                );
                response.results.push(CredentialSyncResult {
                    provider: credential.provider.to_string(),
                    action: "failed".into(),
                    error: Some(msg),
                });
            }
        }
    }

    for (provider, row) in server_by_provider {
        if local_providers.iter().any(|p| p == &provider) {
            continue;
        }
        response.attempted += 1;
        let url = format!(
            "{}/v1/runtime/credentials/{}",
            auth.server_url.trim_end_matches('/'),
            row.id
        );
        match state
            .http_client
            .delete(&url)
            .bearer_auth(&auth.token)
            .timeout(std::time::Duration::from_secs(SYNC_TIMEOUT_SECS))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 204 => {
                response.revoked += 1;
                response.results.push(CredentialSyncResult {
                    provider,
                    action: "revoked".into(),
                    error: None,
                });
            }
            Ok(resp) => {
                response.failed += 1;
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let err = extract_error_message(&body, status.as_u16());
                response.results.push(CredentialSyncResult {
                    provider,
                    action: "revoke_failed".into(),
                    error: Some(err),
                });
            }
            Err(err) => {
                response.failed += 1;
                response.results.push(CredentialSyncResult {
                    provider,
                    action: "revoke_failed".into(),
                    error: Some(err.to_string()),
                });
            }
        }
    }

    if response.synced > 0 || response.revoked > 0 {
        info!(
            "[runtime-credentials] vault sync complete synced={} revoked={} failed={} server={}",
            response.synced, response.revoked, response.failed, auth.server_url
        );
    } else if response.failed > 0 {
        warn!(
            "[runtime-credentials] vault sync failed for all providers failed={} server={}",
            response.failed, auth.server_url
        );
    }

    Ok(response)
}

async fn fetch_vault_status(state: &AppState) -> CredentialVaultStatus {
    let Some(auth) = resolve_auth(state) else {
        return CredentialVaultStatus::default();
    };

    let prefs = prefs::get_prefs(&state.pool, &state.default_user_id);
    let local_providers = prefs
        .as_ref()
        .map(|p| {
            provider_secrets(p)
                .into_iter()
                .map(|s| s.provider.to_string())
                .collect()
        })
        .unwrap_or_default();

    let encryption_configured = fetch_encryption_configured(&state.http_client, &auth)
        .await
        .unwrap_or(false);
    let server_credentials = fetch_server_credentials(&state.http_client, &auth)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| ServerCredentialSummary {
            id: row.id,
            provider: row.provider,
            scope: row.scope,
            display_name: row.display_name,
            secret_last4: row.secret_last4,
            status: row.status,
            updated_at: row.updated_at,
        })
        .collect();

    CredentialVaultStatus {
        signed_in: true,
        server_url: Some(auth.server_url),
        encryption_configured,
        server_credentials,
        local_providers,
    }
}

fn resolve_auth(state: &AppState) -> Option<AuthContext> {
    let user = users::get_user(&state.pool, &state.default_user_id)?;
    let token = user
        .cloud_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)?;
    let server_url = user
        .enterprise_server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AIRNOTE_CONTROL_PLANE_URL").ok())
        .or_else(|| std::env::var("CLOUD_API_URL").ok())
        .unwrap_or_else(|| DEFAULT_CONTROL_PLANE_URL.to_string());
    Some(AuthContext { token, server_url })
}

async fn fetch_server_credentials(
    http: &reqwest::Client,
    auth: &AuthContext,
) -> Result<Vec<ServerCredentialRow>, String> {
    let url = format!(
        "{}/v1/runtime/credentials",
        auth.server_url.trim_end_matches('/')
    );
    let resp = http
        .get(&url)
        .bearer_auth(&auth.token)
        .timeout(std::time::Duration::from_secs(SYNC_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("credentials list request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(extract_error_message(&body, status.as_u16()));
    }
    resp.json::<Vec<ServerCredentialRow>>()
        .await
        .map_err(|e| format!("credentials list decode failed: {e}"))
}

async fn fetch_encryption_configured(
    http: &reqwest::Client,
    auth: &AuthContext,
) -> Result<bool, String> {
    let url = format!(
        "{}/v1/runtime/status",
        auth.server_url.trim_end_matches('/')
    );
    let resp = http
        .get(&url)
        .bearer_auth(&auth.token)
        .timeout(std::time::Duration::from_secs(SYNC_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("runtime status request failed: {e}"))?;
    if !resp.status().is_success() {
        return Ok(false);
    }
    let payload = resp
        .json::<RuntimeStatusPayload>()
        .await
        .map_err(|e| format!("runtime status decode failed: {e}"))?;
    Ok(payload.credential_encryption_configured)
}

fn extract_error_message(body: &str, status: u16) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(msg) = value
            .get("message")
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
        {
            return format!("server returned {status}: {msg}");
        }
    }
    let snippet: String = body.chars().take(160).collect();
    if snippet.is_empty() {
        format!("server returned {status}")
    } else {
        format!("server returned {status}: {snippet}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_secrets_includes_groq_from_gateway_gsk() {
        let prefs = Preferences {
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
            server_runtime_enabled: true,
            server_audio_runtime_enabled: false,
            updated_at: 0,
            gateway_api_key: Some("gsk_test_gateway_key_1234567890".into()),
            deepgram_api_key: Some("dg_test".into()),
            gemini_api_key: None,
            groq_api_key: None,
            cerebras_api_key: None,
            llm_provider: "groq".into(),
            stt_provider: "deepgram".into(),
        };
        let providers: Vec<&str> = provider_secrets(&prefs)
            .iter()
            .map(|p| p.provider)
            .collect();
        assert!(providers.contains(&"deepgram"));
        assert!(providers.contains(&"groq"));
        assert!(providers.contains(&"gateway"));
    }
}
