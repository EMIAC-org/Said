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
            let lock: Result<bool, sqlx::Error> = sqlx::query_scalar(
                "SELECT pg_try_advisory_lock(hashtext('airnote_vocab_aggregation'))",
            )
            .fetch_one(&db)
            .await;
            match lock {
                Ok(true) => {
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
                        deepgram_api_key: String::new(),
                        stt_provider: "deepgram".to_string(),
                        groq_api_key: String::new(),
                        diagnostics_rate_limit:
                            routes::diagnostics::DiagnosticsRateLimiter::default(),
                        divo_base_url: String::new(),
                        runtime_credentials_key: String::new(),
                        runtime_cipher: None,
                        deepseek_api_key: String::new(),
                        deepseek_base_url: String::new(),
                        deepseek_message_polish_model: String::new(),
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
                    .execute(&db)
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
