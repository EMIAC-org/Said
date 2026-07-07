//! Proxy for server-owned message polish (`POST /v1/runtime/message-polish`).

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

use crate::{cp_client, llm::PolishResult, store::DbPool};

pub const MESSAGE_POLISH_SIGNIN_ERROR: &str = "Polish My Message requires AirNote sign-in";

#[derive(Debug, Serialize)]
struct ServerMessagePolishRequest {
    text: String,
    client_run_id: Option<String>,
    /// "polish" (⌥1) or "to_english" (⌥2). The server swaps the prompt directive.
    mode: String,
}

#[derive(Debug, Deserialize)]
struct ServerMessagePolishResponse {
    output: String,
    model_used: String,
    latency_ms: ServerMessagePolishLatency,
}

#[derive(Debug, Deserialize)]
struct ServerMessagePolishLatency {
    total: i64,
}

pub async fn run_server_message_polish(
    http_client: &Client,
    pool: &DbPool,
    user_id: &str,
    text: &str,
    client_run_id: Option<&str>,
    mode: &str,
) -> Result<(PolishResult, String), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("message polish input is empty".to_string());
    }

    if !crate::store::users::has_enterprise_auth(pool, user_id) {
        return Err(MESSAGE_POLISH_SIGNIN_ERROR.to_string());
    }

    let Some(user) = crate::store::users::get_user(pool, user_id) else {
        return Err("local user not found".to_string());
    };
    let token = user
        .cloud_token
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| MESSAGE_POLISH_SIGNIN_ERROR.to_string())?;
    let base_url = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(said_core::AIRNOTE_DEFAULT_CONTROL_PLANE_URL)
        .to_string();

    let req = ServerMessagePolishRequest {
        text: trimmed.to_string(),
        client_run_id: client_run_id
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| Some(Uuid::new_v4().to_string())),
        mode: mode.to_string(),
    };

    let url = format!(
        "{}/v1/runtime/message-polish",
        base_url.trim_end_matches('/')
    );
    let start = Instant::now();
    let resp = cp_client::with_org_context(
        http_client
            .post(&url)
            .bearer_auth(token)
            .json(&req)
            .timeout(std::time::Duration::from_secs(60)),
        Some(&user),
    )
    .send()
    .await
    .map_err(|e| format!("server message polish request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(MESSAGE_POLISH_SIGNIN_ERROR.to_string());
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "server message polish returned {status}: {}",
            said_core::text::truncate_utf8(&body, 240)
        ));
    }

    let parsed = resp
        .json::<ServerMessagePolishResponse>()
        .await
        .map_err(|e| format!("server message polish response parse failed: {e}"))?;

    let output = parsed.output.trim().to_string();
    if output.is_empty() {
        return Err("server message polish returned empty output".to_string());
    }

    let measured_ms = start.elapsed().as_millis() as u64;
    let server_ms = parsed.latency_ms.total.max(0) as u64;
    let polish_ms = measured_ms.max(server_ms);

    Ok((
        PolishResult {
            polished: output,
            polish_ms,
        },
        format!("server-message-polish:{}", parsed.model_used),
    ))
}
