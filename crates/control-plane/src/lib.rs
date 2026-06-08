//! AirNote Enterprise — Control Plane library.
//!
//! Re-exports the building blocks so integration tests (and the binary)
//! can construct and test the full Axum application.

pub mod ai_worker;
pub mod auth;
pub mod codex_client;
pub mod format_recover;
pub mod lark_client;
pub mod lark_sync;
pub mod meeting_hub;
pub mod notification_hub;
pub mod notification_worker;
pub mod number_format;
pub mod routes;
pub mod store;
pub mod vocab_worker;

use std::sync::Arc;
use std::time::Instant;

use axum::{
    Router,
    extract::Path,
    http::{Method, StatusCode, Uri, header},
    response::{Html, IntoResponse, Redirect},
    routing::{delete, get, patch, post},
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
    pub notifications: Arc<notification_hub::NotificationHub>,
    pub deepgram_api_key: String,
    pub groq_api_key: String,
    pub diagnostics_rate_limit: routes::diagnostics::DiagnosticsRateLimiter,
    /// Base URL of the Divo agent backend (e.g. https://divo.outreachdeal.com).
    pub divo_base_url: String,
    /// Secret used to encrypt BYOK provider credentials before storing them.
    pub runtime_credentials_key: String,
}

// ── Router constructor ───────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    Router::new()
        // Previews (standalone HTML, outside /admin SPA)
        .route("/preview/floors", get(preview_floors))
        .route("/report-bug", get(report_bug_page))
        // Public
        .route("/v1/health", get(routes::health::handler))
        .route("/v1/auth/signup", post(routes::auth::signup))
        .route("/v1/auth/login", post(routes::auth::login))
        .route("/v1/auth/desktop-email", post(routes::auth::desktop_email))
        .route("/v1/bug-reports/public", post(routes::bugs::submit_public))
        .route("/v1/diagnostics", post(routes::diagnostics::ingest))
        .route("/v1/diagnostics", get(routes::diagnostics::list))
        // Authenticated
        .route("/v1/auth/logout", post(routes::auth::logout))
        .route("/v1/auth/me", get(routes::auth::me))
        .route(
            "/v1/bug-reports/session",
            post(routes::bugs::create_session),
        )
        .route("/v1/bug-reports", get(routes::bugs::list))
        .route(
            "/v1/bug-reports/:id/status",
            patch(routes::bugs::update_status),
        )
        // Enterprise — Lark OAuth
        .route("/v1/auth/lark/start", get(routes::lark_auth::start))
        .route("/v1/auth/lark/callback", get(routes::lark_auth::callback))
        .route("/v1/auth/lark/refresh", post(routes::lark_auth::refresh))
        // Divo agent proxy (attaches the account's Lark token, streams SSE back)
        .route("/v1/divo/chat", post(routes::divo::chat))
        .route("/v1/divo/threads", get(routes::divo::list_threads))
        .route("/v1/divo/threads/:id", get(routes::divo::thread))
        .route(
            "/v1/runtime/voice/polish",
            post(routes::runtime::voice_polish),
        )
        .route("/v1/runtime/voice/wav", post(routes::runtime::voice_wav))
        .route("/v1/runtime/status", get(routes::runtime::status))
        .route("/v1/runtime/runs", get(routes::runtime::list_runs))
        .route("/v1/runtime/runs/:id", get(routes::runtime::run_detail))
        .route(
            "/v1/runtime/learning-events",
            get(routes::runtime::list_learning_events),
        )
        .route(
            "/v1/runtime/client-events",
            post(routes::runtime::client_event),
        )
        .route(
            "/v1/runtime/learning/analyze-edit",
            post(routes::runtime::analyze_edit_learning),
        )
        .route(
            "/v1/runtime/learning/confirm-batch",
            post(routes::runtime::confirm_learning_batch),
        )
        .route(
            "/v1/runtime/notifications/ws",
            get(routes::runtime::notifications_ws),
        )
        .route(
            "/v1/runtime/credentials",
            get(routes::runtime::list_credentials).post(routes::runtime::save_credential),
        )
        .route(
            "/v1/runtime/credentials/:id/validate",
            post(routes::runtime::validate_credential),
        )
        .route(
            "/v1/runtime/credentials/:id",
            delete(routes::runtime::revoke_credential),
        )
        .route(
            "/v1/runtime/voice/dry-run",
            post(routes::runtime::voice_dry_run),
        )
        .route("/v1/runtime/voice/ws", get(routes::runtime::voice_ws))
        // History
        .route(
            "/v1/runtime/history",
            get(routes::runtime_history::list_history),
        )
        .route(
            "/v1/runtime/history/sync",
            post(routes::runtime_history::sync_history),
        )
        .route(
            "/v1/runtime/history/:id",
            get(routes::runtime_history::get_history_item)
                .patch(routes::runtime_history::patch_history_item)
                .delete(routes::runtime_history::delete_history_item),
        )
        .route(
            "/v1/runtime/memory/sync",
            post(routes::runtime_history::sync_memory),
        )
        .route(
            "/v1/runtime/settings",
            get(routes::runtime_settings::get_settings)
                .patch(routes::runtime_settings::patch_settings),
        )
        .route(
            "/v1/runtime/settings/sync",
            post(routes::runtime_settings::sync_settings),
        )
        .route("/v1/license/check", get(routes::license::check))
        .route("/v1/metering/report", post(routes::metering::report))
        // Enterprise — Desktop clients
        .route("/v1/clients/register", post(routes::clients::register))
        .route("/v1/clients/heartbeat", post(routes::clients::heartbeat))
        .route(
            "/v1/orgs/:org_id/clients",
            get(routes::clients::list_org_clients),
        )
        .route(
            "/v1/orgs/:org_id/clients/:account_id/usage",
            get(routes::clients::client_usage),
        )
        .route("/v1/orgs/:org_id/stats", get(routes::clients::org_stats))
        // Enterprise — Company vocabulary bucket
        .route(
            "/v1/orgs/:org_id/vocab/terms",
            get(routes::vocab::list_terms).post(routes::vocab::create_term),
        )
        .route(
            "/v1/orgs/:org_id/vocab/terms/:term_id",
            patch(routes::vocab::update_term).delete(routes::vocab::delete_term),
        )
        .route(
            "/v1/orgs/:org_id/vocab/aliases",
            get(routes::vocab::list_aliases).post(routes::vocab::create_alias),
        )
        .route(
            "/v1/orgs/:org_id/vocab/aliases/:alias_id",
            patch(routes::vocab::update_alias).delete(routes::vocab::delete_alias),
        )
        .route(
            "/v1/orgs/:org_id/vocab/publish",
            post(routes::vocab::publish),
        )
        .route(
            "/v1/orgs/:org_id/vocab/releases",
            get(routes::vocab::releases),
        )
        .route(
            "/v1/orgs/:org_id/vocab/suggestions",
            get(routes::vocab::list_suggestions),
        )
        .route(
            "/v1/orgs/:org_id/vocab/suggestions/aggregate",
            post(routes::vocab::aggregate_now),
        )
        .route(
            "/v1/orgs/:org_id/vocab/suggestions/:suggestion_id",
            patch(routes::vocab::update_suggestion),
        )
        .route(
            "/v1/orgs/:org_id/clients/:account_id/vocab",
            get(routes::vocab::user_vocab_detail),
        )
        .route(
            "/v1/company-vocab/version",
            get(routes::vocab::desktop_version),
        )
        .route(
            "/v1/company-vocab/bucket",
            get(routes::vocab::desktop_bucket),
        )
        .route(
            "/v1/company-vocab/user-vocab",
            post(routes::vocab::upload_user_vocab),
        )
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
        .route(
            "/v1/meetings/:id/guest-link",
            post(routes::guest::create_guest_link),
        )
        .route("/v1/meetings/:id/start", post(routes::meetings::start))
        .route("/v1/meetings/:id/end", post(routes::meetings::end))
        .route(
            "/v1/meetings/:id/push-tasks",
            post(routes::meetings::push_tasks),
        )
        // Enterprise — Guest browser capture
        .route("/join/:token", get(routes::guest::guest_page))
        .route("/join/:token/auth", post(routes::guest::guest_auth))
        .route("/v1/meetings/:id/guest-ws", get(routes::guest_ws::handler))
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
        .route("/auth/lark", get(routes::lark_auth::desktop_start))
        // Admin dashboard (React SPA) — static assets first, then catch-all
        .route("/admin/assets/app.css", get(admin_css))
        .route("/admin/assets/app.js", get(admin_js))
        .route("/admin/simulator", get(admin_simulator))
        .route("/admin", get(admin_redirect))
        .route("/admin/", get(admin_index))
        .route("/admin/*path", get(admin_spa))
        .fallback(not_found_or_admin_typo)
        .layer(cors)
        .with_state(state)
}

// ── Embedded admin dashboard ─────────────────────────────────────────────────

const ADMIN_HTML: &str = include_str!("../admin-ui/dist/index.html");
const ADMIN_CSS: &str = include_str!("../admin-ui/dist/assets/app.css");
const ADMIN_JS: &str = include_str!("../admin-ui/dist/assets/app.js");
const SIMULATOR_HTML: &str = include_str!("../admin/simulator.html");
const PREVIEW_FLOORS: &str = include_str!("../admin-ui/public/preview-floors.html");
const REPORT_BUG_HTML: &str = include_str!("../public/report-bug.html");

async fn admin_index() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

async fn admin_redirect(uri: Uri) -> Redirect {
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    Redirect::temporary(&format!("/admin/{query}"))
}

async fn admin_spa(Path(path): Path<String>) -> axum::response::Response {
    if path.starts_with("assets/") {
        return not_found_page().into_response();
    }
    Html(ADMIN_HTML).into_response()
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

async fn report_bug_page() -> Html<&'static str> {
    Html(REPORT_BUG_HTML)
}

async fn not_found_or_admin_typo(uri: Uri) -> axum::response::Response {
    if uri.path().starts_with("/admin") {
        return not_found_page().into_response();
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn not_found_page() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        [("content-type", "text/html; charset=utf-8")],
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>AirNote Enterprise - Not Found</title>
  <style>
    body{margin:0;min-height:100vh;display:grid;place-items:center;background:#080b16;color:#e8eaf0;font-family:Inter,ui-sans-serif,system-ui,sans-serif}
    main{width:min(420px,calc(100vw - 32px));padding:28px;border:1px solid #1a2038;border-radius:18px;background:#0e1225}
    h1{margin:0 0 8px;font-size:20px}p{margin:0 0 20px;color:#8f96b5;font-size:13px;line-height:1.5}
    a{display:inline-flex;padding:10px 14px;border-radius:12px;background:#7591ef;color:white;text-decoration:none;font-size:13px;font-weight:700}
  </style>
</head>
<body><main><h1>Page not found</h1><p>The AirNote admin page you requested does not exist.</p><a href="/admin/">Open admin</a></main></body>
</html>"#,
    )
}
