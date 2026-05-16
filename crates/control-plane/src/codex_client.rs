//! Codex client — PKCE OAuth for OpenAI account connection + Codex API.
//!
//! Instead of using official OpenAI API keys, we connect a ChatGPT/OpenAI
//! account via PKCE OAuth and call the ChatGPT Codex backend directly.

use sha2::{Digest, Sha256};

// ── Constants ───────────────────────────────────────────────────────────────

/// Public Codex CLI client ID (same one Gateway uses).
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_BASE: &str = "https://auth.openai.com/oauth";
const CODEX_API: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Default redirect URI — overridable via `OPENAI_REDIRECT_URI` env var.
const DEFAULT_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CodexError {
    Http(reqwest::Error),
    Auth(String),
    RateLimit(String),
    Parse(String),
}

impl std::fmt::Display for CodexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Auth(msg) => write!(f, "auth error: {msg}"),
            Self::RateLimit(msg) => write!(f, "rate limit: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
        }
    }
}

impl std::error::Error for CodexError {}

impl From<reqwest::Error> for CodexError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

// ── PKCE types ──────────────────────────────────────────────────────────────

pub struct PkceSession {
    pub auth_url: String,
    pub code_verifier: String,
    pub state: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
}

/// Result from a Codex API call — the concatenated output text plus
/// the plan type header if the API returned one.
pub struct CodexResponse {
    pub text: String,
    pub plan_type: Option<String>,
}

// ── PKCE session creation ───────────────────────────────────────────────────

/// Create a new PKCE session for OpenAI OAuth.
///
/// Generates a random `state` (UUID v4), a random `code_verifier`
/// (64 URL-safe base64 chars), and computes the S256 challenge.
pub fn create_pkce_session() -> PkceSession {
    let state = uuid::Uuid::new_v4().to_string();

    // Generate a 64-byte random code_verifier (URL-safe base64, no padding).
    // This yields ~86 chars — well within the 43-128 range.
    let random_bytes: Vec<u8> = (0..48).map(|_| rand::random::<u8>()).collect();
    let code_verifier = base64_url_encode(&random_bytes);

    // code_challenge = base64url(sha256(code_verifier))
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let digest = hasher.finalize();
    let code_challenge = base64_url_encode(&digest);

    let redirect_uri = redirect_uri();

    let auth_url = format!(
        "{AUTH_BASE}/authorize\
         ?client_id={CLIENT_ID}\
         &redirect_uri={redirect}\
         &response_type=code\
         &scope=openid%20profile%20email%20offline_access\
         &code_challenge={code_challenge}\
         &code_challenge_method=S256\
         &state={state}\
         &id_token_add_organizations=true\
         &codex_cli_simplified_flow=true",
        redirect = urlencoding::encode(&redirect_uri),
    );

    PkceSession {
        auth_url,
        code_verifier,
        state,
    }
}

// ── Token exchange ──────────────────────────────────────────────────────────

/// Exchange an authorization code for tokens.
pub async fn exchange_code(code: &str, code_verifier: &str) -> Result<TokenResponse, CodexError> {
    let redirect_uri = redirect_uri();

    let resp = reqwest::Client::new()
        .post(format!("{AUTH_BASE}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", code_verifier),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect_uri),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CodexError::Auth(format!(
            "token exchange failed ({status}): {body}"
        )));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| CodexError::Parse(format!("failed to parse token response: {e}")))
}

/// Refresh an access token using a refresh token.
pub async fn refresh_token(refresh_token: &str) -> Result<TokenResponse, CodexError> {
    let resp = reqwest::Client::new()
        .post(format!("{AUTH_BASE}/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CodexError::Auth(format!(
            "token refresh failed ({status}): {body}"
        )));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| CodexError::Parse(format!("failed to parse refresh response: {e}")))
}

// ── Codex API call ──────────────────────────────────────────────────────────

/// Call the Codex responses API and collect the streamed output text.
///
/// Sends a streaming request, reads SSE events, concatenates
/// `response.output_text.delta` payloads, and returns the full text
/// along with the `x-codex-plan-type` header if present.
pub async fn call_codex(
    access_token: &str,
    model: &str,
    instructions: &str,
    user_input: &str,
) -> Result<CodexResponse, CodexError> {
    let body = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": [
            { "type": "message", "role": "user", "content": user_input }
        ],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "reasoning": { "summary": "auto" },
        "store": false,
        "stream": true,
    });

    let resp = reqwest::Client::new()
        .post(CODEX_API)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let detail = resp.text().await.unwrap_or_default();
        return Err(CodexError::RateLimit(detail));
    }

    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(CodexError::Auth(format!(
            "codex call failed ({status}): {detail}"
        )));
    }

    // Extract plan-type header before consuming the body.
    let plan_type = resp
        .headers()
        .get("x-codex-plan-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Read the SSE stream.
    let full_body = resp.bytes().await?;
    let text = parse_sse_deltas(&full_body);

    Ok(CodexResponse { text, plan_type })
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Base64-URL encode without padding (RFC 7636).
fn base64_url_encode(input: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

/// Get the redirect URI from env or fallback to the default.
fn redirect_uri() -> String {
    std::env::var("OPENAI_REDIRECT_URI").unwrap_or_else(|_| DEFAULT_REDIRECT_URI.to_string())
}

/// Parse SSE data from the Codex streaming response, collecting all
/// `response.output_text.delta` events into a single string.
///
/// SSE format: `data: {"type":"response.output_text.delta","delta":"...",...}\n\n`
fn parse_sse_deltas(raw: &[u8]) -> String {
    let body_str = String::from_utf8_lossy(raw);
    let mut result = String::new();

    for line in body_str.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }

        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };

        match parsed.get("type").and_then(|t| t.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(delta) = parsed.get("delta").and_then(|d| d.as_str()) {
                    result.push_str(delta);
                }
            }
            Some("response.completed") => break,
            _ => {}
        }
    }

    result
}
