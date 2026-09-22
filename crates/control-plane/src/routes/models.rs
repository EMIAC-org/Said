//! GET /v1/models/dictation — hand the desktop a download URL for the local
//! dictation model.
//!
//! The model lives in a private Hugging Face repository. Shipping the HF token
//! inside the desktop app would make that repository effectively public: anyone
//! can open a DMG and read the string out. So the token stays here, and the
//! desktop asks this endpoint for a URL instead.
//!
//! Hugging Face answers an authenticated `resolve` request with a 302 to a
//! pre-signed CDN URL that is valid for about an hour. We return that URL, so
//! the 141 MB download goes straight from the CDN to the user and never passes
//! through this server. The token never leaves this process.
//!
//! The route is deliberately unauthenticated: the desktop needs the model
//! during onboarding, before an account exists. What we are protecting is the
//! token and write access to the repository, not the model bytes themselves.

use std::sync::{Arc, Mutex};

use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};

use crate::AppState;

/// Private repository holding the 41-hour Hinglish fine-tune.
const HF_REPO: &str = "Marquestra/clario-hinglish-stt-41h";

/// Pinned commit. An unpinned `main` would let a repository push change what
/// every desktop installs without a client release; dictation quality is not
/// something to ship by accident.
const HF_REVISION: &str = "3ace1a98541224c935cc3e6a36175869ac3117d8";

/// whisper.cpp GGML artifact. The repo also carries `model.safetensors`, which
/// is the transformers format and useless to our runtime.
const HF_FILENAME: &str = "ggml-model-fp16.bin";

/// Exact size, used by the desktop for progress and for rejecting a truncated
/// download before it is installed.
const MODEL_SIZE_BYTES: u64 = 147_951_465;

/// SHA-256 of the artifact at `HF_REVISION`, so the desktop can prove it
/// installed the file we meant to serve.
const MODEL_SHA256: &str = "a54258c2e9a7dc2ec664fbaaabd0b71264933c05a89c6ae7f548db121d6acaeb";

/// Model identifier the desktop stores in preferences.
const MODEL_KEY: &str = "clario-hinglish-41h";

/// Human-readable name for the UI.
const MODEL_NAME: &str = "AirNote Hinglish (41h)";

/// Hugging Face signs CDN URLs for roughly an hour. Re-using one for 45 minutes
/// keeps us well inside that window while sparing the HF API a round trip on
/// every desktop launch.
const CACHE_TTL_SECS: u64 = 45 * 60;

/// A signed URL plus the instant it stops being safe to hand out.
///
/// Public only because it appears inside [`SignedUrlCache`], which `AppState`
/// holds; the fields stay private so nothing outside this module can forge one.
#[derive(Clone)]
pub struct CachedUrl {
    url: String,
    fetched_at: std::time::Instant,
}

/// Shared across requests so a launch spike costs one upstream call, not one
/// per user.
pub type SignedUrlCache = Arc<Mutex<Option<CachedUrl>>>;

pub fn new_cache() -> SignedUrlCache {
    Arc::new(Mutex::new(None))
}

pub async fn dictation(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let url = signed_url(&state).await?;

    Ok(Json(json!({
        "key":         MODEL_KEY,
        "name":        MODEL_NAME,
        "filename":    HF_FILENAME,
        "url":         url,
        "size_bytes":  MODEL_SIZE_BYTES,
        "sha256":      MODEL_SHA256,
        "revision":    HF_REVISION,
        // The desktop should treat the URL as short-lived and re-request rather
        // than storing it across launches.
        "expires_in_s": CACHE_TTL_SECS,
    })))
}

/// Return a cached signed URL, or ask Hugging Face for a fresh one.
async fn signed_url(state: &AppState) -> Result<String, (StatusCode, Json<Value>)> {
    if let Some(hit) = cached(&state.model_url_cache) {
        return Ok(hit);
    }

    let token = state.hf_token.trim();
    if token.is_empty() {
        tracing::error!("[models] HF_TOKEN is not configured — cannot serve the dictation model");
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "Model download is not configured on this server",
        ));
    }

    let resolve = format!("https://huggingface.co/{HF_REPO}/resolve/{HF_REVISION}/{HF_FILENAME}");

    // `redirect(none)` is the whole trick: we want the Location header, not the
    // 141 MB body that following it would stream through this process.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| {
            tracing::error!("[models] could not build HTTP client: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Model download is temporarily unavailable",
            )
        })?;

    let response = client
        .get(&resolve)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("[models] Hugging Face request failed: {e}");
            err(
                StatusCode::BAD_GATEWAY,
                "Could not reach the model host. Please try again.",
            )
        })?;

    let status = response.status();
    if !status.is_redirection() {
        // 401/403 here means the server's token is wrong or was revoked, which
        // is an operator problem. Say so in the log; keep it vague to callers.
        tracing::error!(
            "[models] expected a redirect from Hugging Face, got {status} for {HF_REPO}"
        );
        return Err(err(
            StatusCode::BAD_GATEWAY,
            "Could not reach the model host. Please try again.",
        ));
    }

    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            tracing::error!("[models] Hugging Face redirect carried no Location header");
            err(
                StatusCode::BAD_GATEWAY,
                "Could not reach the model host. Please try again.",
            )
        })?;

    store(&state.model_url_cache, &location);
    tracing::info!("[models] issued a fresh signed URL for the dictation model");
    Ok(location)
}

fn cached(cache: &SignedUrlCache) -> Option<String> {
    let guard = cache.lock().ok()?;
    let entry = guard.as_ref()?;
    if entry.fetched_at.elapsed().as_secs() < CACHE_TTL_SECS {
        Some(entry.url.clone())
    } else {
        None
    }
}

fn store(cache: &SignedUrlCache, url: &str) {
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(CachedUrl {
            url: url.to_string(),
            fetched_at: std::time::Instant::now(),
        });
    }
}

fn err(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_entry_is_served_from_cache() {
        let cache = new_cache();
        store(&cache, "https://cdn.example/signed");
        assert_eq!(
            cached(&cache).as_deref(),
            Some("https://cdn.example/signed")
        );
    }

    #[test]
    fn an_expired_entry_is_refetched() {
        let cache = new_cache();
        *cache.lock().unwrap() = Some(CachedUrl {
            url: "https://cdn.example/stale".to_string(),
            fetched_at: std::time::Instant::now()
                - std::time::Duration::from_secs(CACHE_TTL_SECS + 1),
        });
        assert_eq!(cached(&cache), None);
    }

    #[test]
    fn an_empty_cache_reports_a_miss() {
        assert_eq!(cached(&new_cache()), None);
    }

    #[test]
    fn the_pinned_revision_is_a_full_commit_sha() {
        // A branch name here would let a repo push change what ships.
        assert_eq!(HF_REVISION.len(), 40);
        assert!(HF_REVISION.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
