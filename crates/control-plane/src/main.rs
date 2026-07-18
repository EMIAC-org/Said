//! Voice Polish Studio — Cloud Control Plane
//!
//! A thin axum service that owns ONLY: accounts, sessions, license keys, and
//! aggregate usage metrics. All user content stays on the local Mac.
//!
//! Configure via env vars (or .env file):
//!   DATABASE_URL  — Neon Postgres connection string
//!   PORT          — listen port (default 3100)

use std::sync::Arc;
use std::time::Instant;

use clap::Parser;
use tracing::info;

use said_control_plane::{
    AppState, LarkConfig, ai_worker, build_router, meeting_hub, memory_hygiene_worker,
    notification_hub, notification_worker, routes, store, vocab_worker,
};

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

    /// Lark Open API app ID
    #[arg(long, env = "LARK_APP_ID", default_value = "")]
    lark_app_id: String,

    /// Lark Open API app secret
    #[arg(long, env = "LARK_APP_SECRET", default_value = "")]
    lark_app_secret: String,

    /// Lark OAuth redirect URI
    #[arg(long, env = "LARK_REDIRECT_URI", default_value = "")]
    lark_redirect_uri: String,

    /// JWT signing secret for session tokens
    #[arg(
        long,
        env = "JWT_SECRET",
        default_value = "airnote-enterprise-dev-secret"
    )]
    jwt_secret: String,

    /// OpenAI API key used as a platform fallback for text-model provider calls.
    #[arg(long, env = "OPENAI_API_KEY", default_value = "")]
    openai_api_key: String,

    /// Groq API key for server-runtime polish latency probes
    #[arg(long, env = "GROQ_API_KEY", default_value = "")]
    groq_api_key: String,

    /// DeepInfra API key for paid Gemma 4 26B A4B server-runtime polish.
    #[arg(long, env = "DEEPINFRA_API_KEY", default_value = "")]
    deepinfra_api_key: String,

    /// Legacy gateway key fallback for server-runtime polish latency probes
    #[arg(long, env = "GATEWAY_API_KEY", default_value = "")]
    gateway_api_key: String,

    /// Divo agent backend base URL (AirNote ⇄ Divo proxy target)
    #[arg(
        long,
        env = "DIVO_BASE_URL",
        default_value = "https://divo.outreachdeal.com"
    )]
    divo_base_url: String,

    /// Secret used to encrypt runtime/BYOK provider credentials at rest.
    #[arg(long, env = "RUNTIME_CREDENTIALS_KEY", default_value = "")]
    runtime_credentials_key: String,

    /// Internal workspace whose COMPANY_ADMIN members can view platform-wide
    /// observability. Empty disables platform-wide access.
    #[arg(long, env = "PLATFORM_ADMIN_ORG_SLUG", default_value = "")]
    platform_admin_org_slug: String,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    install_rustls_crypto_provider();

    load_dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    info!("[cp] starting on port {}", cli.port);

    let db = store::connect(&cli.database_url)
        .await
        .expect("failed to connect to Postgres");

    let lark = LarkConfig {
        app_id: cli.lark_app_id,
        app_secret: cli.lark_app_secret,
        redirect_uri: cli.lark_redirect_uri,
        jwt_secret: cli.jwt_secret,
    };

    let hub = meeting_hub::MeetingHub::new(db.clone());
    let notifications = notification_hub::NotificationHub::new();

    // Start the AI background worker (processes closed meeting slots).
    ai_worker::start_ai_worker(db.clone(), hub.clone());

    // Start the notification worker (15-min reminders + urgent join alerts).
    notification_worker::start_notification_worker(db.clone(), lark.clone(), hub.clone());

    // Build privacy-safe org vocabulary suggestions once daily.
    vocab_worker::start_vocab_aggregation_worker(db.clone());
    memory_hygiene_worker::start_memory_hygiene_worker(db.clone());

    let groq_api_key = if cli.groq_api_key.trim().is_empty() {
        cli.gateway_api_key.clone()
    } else {
        cli.groq_api_key.clone()
    };
    info!(
        "[cp] runtime credential env: openai={} groq={} deepinfra={} deepseek={} runtime_credentials_key={}",
        !cli.openai_api_key.trim().is_empty(),
        !groq_api_key.trim().is_empty(),
        !cli.deepinfra_api_key.trim().is_empty(),
        !std::env::var("DEEPSEEK_API_KEY")
            .unwrap_or_default()
            .trim()
            .is_empty(),
        !cli.runtime_credentials_key.trim().is_empty(),
    );

    // Derive the credential cipher + read DeepSeek config once at startup so the
    // per-request hot path doesn't re-run the KDF or hit the environment.
    let runtime_cipher = routes::runtime::derive_runtime_cipher(&cli.runtime_credentials_key);
    let deepseek_api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    let deepseek_base_url = std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string())
        .trim()
        .trim_end_matches('/')
        .to_string();

    let setup_caches = said_control_plane::new_setup_caches();

    let state = AppState {
        db: db.clone(),
        started_at: Arc::new(Instant::now()),
        lark,
        hub,
        notifications,
        openai_api_key: cli.openai_api_key,
        groq_api_key,
        deepinfra_api_key: cli.deepinfra_api_key,
        diagnostics_rate_limit: routes::diagnostics::DiagnosticsRateLimiter::default(),
        divo_base_url: cli.divo_base_url,
        runtime_credentials_key: cli.runtime_credentials_key,
        runtime_cipher,
        deepseek_api_key,
        deepseek_base_url,
        platform_admin_org_slug: cli.platform_admin_org_slug,
        tenant_cache: setup_caches.tenant_cache,
        runtime_memory_cache: setup_caches.runtime_memory_cache,
        profile_cache: setup_caches.profile_cache,
        app_bucket_cache: setup_caches.app_bucket_cache,
        bucket_profile_cache: setup_caches.bucket_profile_cache,
        prompt_profile_context_cache: setup_caches.prompt_profile_context_cache,
        runtime_credential_cache: setup_caches.runtime_credential_cache,
    };

    // Per-user batched profiling + KB worker (deepseek-v4-flash, one run per ~10 dictations).
    said_control_plane::profile::updater::batch_run::start_batch_worker(state.clone());

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
        info!("[cp] shutting down");
    };

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .expect("failed to bind");

    info!("[cp] listening on 0.0.0.0:{}", cli.port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server failed");
}

fn load_dotenv() {
    if dotenvy::dotenv().is_ok() {
        return;
    }
    // `just dev-admin` sources repo-root `.env`, but `cargo run` from this crate does not.
    let root_env = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join(".env"));
    if let Some(path) = root_env {
        let _ = dotenvy::from_path(path);
    }
}

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
