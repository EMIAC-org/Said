//! Proxy for desktop Developer Problem Command (`POST /v1/runtime/problem/solve`).

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Instant;
use uuid::Uuid;

use crate::{AppState, cp_client};

pub const PROBLEM_CONTEXT_CAP_CHARS: usize = 8_000;
pub const PROBLEM_SCREEN_CONTEXT_CAP_CHARS: usize = 500;
pub const PROBLEM_SIGNIN_ERROR: &str = "Developer Problem Command requires AirNote sign-in";

#[derive(Debug, Deserialize)]
pub struct ProblemSolveRequest {
    pub transcript: String,
    #[serde(default)]
    pub context_mode: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub project_context: Option<String>,
    #[serde(default)]
    pub screen_context: Option<String>,
    #[serde(default)]
    pub client_run_id: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct ServerProblemSolveRequest {
    transcript: String,
    context_mode: String,
    project_id: Option<String>,
    project_name: Option<String>,
    project_context: Option<String>,
    screen_context: Option<String>,
    selected_model: String,
    client_run_id: Option<String>,
    platform: Option<String>,
    app_version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProblemSolveResponse {
    pub run_id: String,
    pub output: String,
    pub model_used: String,
    pub prompt_version: String,
    pub latency_ms: ProblemSolveLatency,
    pub context_mode: String,
    pub project_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProblemSolveLatency {
    #[serde(default)]
    pub prompt: i64,
    #[serde(default)]
    pub model: i64,
    pub total: i64,
}

pub async fn solve(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(req): Json<ProblemSolveRequest>,
) -> impl IntoResponse {
    match run_server_problem_solve(&state, req).await {
        Ok(response) => (StatusCode::OK, Json(json!(response))).into_response(),
        Err((status, message)) => (
            status,
            Json(json!({
                "error_code": "problem_solve_failed",
                "message": message,
            })),
        )
            .into_response(),
    }
}

async fn run_server_problem_solve(
    state: &AppState,
    req: ProblemSolveRequest,
) -> Result<ProblemSolveResponse, (StatusCode, String)> {
    let transcript = req.transcript.trim();
    if transcript.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "transcript is required".to_string(),
        ));
    }

    let context_mode = normalize_context_mode(&req.context_mode);
    if context_mode == "ambiguous" {
        return Err((
            StatusCode::BAD_REQUEST,
            "ambiguous project context must be resolved on desktop before solving".to_string(),
        ));
    }

    let project_context = req
        .project_context
        .as_deref()
        .map(|s| {
            s.chars()
                .take(PROBLEM_CONTEXT_CAP_CHARS + 1)
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty());
    if project_context
        .as_ref()
        .map(|s| s.chars().count() > PROBLEM_CONTEXT_CAP_CHARS)
        .unwrap_or(false)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("project context must be at most {PROBLEM_CONTEXT_CAP_CHARS} characters"),
        ));
    }

    if context_mode == "project" && project_context.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "project context is required for project mode".to_string(),
        ));
    }

    let user_id = state.default_user_id.as_str();
    if !crate::store::users::has_enterprise_auth(&state.pool, user_id) {
        return Err((StatusCode::FORBIDDEN, PROBLEM_SIGNIN_ERROR.to_string()));
    }

    let Some(user) = crate::store::users::get_user(&state.pool, user_id) else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "local user not found".to_string(),
        ));
    };
    let token = user
        .cloud_token
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| (StatusCode::FORBIDDEN, PROBLEM_SIGNIN_ERROR.to_string()))?;
    let base_url = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(said_core::AIRNOTE_DEFAULT_CONTROL_PLANE_URL)
        .to_string();
    let prefs = crate::get_prefs_cached(&state.prefs_cache, &state.pool, user_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "preferences not found".to_string(),
            )
        })?;

    let server_req = ServerProblemSolveRequest {
        transcript: transcript.to_string(),
        context_mode,
        project_id: req.project_id.filter(|s| !s.trim().is_empty()),
        project_name: req.project_name.filter(|s| !s.trim().is_empty()),
        project_context,
        screen_context: req.screen_context.map(|s| {
            s.chars()
                .take(PROBLEM_SCREEN_CONTEXT_CAP_CHARS)
                .collect::<String>()
        }),
        selected_model: prefs.selected_model,
        client_run_id: req
            .client_run_id
            .filter(|s| !s.trim().is_empty())
            .or_else(|| Some(Uuid::new_v4().to_string())),
        platform: req.platform,
        app_version: req.app_version,
    };

    let url = format!(
        "{}/v1/runtime/problem/solve",
        base_url.trim_end_matches('/')
    );
    let start = Instant::now();
    let resp = cp_client::with_org_context(
        state
            .http_client
            .post(&url)
            .bearer_auth(token)
            .json(&server_req)
            .timeout(std::time::Duration::from_secs(90)),
        Some(&user),
    )
    .send()
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("server problem solve request failed: {e}"),
        )
    })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err((StatusCode::FORBIDDEN, PROBLEM_SIGNIN_ERROR.to_string()));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "server problem solve returned {status}: {}",
                said_core::text::truncate_utf8(&body, 240)
            ),
        ));
    }

    let mut parsed = resp.json::<ProblemSolveResponse>().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("server problem solve response parse failed: {e}"),
        )
    })?;
    parsed.latency_ms.total = parsed
        .latency_ms
        .total
        .max(start.elapsed().as_millis() as i64);
    Ok(parsed)
}

fn normalize_context_mode(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "project" | "matched" | "using_context" => "project".to_string(),
        "ambiguous" => "ambiguous".to_string(),
        _ => "generic".to_string(),
    }
}
