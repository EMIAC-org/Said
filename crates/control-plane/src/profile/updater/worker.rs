//! Background worker for profile learn-from-edit jobs.

use std::time::Duration;

use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::profile::updater::jobs::process_learn_job;

const TICK_SECS: u64 = 2;
const ADVISORY_LOCK_KEY: &str = "airnote_profile_updater";

pub fn start_profile_updater_worker(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        loop {
            interval.tick().await;
            let mut conn = match state.db.acquire().await {
                Ok(conn) => conn,
                Err(e) => {
                    warn!("[profile-updater] lock connection failed: {e}");
                    continue;
                }
            };
            let lock: Result<bool, sqlx::Error> = sqlx::query_scalar(&format!(
                "SELECT pg_try_advisory_lock(hashtext('{ADVISORY_LOCK_KEY}'))"
            ))
            .fetch_one(&mut *conn)
            .await;

            match lock {
                Ok(true) => {
                    if let Err(e) = process_next_job(&state).await {
                        warn!("[profile-updater] tick error: {e}");
                    }
                    let _ = sqlx::query(&format!(
                        "SELECT pg_advisory_unlock(hashtext('{ADVISORY_LOCK_KEY}'))"
                    ))
                    .execute(&mut *conn)
                    .await;
                }
                Ok(false) => {}
                Err(e) => warn!("[profile-updater] lock failed: {e}"),
            }
        }
    });
    info!("[profile-updater] started ({TICK_SECS}s poll)");
}

async fn process_next_job(state: &AppState) -> Result<(), String> {
    let job_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM runtime_profile_learn_jobs
          WHERE status = 'queued'
          ORDER BY created_at ASC
          LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let Some(job_id) = job_id else {
        return Ok(());
    };

    process_learn_job(state, job_id).await
}
