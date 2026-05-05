use axum::{
    Router,
    http::{Method, header},
    middleware,
    routing::{get, patch, post},
};
use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

pub mod auth;
pub mod embedder;
pub mod llm;
pub mod routes;
pub mod store;
pub mod stt;

#[cfg(test)]
mod learning_flow_tests;

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
    tracing::debug!("[prefs-cache] miss → refreshed from SQLite");
    Some(prefs)
}

/// Invalidate the cache after a successful preferences update.
pub async fn invalidate_prefs_cache(cache: &PrefsCache) {
    let mut guard = cache.write().await;
    *guard = None;
    tracing::debug!("[prefs-cache] invalidated");
}

// ── Lexicon hot-cache ──────────────────────────────────────────────────────────
//
// Caches corrections + stt_replacements together — both change at most once per
// session, but are read synchronously from SQLite on every voice/text request.
// TTL = 60 s; invalidated immediately on any write (classify / feedback routes).

const LEXICON_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct CachedLexicon {
    pub corrections: Vec<store::corrections::Correction>,
    pub stt_replacements: Vec<store::stt_replacements::SttReplacement>,
    pub cached_at: Instant,
}

pub type LexiconCache = Arc<RwLock<Option<CachedLexicon>>>;

/// Read corrections + stt_replacements from cache, or SQLite on miss.
/// On a miss, both reads run in parallel on blocking threads.
pub async fn get_lexicon_cached(
    cache: &LexiconCache,
    pool: &store::DbPool,
    user_id: &str,
) -> (
    Vec<store::corrections::Correction>,
    Vec<store::stt_replacements::SttReplacement>,
) {
    // Fast path
    {
        let guard = cache.read().await;
        if let Some(ref entry) = *guard {
            if entry.cached_at.elapsed() < LEXICON_CACHE_TTL {
                return (entry.corrections.clone(), entry.stt_replacements.clone());
            }
        }
    }
    // Slow path — both SQLite reads in parallel, off the async executor
    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let uid1 = user_id.to_string();
    let uid2 = user_id.to_string();
    let (c, s) = tokio::join!(
        tokio::task::spawn_blocking(move || store::corrections::load_all(&pool1, &uid1)),
        tokio::task::spawn_blocking(move || store::stt_replacements::load_all(&pool2, &uid2)),
    );
    let corrections = c.unwrap_or_default();
    let stt_replacements = s.unwrap_or_default();

    let mut guard = cache.write().await;
    *guard = Some(CachedLexicon {
        corrections: corrections.clone(),
        stt_replacements: stt_replacements.clone(),
        cached_at: Instant::now(),
    });
    tracing::debug!("[lexicon-cache] miss → refreshed from SQLite");
    (corrections, stt_replacements)
}

/// Invalidate after any write to corrections or stt_replacements tables.
pub async fn invalidate_lexicon_cache(cache: &LexiconCache) {
    let mut guard = cache.write().await;
    *guard = None;
    tracing::debug!("[lexicon-cache] invalidated");
}

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub pool: store::DbPool,
    pub shared_secret: Arc<String>,
    pub default_user_id: Arc<String>,
    /// Preferences hot-cache — avoids SQLite SELECT per request.
    pub prefs_cache: PrefsCache,
    /// Lexicon hot-cache — corrections + stt_replacements together.
    pub lexicon_cache: LexiconCache,
    /// Shared HTTP client — keeps TCP/TLS connections alive across all requests.
    pub http_client: Client,
}

// ── Router factory ────────────────────────────────────────────────────────────

pub fn router_with_state(state: AppState) -> Router {
    // Public routes (no auth)
    let public = Router::new().route("/v1/health", get(routes::health::handler));

    // Authenticated routes (require shared-secret bearer)
    let authenticated = Router::new()
        .route("/v1/pre-embed", post(routes::pre_embed::handler))
        .route("/v1/voice/polish", post(routes::voice::polish))
        .route(
            "/v1/voice/polish-transcript",
            post(routes::voice::polish_transcript),
        )
        .route("/v1/voice/repair", post(routes::voice::repair_transcript))
        .route("/v1/text/polish", post(routes::text::polish))
        .route("/v1/text/refine-last", post(routes::text::refine_last))
        .route("/v1/edit-feedback", post(routes::feedback::submit))
        .route("/v1/classify-edit", post(routes::classify::classify))
        .route("/v1/pending-edits", post(routes::pending_edits::create))
        .route("/v1/pending-edits", get(routes::pending_edits::list))
        .route(
            "/v1/pending-edits/:id/resolve",
            post(routes::pending_edits::resolve),
        )
        .route("/v1/vocabulary/terms", get(routes::vocabulary::list_terms))
        .route("/v1/vocabulary", get(routes::vocabulary::list))
        .route("/v1/vocabulary", post(routes::vocabulary::create))
        .route(
            "/v1/vocabulary/:term",
            axum::routing::delete(routes::vocabulary::delete),
        )
        .route(
            "/v1/vocabulary/:term/star",
            post(routes::vocabulary::toggle_star),
        )
        .route("/v1/history", get(routes::history::list))
        .route(
            "/v1/recordings/:id",
            axum::routing::delete(routes::history::delete),
        )
        .route(
            "/v1/recordings/:id/audio",
            get(routes::history::audio).post(routes::history::upload_audio),
        )
        .route("/v1/preferences", get(routes::prefs::get_prefs))
        .route("/v1/preferences", patch(routes::prefs::patch_prefs))
        .route("/v1/corrections", get(routes::prefs::get_corrections))
        .route("/v1/stt/bias", get(routes::stt::get_bias))
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

    public.merge(authenticated).layer(cors).with_state(state)
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

    let state = AppState {
        pool,
        shared_secret: Arc::new(secret),
        default_user_id: Arc::new(user_id),
        prefs_cache: Arc::new(RwLock::new(None)),
        lexicon_cache: Arc::new(RwLock::new(None)),
        http_client,
    };
    routes::vocabulary::spawn_prompt_artifact_repair(state.clone());

    router_with_state(state)
}
