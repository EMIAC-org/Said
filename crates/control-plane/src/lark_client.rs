//! HTTP client for Lark Open API — OAuth flow + user info.

use serde::{Deserialize, Serialize};

// ── Response types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LarkTokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LarkUser {
    pub open_id: String,
    pub user_id: Option<String>,
    pub name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub department: Option<String>,
}

// ── Internal: Lark API envelope ──────────────────────────────────────────────

/// Every Lark API response wraps the payload in `{"code": 0, "msg"/"message": "...", "data": ...}`.
#[derive(Debug, Deserialize)]
pub struct LarkEnvelope<T> {
    pub code: i64,
    #[serde(alias = "message")]
    pub msg: Option<String>,
    pub data: Option<T>,
}

/// The app_access_token endpoint returns fields at the top level (no `data` wrapper).
#[derive(Debug, Deserialize)]
struct AppTokenResponse {
    code: i64,
    msg: String,
    app_access_token: Option<String>,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Build the Lark OAuth authorize URL that the browser should redirect to.
///
/// We request an explicit `scope` so the issued user_access_token carries the
/// permissions meeting-minutes export needs (create a Lark doc as the user,
/// read its share URL, create tasks) on top of the identity scope. Some tenants
/// don't grant app permissions to user tokens unless they're requested here.
/// Lark accumulates granted scopes across authorizations, so adding these does
/// not strip a user's previously-granted permissions. Overridable via
/// `LARK_OAUTH_SCOPES` (space-separated) if a tenant needs a different set.
pub fn build_oauth_url(app_id: &str, redirect_uri: &str, state: &str) -> String {
    let encoded_redirect = urlencoding::encode(redirect_uri);
    let scopes = std::env::var("LARK_OAUTH_SCOPES")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            "contact:user.base:readonly docx:document drive:drive task:task".to_string()
        });
    let encoded_scope = urlencoding::encode(&scopes);
    format!(
        "https://open.larksuite.com/open-apis/authen/v1/authorize\
         ?app_id={app_id}\
         &redirect_uri={encoded_redirect}\
         &response_type=code\
         &scope={encoded_scope}\
         &state={state}"
    )
}

/// Exchange a user authorization code for an access + refresh token pair.
pub async fn exchange_code(
    app_id: &str,
    app_secret: &str,
    code: &str,
) -> Result<LarkTokenResponse, String> {
    let app_token = get_app_access_token(app_id, app_secret).await?;

    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.larksuite.com/open-apis/authen/v1/oidc/access_token")
        .header("Authorization", format!("Bearer {app_token}"))
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
        }))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Lark exchange_code request failed: {e}");
            format!("exchange_code request failed: {e}")
        })?;

    let raw = resp.text().await.map_err(|e| {
        tracing::error!("Lark exchange_code read body failed: {e}");
        format!("exchange_code read body failed: {e}")
    })?;
    tracing::info!("Lark exchange_code raw response: {raw}");

    let envelope: LarkEnvelope<LarkTokenResponse> = serde_json::from_str(&raw).map_err(|e| {
        tracing::error!("Lark exchange_code parse failed: {e}");
        format!("exchange_code parse failed: {e}")
    })?;

    if envelope.code != 0 {
        tracing::error!(
            "Lark exchange_code API error {}: {}",
            envelope.code,
            envelope.msg.as_deref().unwrap_or("unknown")
        );
        return Err(format!(
            "Lark API error {}: {}",
            envelope.code,
            envelope.msg.as_deref().unwrap_or("unknown")
        ));
    }

    envelope
        .data
        .ok_or_else(|| "exchange_code: response had code 0 but no data".to_string())
}

/// Refresh an expired user access token using a refresh token.
pub async fn refresh_access_token(
    app_id: &str,
    app_secret: &str,
    refresh_token: &str,
) -> Result<LarkTokenResponse, String> {
    let app_token = get_app_access_token(app_id, app_secret).await?;

    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.larksuite.com/open-apis/authen/v1/oidc/refresh_access_token")
        .header("Authorization", format!("Bearer {app_token}"))
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Lark refresh_access_token request failed: {e}");
            format!("refresh_access_token request failed: {e}")
        })?;

    let envelope: LarkEnvelope<LarkTokenResponse> = resp.json().await.map_err(|e| {
        tracing::error!("Lark refresh_access_token parse failed: {e}");
        format!("refresh_access_token parse failed: {e}")
    })?;

    if envelope.code != 0 {
        tracing::error!(
            "Lark refresh_access_token API error {}: {}",
            envelope.code,
            envelope.msg.as_deref().unwrap_or("unknown")
        );
        return Err(format!(
            "Lark API error {}: {}",
            envelope.code,
            envelope.msg.as_deref().unwrap_or("unknown")
        ));
    }

    envelope
        .data
        .ok_or_else(|| "refresh_access_token: response had code 0 but no data".to_string())
}

/// Fetch the authenticated user's profile from Lark.
pub async fn get_user_info(access_token: &str) -> Result<LarkUser, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://open.larksuite.com/open-apis/authen/v1/user_info")
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Lark get_user_info request failed: {e}");
            format!("get_user_info request failed: {e}")
        })?;

    let envelope: LarkEnvelope<LarkUser> = resp.json().await.map_err(|e| {
        tracing::error!("Lark get_user_info parse failed: {e}");
        format!("get_user_info parse failed: {e}")
    })?;

    if envelope.code != 0 {
        tracing::error!(
            "Lark get_user_info API error {}: {}",
            envelope.code,
            envelope.msg.as_deref().unwrap_or("unknown")
        );
        return Err(format!(
            "Lark API error {}: {}",
            envelope.code,
            envelope.msg.as_deref().unwrap_or("unknown")
        ));
    }

    envelope
        .data
        .ok_or_else(|| "get_user_info: response had code 0 but no data".to_string())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Obtain an internal app_access_token (tenant-level) used to authorize
/// subsequent user-token API calls.
pub async fn get_app_access_token(app_id: &str, app_secret: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.larksuite.com/open-apis/auth/v3/app_access_token/internal")
        .json(&serde_json::json!({
            "app_id": app_id,
            "app_secret": app_secret,
        }))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Lark app_access_token request failed: {e}");
            format!("app_access_token request failed: {e}")
        })?;

    let parsed: AppTokenResponse = resp.json().await.map_err(|e| {
        tracing::error!("Lark app_access_token parse failed: {e}");
        format!("app_access_token parse failed: {e}")
    })?;

    if parsed.code != 0 {
        tracing::error!(
            "Lark app_access_token API error {}: {}",
            parsed.code,
            parsed.msg
        );
        return Err(format!("Lark API error {}: {}", parsed.code, parsed.msg));
    }

    parsed
        .app_access_token
        .ok_or_else(|| "app_access_token: response had code 0 but no token".to_string())
}
