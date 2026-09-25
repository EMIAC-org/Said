//! Endpoints of the removed server-side learning system.
//!
//! The server no longer learns anything: no profile, no personal vocabulary,
//! no aliases, no company vocabulary. Desktops already in users' hands (2.4.5,
//! 2.5.0) still call these from background uploaders and outboxes that retry
//! on failure, so each one answers success with an empty body in the shape
//! those builds parse, and does nothing else. No auth, no database, no body
//! parsing: a stub must never be the reason an old client keeps retrying.

use axum::Json;
use serde_json::{Value, json};

/// `POST /v1/runtime/client-events` — kept so older desktops stop retrying.
pub async fn client_event() -> Json<Value> {
    Json(json!({ "stored": false, "notified": false }))
}

/// `POST /v1/runtime/learning/meaning` — kept so older desktops stop retrying.
pub async fn vocabulary_meaning() -> Json<Value> {
    Json(json!({ "meaning": "", "provider": "", "model": "" }))
}

/// `POST /v1/runtime/memory/sync` — kept so older desktops stop retrying.
pub async fn memory_sync() -> Json<Value> {
    Json(json!({
        "accepted_vocab": 0,
        "accepted_aliases": 0,
        "accepted_policies": 0,
        "accepted_emails": 0,
        "blocked_vocab": 0,
        "blocked_aliases": 0,
        "skipped": 0,
    }))
}

/// `POST /v1/runtime/memory/dirty` — kept so older desktops stop retrying.
pub async fn memory_dirty() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// `POST /v1/runtime/observability/aliases` — kept so older desktops stop retrying.
pub async fn alias_events() -> Json<Value> {
    Json(json!({ "ok": true }))
}

/// `GET /v1/runtime/profile/insights` — kept so older desktops stop retrying.
pub async fn profile_insights() -> Json<Value> {
    Json(json!({
        "run_stats": {
            "run_count": 0,
            "skipped_count": 0,
            "last_run_at": null,
            "last_run_outcome": null,
        },
        "knowledge": { "background": null, "domains": [], "focus_areas": [] },
        "buckets": [],
    }))
}

/// `GET /v1/runtime/profile/buckets` — kept so older desktops stop retrying.
pub async fn app_buckets() -> Json<Value> {
    Json(json!({ "buckets": [], "apps": [] }))
}

/// `POST /v1/runtime/profile/buckets/override` — kept so older desktops stop retrying.
pub async fn set_app_bucket() -> Json<Value> {
    Json(json!({ "ok": true }))
}

/// `GET /v1/company-vocab/version` — kept so older desktops stop retrying.
/// Version 0 and `changed: false` mean "nothing to download".
pub async fn company_vocab_version() -> Json<Value> {
    Json(json!({ "version": 0, "bucket_hash": null, "changed": false }))
}

/// `GET /v1/company-vocab/bucket` — kept so older desktops stop retrying.
pub async fn company_vocab_bucket() -> Json<Value> {
    Json(json!({
        "org_id": "",
        "version": 0,
        "bucket_hash": null,
        "manifest": { "schema_version": 1, "terms": [], "aliases": [] },
    }))
}

/// `POST /v1/company-vocab/user-vocab` — kept so older desktops stop retrying.
pub async fn company_vocab_upload() -> Json<Value> {
    Json(json!({ "ok": true, "terms": 0, "aliases": 0 }))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use axum::Json;
    use serde_json::{Value, json};

    use crate::{AppState, LarkConfig, build_router, meeting_hub, notification_hub, routes};

    /// Real router, a database that is never reachable: every stub must answer
    /// without auth and without touching Postgres.
    fn state_without_database() -> AppState {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://stub@127.0.0.1:1/stub")
            .expect("lazy pool");
        let caches = crate::new_setup_caches();
        AppState {
            db: db.clone(),
            started_at: Arc::new(Instant::now()),
            lark: LarkConfig {
                app_id: String::new(),
                app_secret: String::new(),
                redirect_uri: String::new(),
                jwt_secret: "test".into(),
            },
            hub: meeting_hub::MeetingHub::new(db),
            notifications: notification_hub::NotificationHub::new(),
            openai_api_key: String::new(),
            groq_api_key: String::new(),
            deepinfra_api_key: String::new(),
            hf_token: String::new(),
            model_url_cache: routes::models::new_cache(),
            diagnostics_rate_limit: routes::diagnostics::DiagnosticsRateLimiter::default(),
            runtime_credentials_key: String::new(),
            runtime_cipher: None,
            platform_admin_org_slug: String::new(),
            tenant_cache: caches.tenant_cache,
            runtime_credential_cache: caches.runtime_credential_cache,
        }
    }

    #[tokio::test]
    async fn every_call_an_old_desktop_makes_gets_a_parseable_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(axum::serve(listener, build_router(state_without_database())).into_future());
        let http = reqwest::Client::new();

        // (method, path, a field the 2.4.5 / 2.5.0 client reads)
        let calls = [
            ("POST", "/v1/runtime/client-events", "stored"),
            ("POST", "/v1/runtime/learning/meaning", "meaning"),
            ("POST", "/v1/runtime/memory/sync", "accepted_vocab"),
            ("POST", "/v1/runtime/memory/dirty", "status"),
            ("POST", "/v1/runtime/observability/aliases", "ok"),
            ("GET", "/v1/runtime/profile/insights", "run_stats"),
            ("GET", "/v1/runtime/profile/buckets", "apps"),
            ("POST", "/v1/runtime/profile/buckets/override", "ok"),
            (
                "GET",
                "/v1/company-vocab/version?current_version=3",
                "changed",
            ),
            ("GET", "/v1/company-vocab/bucket?version=3", "manifest"),
            ("POST", "/v1/company-vocab/user-vocab", "ok"),
        ];
        for (method, path, field) in calls {
            let url = format!("{base}{path}");
            let request = match method {
                "GET" => http.get(url),
                _ => http
                    .post(url)
                    .json(&json!({ "items": [{ "heard": "a", "correct": "b" }] })),
            };
            let response = request.bearer_auth("expired-token").send().await.unwrap();
            assert!(
                response.status().is_success(),
                "{method} {path} returned {}",
                response.status()
            );
            let body: Value = response.json().await.unwrap();
            assert!(body.get(field).is_some(), "{path} lacks `{field}`: {body}");
        }
    }

    #[tokio::test]
    async fn old_desktops_are_told_there_is_no_company_vocabulary_to_download() {
        let Json(version) = super::company_vocab_version().await;
        assert_eq!(version["version"], 0);
        assert_eq!(version["changed"], false);

        // 2.5.0 deserializes the bucket into a struct with a required `org_id`
        // string and `manifest.terms` / `manifest.aliases` lists.
        let Json(bucket) = super::company_vocab_bucket().await;
        assert!(bucket["org_id"].is_string());
        assert!(bucket["manifest"]["terms"].as_array().unwrap().is_empty());
        assert!(bucket["manifest"]["aliases"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn profile_insights_carry_the_fields_old_desktops_require() {
        let Json(body) = super::profile_insights().await;
        assert_eq!(body["run_stats"]["run_count"], 0);
        assert_eq!(body["run_stats"]["skipped_count"], 0);
        assert!(body["knowledge"]["domains"].as_array().unwrap().is_empty());
        assert!(
            body["knowledge"]["focus_areas"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(body["buckets"].as_array().unwrap().is_empty());
    }
}
