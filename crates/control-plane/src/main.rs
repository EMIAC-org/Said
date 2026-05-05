//! Voice Polish Studio — Cloud Control Plane
//!
//! A thin axum service that owns ONLY: accounts, sessions, license keys, and
//! aggregate usage metrics. All user content stays on the local Mac.
//!
//! Configure via env vars (or .env file):
//!   DATABASE_URL  — Neon Postgres connection string
//!   PORT          — listen port (default 3100)

mod auth;
mod routes;
mod store;

use std::{sync::Arc, time::Instant};

use axum::{
    Router,
    http::{Method, header},
    routing::{get, post},
};
use clap::Parser;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "control-plane",
    version,
    about = "Voice Polish Studio cloud control plane"
)]
struct Cli {
    /// TCP port to listen on
    #[arg(long, env = "PORT", default_value = "3100")]
    port: u16,

    /// Postgres database URL
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub db: store::Db,
    pub started_at: Arc<Instant>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // 1. Load .env (optional — Fly.io / Railway inject vars directly)
    let _ = dotenvy::dotenv();

    // 2. Tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 3. Parse CLI / env
    let cli = Cli::parse();
    info!("[cp] starting on port {}", cli.port);

    // 4. Connect to Postgres + apply schema
    let db = store::connect(&cli.database_url)
        .await
        .expect("failed to connect to Postgres");

    let state = AppState {
        db,
        started_at: Arc::new(Instant::now()),
    };

    // 5. CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    // 6. Router
    let app = Router::new()
        // Public
        .route("/v1/health", get(routes::health::handler))
        .route("/v1/auth/signup", post(routes::auth::signup))
        .route("/v1/auth/login", post(routes::auth::login))
        // Authenticated
        .route("/v1/auth/logout", post(routes::auth::logout))
        .route("/v1/auth/me", get(routes::auth::me))
        .route("/v1/license/check", get(routes::license::check))
        .route("/v1/metering/report", post(routes::metering::report))
        .layer(cors)
        .with_state(state);

    // 7. Graceful shutdown on Ctrl-C / SIGTERM
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
        info!("[cp] shutting down");
    };

    // 8. Serve
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .expect("failed to bind");

    info!("[cp] listening on 0.0.0.0:{}", cli.port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server failed");
}
