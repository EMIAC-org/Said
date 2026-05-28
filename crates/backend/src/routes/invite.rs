//! POST /v1/invite
//!
//! Sends an invite email via Resend (https://resend.com).
//!
//! Configuration — two ways, runtime env wins over build-time:
//!
//!   1. Build-time bake (ship-with-binary):
//!        RESEND_API_KEY=re_xxx pnpm tauri build
//!      The key is read at *compile* time via `option_env!` and baked
//!      into the sidecar binary. No env needed at runtime.
//!      ⚠️  The key is extractable from the binary by anyone with the .app
//!      — fine for shipping to friends, NOT safe for public distribution.
//!
//!   2. Runtime env (server / dev):
//!        RESEND_API_KEY=re_xxx ./airnote-backend
//!      Useful if you later move the backend off-device.
//!
//!   RESEND_FROM is optional. Defaults to "AirNote <onboarding@resend.dev>"
//!   which works without DNS but lands in spam more often. Once you've
//!   verified a domain in Resend, set it to "AirNote <hello@yourdomain.com>".
//!
//! When neither source provides a key, returns 503 with body
//! `{ "error": "email_not_configured" }` so the desktop client falls
//! back to opening the user's mail app via `mailto:`.

/// Build-time key (baked in if RESEND_API_KEY was set during `cargo build`).
const BUILD_TIME_KEY: Option<&str> = option_env!("RESEND_API_KEY");
const BUILD_TIME_FROM: Option<&str> = option_env!("RESEND_FROM");

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::AppState;

#[derive(Deserialize)]
pub struct InviteBody {
    pub to: String,
}

const SUBJECT: &str = "You should try AirNote";

const BODY_TEXT: &str = "\
Hey — I've been using AirNote to dictate and polish text.
It's quietly become my favourite way to write.

Thought you'd like it: https://airnote.app
";

const BODY_HTML: &str = "\
<div style=\"font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;font-size:15px;line-height:1.55;color:#1a1a1a;max-width:560px;\">
  <p>Hey —</p>
  <p>I've been using <strong>AirNote</strong> to dictate and polish text.<br/>It's quietly become my favourite way to write.</p>
  <p>Thought you'd like it: <a href=\"https://airnote.app\" style=\"color:#5b9b6a;font-weight:600;text-decoration:none;\">airnote.app</a></p>
</div>
";

pub async fn send(
    State(state): State<AppState>,
    Json(body): Json<InviteBody>,
) -> (StatusCode, Json<serde_json::Value>) {
    let to = body.to.trim();
    if to.is_empty() || !to.contains('@') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_email" })),
        );
    }

    // Runtime env wins; otherwise fall back to the build-time bake.
    let api_key = std::env::var("RESEND_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            BUILD_TIME_KEY
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        });

    let api_key = match api_key {
        Some(k) => k,
        None => {
            warn!(
                "[invite] no RESEND_API_KEY (build-time or runtime) — returning email_not_configured"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "email_not_configured" })),
            );
        }
    };

    let from = std::env::var("RESEND_FROM")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            BUILD_TIME_FROM
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "AirNote <onboarding@resend.dev>".into());

    let payload = json!({
        "from":    from,
        "to":      [to],
        "subject": SUBJECT,
        "text":    BODY_TEXT,
        "html":    BODY_HTML,
    });

    let resp = state
        .http_client
        .post("https://api.resend.com/emails")
        .bearer_auth(&api_key)
        .json(&payload)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            info!("[invite] sent to {to}");
            (StatusCode::OK, Json(json!({ "ok": true })))
        }
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            warn!("[invite] resend error {status}: {text}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "send_failed", "detail": text })),
            )
        }
        Err(e) => {
            warn!("[invite] network error: {e}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "send_failed", "detail": e.to_string() })),
            )
        }
    }
}
