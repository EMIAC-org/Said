//! AirNote Mobile Gateway — the iOS app's hosted runtime.
//!
//! A standalone Axum service that owns the entire iOS server side: mobile auth,
//! voice sessions, the STT → LLM polish → Hinglish script-guard pipeline
//! (streaming + batch), privacy-safe event ingestion, and personal vocabulary.
//!
//! It is intentionally isolated from the desktop/enterprise `control-plane`:
//! its own database, its own accounts, its own deploy. The control-plane has no
//! connection to the iOS app.

pub mod auth;
pub mod routes;
pub mod runtime;
pub mod store;
pub mod util;

use std::sync::Arc;
use std::time::Instant;

use axum::{
    Router,
    http::{Method, header},
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};

/// Default OpenAI-compatible chat endpoint (Groq) used for polish.
pub const DEFAULT_LLM_BASE_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
/// Default fast polish model.
pub const DEFAULT_LLM_MODEL: &str = "llama-3.1-8b-instant";

#[derive(Clone)]
pub struct AppState {
    pub db: store::Db,
    pub started_at: Arc<Instant>,
    /// Shared HTTP client for Groq polish + Deepgram batch STT.
    pub http: reqwest::Client,
    pub deepgram_api_key: String,
    /// LLM key (GATEWAY_API_KEY) — Groq by default.
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_base_url: String,
    pub gateway_region: String,
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    Router::new()
        // Health + public bootstrap
        .route("/v1/health", get(routes::health::handler))
        .route("/v1/mobile/bootstrap", get(routes::bootstrap::bootstrap))
        // Auth (self-contained)
        .route("/v1/auth/mobile-email", post(routes::auth::mobile_email))
        .route("/v1/auth/mobile-refresh", post(routes::auth::mobile_refresh))
        // Runtime config
        .route("/v1/runtime/config", get(routes::bootstrap::config))
        // Voice sessions
        .route(
            "/v1/runtime/sessions",
            post(routes::sessions::create_session),
        )
        .route("/v1/mobile/sessions", post(routes::sessions::create_session))
        // Privacy-safe events
        .route("/v1/runtime/events", post(routes::events::ingest_event))
        .route("/v1/mobile/events", post(routes::events::ingest_event))
        // Voice pipeline — streaming WS + batch fallback
        .route("/v1/runtime/voice", get(routes::voice::voice_ws))
        .route("/v1/runtime/voice/batch", post(routes::voice::dictate_batch))
        .route("/v1/mobile/dictate", post(routes::voice::dictate_batch))
        // Vocabulary snapshot + explicit learning
        .route("/v1/mobile/vocab/snapshot", get(routes::vocab::snapshot))
        .route("/v1/mobile/vocab/terms", post(routes::vocab::add_term))
        .route("/v1/mobile/feedback", post(routes::vocab::feedback))
        .layer(cors)
        .with_state(state)
}
