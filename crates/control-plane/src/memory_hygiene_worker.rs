//! Lazy batch memory hygiene worker — processes dirty accounts after quiet period.

use std::time::Duration;

use sqlx::PgPool;
use tracing::{info, warn};

use crate::memory_hygiene;

const TICK_SECS: u64 = 5 * 60;
const BATCH_LIMIT: i64 = 10;

pub fn start_memory_hygiene_worker(db: PgPool) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        let mut tick_count: u64 = 0;
        loop {
            interval.tick().await;
            tick_count += 1;
            let nightly = tick_count % 288 == 0; // ~24h at 5m ticks

            let lock: Result<bool, sqlx::Error> = sqlx::query_scalar(
                "SELECT pg_try_advisory_lock(hashtext('airnote_memory_hygiene'))",
            )
            .fetch_one(&db)
            .await;

            match lock {
                Ok(true) => {
                    if let Err(e) = run_hygiene_tick(&db, nightly).await {
                        warn!("[memory-hygiene-worker] tick failed: {e}");
                    }
                    let _ = sqlx::query(
                        "SELECT pg_advisory_unlock(hashtext('airnote_memory_hygiene'))",
                    )
                    .execute(&db)
                    .await;
                }
                Ok(false) => info!("[memory-hygiene-worker] skipped; lock held elsewhere"),
                Err(e) => warn!("[memory-hygiene-worker] lock failed: {e}"),
            }
        }
    });
}

async fn run_hygiene_tick(db: &PgPool, nightly_sweep: bool) -> Result<(), String> {
    let mut accounts = memory_hygiene::fetch_dirty_accounts(db, BATCH_LIMIT)
        .await
        .map_err(|e| e.to_string())?;

    if nightly_sweep && accounts.len() < BATCH_LIMIT as usize {
        let extra: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT account_id
               FROM personal_memory_hygiene_state
              WHERE memory_dirty_at IS NOT NULL
              ORDER BY memory_dirty_at ASC
              LIMIT $1",
        )
        .bind(BATCH_LIMIT - accounts.len() as i64)
        .fetch_all(db)
        .await
        .map_err(|e| e.to_string())?;
        for id in extra {
            if !accounts.contains(&id) {
                accounts.push(id);
            }
        }
    }

    if accounts.is_empty() {
        return Ok(());
    }

    info!(
        "[memory-hygiene-worker] processing {} dirty account(s)",
        accounts.len()
    );

    for account_id in accounts {
        match memory_hygiene::process_account_hygiene(db, account_id).await {
            Ok(n) => info!("[memory-hygiene-worker] account={account_id} applied={n}"),
            Err(e) => warn!("[memory-hygiene-worker] account={account_id} failed: {e}"),
        }
    }
    Ok(())
}
