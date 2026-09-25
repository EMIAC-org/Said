use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{Method, header},
    middleware,
    routing::{get, patch, post},
};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

pub mod auth;
pub mod cp_client;
pub mod llm;
pub mod observability;
pub mod routes;
pub mod store;
pub mod telemetry;
pub mod watchdog;

// Re-export the cross-platform path helpers from said-core so that code
// reading `said_backend::paths::*` keeps working without an extra import.
pub use said_core::paths;

// ── Preferences hot-cache (Gap 3) ─────────────────────────────────────────────
//
// Avoids a SQLite SELECT on every voice/text/feedback request.
// TTL = 30 s. Invalidated immediately on PATCH /v1/preferences.
// At personal scale one user has exactly one entry, so the HashMap is a formality.

const PREFS_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct CachedPrefs {
    pub prefs: store::prefs::Preferences,
    pub cached_at: Instant,
}

pub type PrefsCache = Arc<RwLock<Option<CachedPrefs>>>;

/// Read preferences, hitting the in-memory cache when fresh.
/// Falls back to SQLite on miss or TTL expiry.
pub async fn get_prefs_cached(
    cache: &PrefsCache,
    pool: &store::DbPool,
    user_id: &str,
) -> Option<store::prefs::Preferences> {
    // ── Fast path: cache hit ──────────────────────────────────────────────────
    {
        let guard = cache.read().await;
        if let Some(ref entry) = *guard {
            if entry.cached_at.elapsed() < PREFS_CACHE_TTL {
                return Some(entry.prefs.clone());
            }
        }
    }

    // ── Slow path: SQLite read + re-populate cache ────────────────────────────
    let prefs = store::prefs::get_prefs(pool, user_id)?;
    let mut guard = cache.write().await;
    *guard = Some(CachedPrefs {
        prefs: prefs.clone(),
        cached_at: Instant::now(),
    });
    tracing::trace!("[prefs-cache] miss → refreshed from SQLite");
    Some(prefs)
}

/// Invalidate the cache after a successful preferences update.
pub async fn invalidate_prefs_cache(cache: &PrefsCache) {
    let mut guard = cache.write().await;
    *guard = None;
    tracing::trace!("[prefs-cache] invalidated");
}

const LIVE_SERVER_RUNTIME_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct LiveServerRuntimeLatency {
    pub stt: i64,
    pub polish: i64,
    pub total: i64,
}

#[derive(Clone, Debug)]
pub struct LiveServerRuntimeResult {
    pub transcript: String,
    pub output: String,
    pub model_used: String,
    pub latency_ms: LiveServerRuntimeLatency,
    pub stored_at: Instant,
}

pub type LiveServerRuntimeCache = Arc<RwLock<HashMap<String, LiveServerRuntimeResult>>>;

pub async fn put_live_server_runtime_result(
    cache: &LiveServerRuntimeCache,
    client_run_id: String,
    result: LiveServerRuntimeResult,
) {
    let mut guard = cache.write().await;
    guard.retain(|_, entry| entry.stored_at.elapsed() < LIVE_SERVER_RUNTIME_TTL);
    guard.insert(client_run_id, result);
}

pub async fn take_live_server_runtime_result(
    cache: &LiveServerRuntimeCache,
    client_run_id: &str,
) -> Option<LiveServerRuntimeResult> {
    let mut guard = cache.write().await;
    guard.retain(|_, entry| entry.stored_at.elapsed() < LIVE_SERVER_RUNTIME_TTL);
    let result = guard.remove(client_run_id)?;
    if result.stored_at.elapsed() >= LIVE_SERVER_RUNTIME_TTL {
        return None;
    }
    Some(result)
}

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub pool: store::DbPool,
    pub shared_secret: Arc<String>,
    pub default_user_id: Arc<String>,
    /// Preferences hot-cache — avoids SQLite SELECT per request.
    pub prefs_cache: PrefsCache,
    /// Short-lived cache of live server-runtime results keyed by recording/session id.
    pub live_server_runtime_cache: LiveServerRuntimeCache,
    /// Shared HTTP client — keeps TCP/TLS connections alive across all requests.
    pub http_client: Client,
    /// Watchdog health state — shared with the bare-thread watchdog.
    pub watchdog: Arc<watchdog::WatchdogState>,
}

pub fn router_with_state(state: AppState) -> Router {
    // Public routes (no auth)
    let public = Router::new()
        .route("/v1/health", get(routes::health::handler))
        .route("/v1/health/ping", get(routes::health::ping));

    // Authenticated routes (require shared-secret bearer)
    let authenticated = Router::new()
        .route("/v1/voice/polish", post(routes::voice::polish))
        .route(
            "/v1/problem/transcribe",
            post(routes::voice::problem_transcribe),
        )
        .route("/v1/problem/solve", post(routes::problem::solve))
        .route("/v1/runtime/live/config", get(routes::runtime_live::config))
        .route(
            "/v1/runtime/notifications/config",
            get(routes::runtime_live::notifications_config),
        )
        .route(
            "/v1/runtime/credentials/sync",
            post(routes::runtime_credentials::sync),
        )
        .route(
            "/v1/runtime/credentials/status",
            get(routes::runtime_credentials::status),
        )
        .route(
            "/v1/server-settings/status",
            get(routes::server_settings::status),
        )
        .route(
            "/v1/server-settings/sync",
            post(routes::server_settings::sync),
        )
        .route(
            "/v1/server-migration/status",
            get(routes::server_migration::status),
        )
        .route(
            "/v1/server-migration/run",
            post(routes::server_migration::run),
        )
        .route(
            "/v1/server-migration/cancel",
            post(routes::server_migration::cancel),
        )
        .route(
            "/v1/runtime/live/result",
            post(routes::runtime_live::cache_result),
        )
        .route("/v1/runtime/live/ws", get(routes::runtime_live::ws))
        .route(
            "/v1/voice/polish-transcript",
            post(routes::voice::polish_transcript),
        )
        .route("/v1/voice/repair", post(routes::voice::repair_transcript))
        .route("/v1/text/polish", post(routes::text::polish))
        .route("/v1/text/refine-last", post(routes::text::refine_last))
        .route(
            "/v1/dictionary",
            get(routes::dictionary::list)
                .post(routes::dictionary::add)
                .delete(routes::dictionary::delete_all),
        )
        .route(
            "/v1/dictionary/:id",
            axum::routing::delete(routes::dictionary::delete),
        )
        .route("/v1/history", get(routes::history::list))
        .route("/v1/history/apps", get(routes::history::app_usage))
        .route("/v1/history/sites", get(routes::history::site_usage))
        .route("/v1/site-context", post(routes::history::record_site))
        .route(
            "/v1/voice-runs/latest-failed",
            get(routes::voice_runs::latest_failed),
        )
        .route(
            "/v1/voice-runs/:run_id/failed",
            post(routes::voice_runs::mark_failed),
        )
        .route(
            "/v1/voice-runs/:run_id/paste",
            post(routes::voice_runs::mark_paste),
        )
        .route(
            "/v1/recordings/:id",
            axum::routing::delete(routes::history::delete),
        )
        .route(
            "/v1/recordings/:id/audio",
            get(routes::history::audio).post(routes::history::upload_audio),
        )
        .route(
            "/v1/recordings/:id/kept",
            axum::routing::put(routes::history::record_kept),
        )
        .route("/v1/preferences", get(routes::prefs::get_prefs))
        .route("/v1/preferences", patch(routes::prefs::patch_prefs))
        .route(
            "/v1/telemetry/runs/:run_id",
            patch(routes::telemetry::patch_run),
        )
        .route("/v1/telemetry/flush", post(routes::telemetry::flush))
        .route(
            "/v1/observability/dictation/:recording_id/trace",
            post(routes::telemetry::patch_dictation_trace),
        )
        .route(
            "/v1/observability/meetings/scan",
            post(routes::meeting_telemetry::scan),
        )
        // Cloud auth bridge — store/clear cloud token, query cloud status
        .route(
            "/v1/cloud/token",
            axum::routing::put(routes::cloud::store_token),
        )
        .route(
            "/v1/cloud/token",
            axum::routing::delete(routes::cloud::clear_token),
        )
        .route("/v1/cloud/status", get(routes::cloud::status))
        .route(
            "/v1/cloud/active-org",
            axum::routing::put(routes::cloud::set_active_org),
        )
        .route(
            "/v1/enterprise/status",
            get(routes::cloud::enterprise_status),
        )
        // OpenAI Codex OAuth
        .route(
            "/v1/openai-oauth/initiate",
            post(routes::openai_oauth::initiate),
        )
        .route("/v1/openai-oauth/status", get(routes::openai_oauth::status))
        .route(
            "/v1/openai-oauth/disconnect",
            axum::routing::delete(routes::openai_oauth::disconnect),
        )
        // Invite-a-friend email
        .route("/v1/invite", post(routes::invite::send))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_secret,
        ));

    // CORS — allow the Tauri webview origin and localhost dev server
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

    public
        .merge(authenticated)
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024)) // 32 MB — keeps long saved-audio retries inside AirNote-owned errors
        .layer(cors)
        .with_state(state)
}

/// Convenience builder used by main.rs — reads shared secret from env,
/// opens the DB, ensures the default user exists, and returns a ready Router.
pub fn router() -> Router {
    let secret = std::env::var("POLISH_SHARED_SECRET").unwrap_or_else(|_| "dev-secret".into());
    let db_path = store::default_db_path();
    let pool = store::open(&db_path);
    let user_id = store::ensure_default_user(&pool);

    let http_client = Client::builder()
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("failed to build shared HTTP client");

    let wd = Arc::new(watchdog::WatchdogState::new());

    let state = AppState {
        pool: pool.clone(),
        shared_secret: Arc::new(secret),
        default_user_id: Arc::new(user_id),
        prefs_cache: Arc::new(RwLock::new(None)),
        live_server_runtime_cache: Arc::new(RwLock::new(HashMap::new())),
        http_client,
        watchdog: wd.clone(),
    };

    watchdog::spawn_watchdog(pool, wd, tokio::runtime::Handle::current());

    router_with_state(state)
}
