//! Daily company-vocabulary suggestion aggregation.

use std::time::Duration;

use sqlx::PgPool;
use tracing::{info, warn};

use crate::{AppState, routes};

pub fn start_vocab_aggregation_worker(db: PgPool) {
    tokio::spawn(async move {
        // Do not compete with startup migrations and initial admin traffic.
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            let mut conn = match db.acquire().await {
                Ok(conn) => conn,
                Err(e) => {
                    warn!("[vocab-worker] lock connection failed: {e}");
                    continue;
                }
            };
            let lock: Result<bool, sqlx::Error> = sqlx::query_scalar(
                "SELECT pg_try_advisory_lock(hashtext('airnote_vocab_aggregation'))",
            )
            .fetch_one(&mut *conn)
            .await;
            match lock {
                Ok(true) => {
                    let setup_caches = crate::new_setup_caches();
                    let state = AppState {
                        db: db.clone(),
                        started_at: std::sync::Arc::new(std::time::Instant::now()),
                        lark: crate::LarkConfig {
                            app_id: String::new(),
                            app_secret: String::new(),
                            redirect_uri: String::new(),
                            jwt_secret: String::new(),
                        },
                        hub: crate::meeting_hub::MeetingHub::new(db.clone()),
                        notifications: crate::notification_hub::NotificationHub::new(),
                        openai_api_key: String::new(),
                        groq_api_key: String::new(),
                        openrouter_api_key: String::new(),
                        deepinfra_api_key: String::new(),
                        diagnostics_rate_limit:
                            routes::diagnostics::DiagnosticsRateLimiter::default(),
                        divo_base_url: String::new(),
                        runtime_credentials_key: String::new(),
                        runtime_cipher: None,
                        deepseek_api_key: String::new(),
                        deepseek_base_url: String::new(),
                        deepseek_message_polish_model: String::new(),
                        tenant_cache: setup_caches.tenant_cache,
                        runtime_memory_cache: setup_caches.runtime_memory_cache,
                        profile_cache: setup_caches.profile_cache,
                        app_bucket_cache: setup_caches.app_bucket_cache,
                        bucket_profile_cache: setup_caches.bucket_profile_cache,
                        prompt_profile_context_cache: setup_caches.prompt_profile_context_cache,
                        runtime_credential_cache: setup_caches.runtime_credential_cache,
                    };
                    match routes::vocab::aggregate_all_orgs(&state).await {
                        Ok((terms, aliases)) => {
                            info!(
                                "[vocab-worker] aggregation complete term_suggestions={} alias_suggestions={}",
                                terms, aliases
                            );
                        }
                        Err((status, body)) => {
                            warn!(
                                "[vocab-worker] aggregation failed status={status}: {:?}",
                                body
                            );
                        }
                    }
                    let _ = sqlx::query(
                        "SELECT pg_advisory_unlock(hashtext('airnote_vocab_aggregation'))",
                    )
                    .execute(&mut *conn)
                    .await;
                }
                Ok(false) => {
                    info!("[vocab-worker] aggregation skipped; another instance holds lock")
                }
                Err(e) => warn!("[vocab-worker] lock failed: {e}"),
            }
        }
    });
}
