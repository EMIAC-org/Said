//! OpenAI Codex OAuth 2.0 + PKCE flow.
//!
//! Routes (all require shared-secret bearer):
//!   POST /v1/openai-oauth/initiate   — returns auth_url, spawns 1455 callback server
//!   GET  /v1/openai-oauth/status     — returns {connected, expires_at?, model_smart, model_mini}
//!   DELETE /v1/openai-oauth/disconnect — revokes local token, reverts llm_provider → gateway
//!
//! The OAuth callback lands on http://localhost:1455/auth/callback (a one-shot
//! axum server spawned during initiate).  That server exchanges the code, saves
//! tokens to SQLite, and returns a friendly HTML success page.

use axum::{extract::State, http::StatusCode, response::Json};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tracing::{info, warn};

use crate::{AppState, llm::openai_codex, store::openai_oauth};

// ── OAuth constants (same as the official Codex CLI) ─────────────────────────

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";

// ── PKCE session (module-level singleton, single-user daemon) ─────────────────

struct PkceSession {
    verifier: String,
    state: String,
}

static PENDING: OnceLock<Mutex<Option<PkceSession>>> = OnceLock::new();
fn pending() -> &'static Mutex<Option<PkceSession>> {
    PENDING.get_or_init(|| Mutex::new(None))
}

// ── PKCE helpers ──────────────────────────────────────────────────────────────

fn pkce() -> (String, String) {
    use sha2::{Digest, Sha256};
    let verifier = random_base64url(32);
    let challenge = base64url_encode(Sha256::digest(verifier.as_bytes()).as_slice());
    (verifier, challenge)
}

fn random_base64url(n: usize) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Deterministic but unique enough for local use — combine timestamp + thread-id
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let seed = hasher.finish();

    // XOR-expand into `n` bytes, then base64url-encode
    let mut bytes = vec![0u8; n];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((seed >> (i % 8 * 8)) ^ (i as u64)) as u8;
    }
    base64url_encode(&bytes)
}

fn base64url_encode(bytes: &[u8]) -> String {
    // Manual base64url (no padding) — avoids adding a dep just for this
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        out.push(CHARS[b0 >> 2] as char);
        out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        out.push(CHARS[b2 & 0x3f] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let b0 = bytes[i] as usize;
            out.push(CHARS[b0 >> 2] as char);
            out.push(CHARS[(b0 & 3) << 4] as char);
        }
        2 => {
            let b0 = bytes[i] as usize;
            let b1 = bytes[i + 1] as usize;
            out.push(CHARS[b0 >> 2] as char);
            out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
            out.push(CHARS[(b1 & 0xf) << 2] as char);
        }
        _ => {}
    }
    out
}

fn random_state() -> String {
    random_base64url(16)
}

// ── Route handlers ────────────────────────────────────────────────────────────

/// POST /v1/openai-oauth/initiate
/// Returns the auth URL to open in the browser and spawns the 1455 callback listener.
pub async fn initiate(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let (verifier, challenge) = pkce();
    let oauth_state = random_state();

    // Store PKCE session
    {
        let mut lock = pending()
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        *lock = Some(PkceSession {
            verifier: verifier.clone(),
            state: oauth_state.clone(),
        });
    }

    // Build auth URL
    let params = [
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", &oauth_state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
    ];
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let auth_url = format!("{AUTH_URL}?{query}");

    info!("[openai_oauth] initiate — spawning 1455 callback server");

    // Spawn one-shot callback server on 1455
    let pool = state.pool.clone();
    let user_id = state.default_user_id.as_str().to_string();
    tokio::spawn(async move {
        if let Err(e) = run_callback_server(pool, user_id).await {
            warn!("[openai_oauth] callback server error: {e}");
        }
    });

    Ok(Json(json!({ "auth_url": auth_url })))
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

/// GET /v1/openai-oauth/status
pub async fn status(State(state): State<AppState>) -> Json<Value> {
    let user_id = state.default_user_id.as_str();
    match openai_oauth::get_token(&state.pool, user_id) {
        None => Json(json!({
            "connected":   false,
            "model_smart": openai_codex::MODEL_SMART,
            "model_mini":  openai_codex::MODEL_MINI,
        })),
        Some(tok) => {
            let now_ms = crate::store::now_ms();
            let expired = tok.expires_at > 0 && tok.expires_at < now_ms;
            Json(json!({
                "connected":    !expired,
                "expires_at":   tok.expires_at,
                "connected_at": tok.connected_at,
                "model_smart":  openai_codex::MODEL_SMART,
                "model_mini":   openai_codex::MODEL_MINI,
            }))
        }
    }
}

/// DELETE /v1/openai-oauth/disconnect
pub async fn disconnect(State(state): State<AppState>) -> StatusCode {
    let user_id = state.default_user_id.as_str();
    openai_oauth::delete_token(&state.pool, user_id);
    info!("[openai_oauth] disconnected");
    StatusCode::NO_CONTENT
}

// ── One-shot callback server on port 1455 ─────────────────────────────────────

async fn run_callback_server(pool: crate::store::DbPool, user_id: String) -> anyhow::Result<()> {
    use axum::{Router, extract::Query, routing::get};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    // Try to bind; if port is already in use (previous flow stuck), return early
    let listener = match TcpListener::bind("127.0.0.1:1455").await {
        Ok(l) => l,
        Err(e) => {
            warn!("[openai_oauth] cannot bind 1455: {e}");
            return Ok(());
        }
    };

    info!("[openai_oauth] 1455 listening for OAuth callback…");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));

    #[derive(Deserialize)]
    struct CallbackParams {
        code: Option<String>,
        state: Option<String>,
        error: Option<String>,
    }

    let pool2 = pool.clone();
    let uid2 = user_id.clone();
    let stx = shutdown_tx.clone();

    let app = Router::new().route(
        "/auth/callback",
        get(move |Query(params): Query<CallbackParams>| {
            let pool3 = pool2.clone();
            let uid3 = uid2.clone();
            let stx2 = stx.clone();
            async move {
                // Consume shutdown sender after first request
                let sender = { stx2.lock().await.take() };

                if let Some(err) = params.error {
                    warn!("[openai_oauth] callback error: {err}");
                    let _ = sender.map(|s| s.send(()));
                    return axum::response::Html(error_page(&err));
                }

                let (code, cb_state) = match (params.code, params.state) {
                    (Some(c), Some(s)) => (c, s),
                    _ => {
                        let _ = sender.map(|s| s.send(()));
                        return axum::response::Html(error_page("missing code or state"));
                    }
                };

                // Validate state
                let session = {
                    let mut lock = pending().lock().unwrap();
                    lock.take()
                };
                let session = match session {
                    Some(s) if s.state == cb_state => s,
                    Some(_) => {
                        let _ = sender.map(|s| s.send(()));
                        return axum::response::Html(error_page("state mismatch"));
                    }
                    None => {
                        let _ = sender.map(|s| s.send(()));
                        return axum::response::Html(error_page("no pending session"));
                    }
                };

                // Exchange code for tokens
                match exchange_code(&code, &session.verifier).await {
                    Ok((access, refresh, expires_at)) => {
                        openai_oauth::save_token(
                            &pool3,
                            &uid3,
                            &access,
                            Some(&refresh),
                            expires_at,
                        );
                        info!("[openai_oauth] ✓ connected, expires_at={expires_at}");
                        let _ = sender.map(|s| s.send(()));
                        axum::response::Html(success_page())
                    }
                    Err(e) => {
                        warn!("[openai_oauth] exchange error: {e}");
                        let _ = sender.map(|s| s.send(()));
                        axum::response::Html(error_page(&e))
                    }
                }
            }
        }),
    );

    // Serve with 5-minute hard timeout
    tokio::select! {
        r = axum::serve(listener, app).with_graceful_shutdown(async { shutdown_rx.await.ok(); }) => {
            r?;
        }
        _ = tokio::time::sleep(Duration::from_secs(300)) => {
            info!("[openai_oauth] 1455 server timed out after 5 min");
        }
    }

    info!("[openai_oauth] 1455 server shut down");
    Ok(())
}

async fn exchange_code(code: &str, verifier: &str) -> Result<(String, String, i64), String> {
    let client = Client::new();
    let payload = json!({
        "client_id":     CLIENT_ID,
        "code":          code,
        "code_verifier": verifier,
        "redirect_uri":  REDIRECT_URI,
        "grant_type":    "authorization_code",
    });

    let resp = client
        .post(TOKEN_URL)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token error {status}: {body}"));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("token parse failed: {e}"))?;

    let access_token = v["access_token"]
        .as_str()
        .ok_or("missing access_token")?
        .to_string();
    let refresh_token = v["refresh_token"].as_str().unwrap_or("").to_string();
    let expires_in = v["expires_in"].as_i64().unwrap_or(864_000);
    let expires_at = crate::store::now_ms() + expires_in * 1000;

    Ok((access_token, refresh_token, expires_at))
}

// ── HTML pages ────────────────────────────────────────────────────────────────

fn success_page() -> String {
    r#"<!doctype html><html><head><meta charset="utf-8">
    <style>
      body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;
           display:flex;align-items:center;justify-content:center;
           min-height:100vh;margin:0;background:#0a0a0a;color:#e5e5e5;}
      .card{max-width:360px;text-align:center;padding:40px 32px;
            background:#141414;border:1px solid #262626;border-radius:20px;}
      .icon{font-size:48px;margin-bottom:16px;}
      h1{font-size:22px;font-weight:700;margin:0 0 8px;}
      p{font-size:14px;color:#71717a;margin:0 0 24px;line-height:1.5;}
      .badge{display:inline-flex;gap:6px;align-items:center;
             padding:6px 14px;border-radius:99px;
             background:rgba(132,204,22,.1);color:#84cc16;
             font-size:13px;font-weight:600;}
    </style></head><body>
    <div class="card">
      <div class="icon">✓</div>
      <h1>ChatGPT Connected</h1>
      <p>Your OpenAI account has been linked. You can close this tab and return to the app.</p>
      <div class="badge">● gpt-5.4 &amp; gpt-5.4-mini ready</div>
    </div>
    <script>window.close();</script>
    </body></html>"#
        .to_string()
}

fn error_page(msg: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
    <style>
      body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;
           display:flex;align-items:center;justify-content:center;
           min-height:100vh;margin:0;background:#0a0a0a;color:#e5e5e5;}}
      .card{{max-width:360px;text-align:center;padding:40px 32px;
            background:#141414;border:1px solid #262626;border-radius:20px;}}
      .icon{{font-size:48px;margin-bottom:16px;}}
      h1{{font-size:22px;font-weight:700;margin:0 0 8px;color:#f87171;}}
      p{{font-size:14px;color:#71717a;margin:0;line-height:1.5;}}
    </style></head><body>
    <div class="card">
      <div class="icon">✗</div>
      <h1>Connection Failed</h1>
      <p>{msg}</p>
    </div>
    </body></html>"#
    )
}
