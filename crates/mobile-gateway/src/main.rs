//! AirNote Mobile Gateway — entry point.
//!
//! Configure via env vars (or a `.env` file):
//!   DATABASE_URL       — Postgres connection string (its own DB, not control-plane's)
//!   PORT               — listen port (default 3200)
//!   DEEPGRAM_API_KEY   — Deepgram STT key (empty ⇒ deterministic mock pipeline)
//!   GATEWAY_API_KEY    — Groq/LLM polish key (empty ⇒ deterministic mock polish)
//!   MOBILE_LLM_MODEL   — polish model (default llama-3.1-8b-instant)
//!   MOBILE_LLM_BASE_URL— OpenAI-compatible chat endpoint (default Groq)
//!   GATEWAY_REGION     — region hint surfaced in bootstrap (default ap-south-1)

use std::sync::Arc;
use std::time::Instant;

use clap::Parser;
use tracing::info;

use airnote_mobile_gateway::{AppState, build_router, store};

#[derive(Parser)]
#[command(
    name = "mobile-gateway",
    version,
    about = "AirNote iOS mobile runtime gateway"
)]
struct Cli {
    #[arg(long, env = "PORT", default_value = "3200")]
    port: u16,

    #[arg(long, env = "DATABASE_URL")]
    database_url: String,

    #[arg(long, env = "DEEPGRAM_API_KEY", default_value = "")]
    deepgram_api_key: String,

    /// Groq / LLM polish key (OpenAI-compatible, Bearer auth).
    #[arg(long, env = "GATEWAY_API_KEY", default_value = "")]
    llm_api_key: String,

    #[arg(
        long,
        env = "MOBILE_LLM_MODEL",
        default_value = "llama-3.1-8b-instant"
    )]
    llm_model: String,

    #[arg(
        long,
        env = "MOBILE_LLM_BASE_URL",
        default_value = "https://api.groq.com/openai/v1/chat/completions"
    )]
    llm_base_url: String,

    #[arg(long, env = "GATEWAY_REGION", default_value = "ap-south-1")]
    gateway_region: String,
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    info!("[mobile-gateway] starting on port {}", cli.port);

    let db = store::connect(&cli.database_url)
        .await
        .expect("failed to connect to Postgres");

    let http = reqwest::Client::builder()
        .build()
        .expect("failed to build HTTP client");

    let state = AppState {
        db,
        started_at: Arc::new(Instant::now()),
        http,
        deepgram_api_key: cli.deepgram_api_key,
        llm_api_key: cli.llm_api_key,
        llm_model: cli.llm_model,
        llm_base_url: cli.llm_base_url,
        gateway_region: cli.gateway_region,
    };

    if state.deepgram_api_key.trim().is_empty() || state.llm_api_key.trim().is_empty() {
        info!("[mobile-gateway] provider keys missing — voice runs in deterministic MOCK mode");
    }

    let app = build_router(state);

    let shutdown = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c().await.expect("ctrl-c handler");
        };
        #[cfg(unix)]
        let sigterm = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("sigterm handler")
                .recv()
                .await;
        };
        #[cfg(not(unix))]
        let sigterm = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c  => {}
            _ = sigterm => {}
        }
        info!("[mobile-gateway] shutting down");
    };

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .expect("failed to bind");

    info!("[mobile-gateway] listening on 0.0.0.0:{}", cli.port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server failed");
}
