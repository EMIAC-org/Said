//! Said Enterprise — Control Plane library.
//!
//! Re-exports the building blocks so integration tests (and the binary)
//! can construct and test the full Axum application.

pub mod ai_worker;
pub mod auth;
pub mod codex_client;
pub mod lark_client;
pub mod lark_sync;
pub mod meeting_hub;
pub mod notification_worker;
pub mod routes;
pub mod store;

use std::sync::Arc;
use std::time::Instant;

use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, header},
    response::{Html, IntoResponse, Redirect},
    routing::{delete, get, post},
};
use tower_http::cors::{Any, CorsLayer};

// ── Lark config ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct LarkConfig {
    pub app_id: String,
    pub app_secret: String,
    pub redirect_uri: String,
    pub jwt_secret: String,
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub db: store::Db,
    pub started_at: Arc<Instant>,
    pub lark: LarkConfig,
    pub hub: Arc<meeting_hub::MeetingHub>,
}

// ── Router constructor ───────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    Router::new()
        // Previews (standalone HTML, outside /admin SPA)
        .route("/preview/floors", get(preview_floors))
        // Public
        .route("/v1/health", get(routes::health::handler))
        .route("/v1/auth/signup", post(routes::auth::signup))
        .route("/v1/auth/login", post(routes::auth::login))
        // Authenticated
        .route("/v1/auth/logout", post(routes::auth::logout))
        .route("/v1/auth/me", get(routes::auth::me))
        // Enterprise — Lark OAuth
        .route("/v1/auth/lark/start", get(routes::lark_auth::start))
        .route("/v1/auth/lark/callback", get(routes::lark_auth::callback))
        .route("/v1/auth/lark/refresh", post(routes::lark_auth::refresh))
        .route("/v1/license/check", get(routes::license::check))
        .route("/v1/metering/report", post(routes::metering::report))
        // Enterprise — Orgs
        .route("/v1/orgs", post(routes::orgs::create))
        .route("/v1/orgs/me", get(routes::orgs::me))
        .route("/v1/orgs/:org_id/members", get(routes::orgs::members))
        // Enterprise — Meetings
        .route(
            "/v1/meetings",
            post(routes::meetings::create).get(routes::meetings::list),
        )
        .route("/v1/meetings/:id", get(routes::meetings::detail))
        .route("/v1/meetings/:id/start", post(routes::meetings::start))
        .route("/v1/meetings/:id/end", post(routes::meetings::end))
        .route(
            "/v1/meetings/:id/push-tasks",
            post(routes::meetings::push_tasks),
        )
        // Enterprise — Lark sync
        .route(
            "/v1/meetings/:id/sync-to-lark",
            post(routes::lark_sync::sync_to_lark),
        )
        // Enterprise — WebSocket
        .route("/v1/meetings/:id/ws", get(routes::ws::handler))
        // Enterprise — OpenAI account connection
        .route("/v1/openai/connect", post(routes::openai::connect))
        .route("/v1/openai/complete", post(routes::openai::complete))
        .route("/v1/openai/status", get(routes::openai::status))
        .route("/v1/openai/disconnect", delete(routes::openai::disconnect))
        // Public OAuth redirect (browser flow — desktop app opens this URL)
        .route("/auth/lark", get(lark_auth_redirect))
        // Admin dashboard (React SPA) — static assets first, then catch-all
        .route("/admin/assets/app.css", get(admin_css))
        .route("/admin/assets/app.js", get(admin_js))
        .route("/admin/simulator", get(admin_simulator))
        .route("/admin", get(admin_index))
        .route("/admin/", get(admin_index))
        .route("/admin/*path", get(admin_index))
        .layer(cors)
        .with_state(state)
}

// ── Embedded admin dashboard ─────────────────────────────────────────────────

const ADMIN_HTML: &str = include_str!("../admin-ui/dist/index.html");
const ADMIN_CSS: &str = include_str!("../admin-ui/dist/assets/app.css");
const ADMIN_JS: &str = include_str!("../admin-ui/dist/assets/app.js");
const SIMULATOR_HTML: &str = include_str!("../admin/simulator.html");
const PREVIEW_FLOORS: &str = include_str!("../admin-ui/public/preview-floors.html");

async fn admin_index() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

async fn admin_css() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/css; charset=utf-8")],
        ADMIN_CSS,
    )
}

async fn admin_js() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/javascript; charset=utf-8")],
        ADMIN_JS,
    )
}

async fn admin_simulator() -> Html<&'static str> {
    Html(SIMULATOR_HTML)
}

async fn preview_floors() -> Html<&'static str> {
    Html(PREVIEW_FLOORS)
}

async fn lark_auth_redirect(State(state): State<AppState>) -> Redirect {
    let oauth_state = uuid::Uuid::new_v4().to_string();
    let url = crate::lark_client::build_oauth_url(
        &state.lark.app_id,
        &state.lark.redirect_uri,
        &oauth_state,
    );
    Redirect::temporary(&url)
}
