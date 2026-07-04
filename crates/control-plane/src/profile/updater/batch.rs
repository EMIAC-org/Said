//! Batched per-user profiling + KB runs.
//!
//! One coalesced job per user per ~10 dictations, claimed concurrently by workers
//! (`FOR UPDATE SKIP LOCKED`), analyzed by DeepSeek-V4-Flash over a bucket-tagged
//! window. This module owns the job lifecycle (enqueue/claim/finish/reap), the
//! window collector, and the cheap pre-call signal gate. The DeepSeek call + split
//! apply (global vs per-bucket) layer on top of these primitives.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::profile::bucket::{self, Bucket};
use crate::profile::store;

/// Default dictations that must accumulate before a user's window is enqueued.
pub const RUNS_PER_BATCH: i64 = 10;

/// Threshold, overridable via `AIRNOTE_PROFILE_BATCH_RUNS` (handy for quick testing —
/// set it to 2-3 so a run fires without dictating ten times). Falls back to the default.
pub fn runs_per_batch() -> i64 {
    std::env::var("AIRNOTE_PROFILE_BATCH_RUNS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(RUNS_PER_BATCH)
}
/// Hard cap on a coalesced window (a fast user may cross many thresholds while queued).
pub const MAX_WINDOW_RUNS: i64 = 40;
/// Retry ceiling before a job is dead-lettered.
pub const MAX_ATTEMPTS: i32 = 3;
/// A `processing` job older than this is considered crashed and reclaimed.
pub const STUCK_AFTER_SECS: i64 = 300;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct BatchJobRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub status: String,
    pub attempts: i32,
    pub run_count: i32,
    pub created_at: DateTime<Utc>,
}

// -------------------------------------------------------------------------------------
// Trigger: count runs since the last run, enqueue a coalesced job at the threshold.
// -------------------------------------------------------------------------------------

/// Runs recorded for this account since `since` (exclusive). Account-scoped: history
/// carries `org_id` (nullable) while profiles use the `org_scope` sentinel, so we count
/// per account and let the window collector attribute buckets.
pub async fn runs_since(
    db: &PgPool,
    account_id: Uuid,
    since: DateTime<Utc>,
) -> Result<i64, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_history_items
          WHERE account_id = $1 AND deleted_at IS NULL AND created_at > $2",
    )
    .bind(account_id)
    .bind(since)
    .fetch_one(db)
    .await?;
    Ok(n)
}

/// The high-water mark after which new dictations count toward the next window:
/// the most recent job's `window_to`, else the profile's `last_run_at`, else epoch.
pub async fn last_window_mark(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<DateTime<Utc>, sqlx::Error> {
    let mark: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT max(window_to) FROM runtime_profile_batch_jobs
          WHERE account_id = $1 AND org_scope = $2 AND window_to IS NOT NULL",
    )
    .bind(account_id)
    .bind(org_scope)
    .fetch_one(db)
    .await?;
    if let Some(m) = mark {
        return Ok(m);
    }
    let last_run: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_run_at FROM runtime_user_profiles
          WHERE account_id = $1 AND org_scope = $2",
    )
    .bind(account_id)
    .bind(org_scope)
    .fetch_optional(db)
    .await?
    .flatten();
    Ok(last_run.unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch")))
}

/// Enqueue a coalesced window job if the user has crossed the threshold and has no
/// in-flight job. Returns the new job id, or `None` if not due / already queued.
pub async fn maybe_enqueue(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mark = last_window_mark(db, account_id, org_scope).await?;
    let count = runs_since(db, account_id, mark).await?;
    let threshold = runs_per_batch();
    if count < threshold {
        tracing::info!(
            "[profile-batch] account={account_id} {count}/{threshold} dictations since last run"
        );
        return Ok(None);
    }
    // INSERT only when no active job exists. The partial unique index makes the loser
    // of a concurrent race fail with a unique violation, which we treat as "already
    // queued" (Ok(None)).
    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO runtime_profile_batch_jobs (account_id, org_scope, status)
         SELECT $1, $2, 'queued'
          WHERE NOT EXISTS (
              SELECT 1 FROM runtime_profile_batch_jobs
               WHERE account_id = $1 AND org_scope = $2
                 AND status IN ('queued', 'processing')
          )
         RETURNING id",
    )
    .bind(account_id)
    .bind(org_scope)
    .fetch_optional(db)
    .await;

    match inserted {
        Ok(Some(id)) => {
            tracing::info!(
                "[profile-batch] account={account_id} QUEUED profiling run ({count} dictations)"
            );
            Ok(Some(id))
        }
        Ok(None) => Ok(None), // a run is already in flight — coalesced
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(None),
        Err(e) => Err(e),
    }
}

// -------------------------------------------------------------------------------------
// Claim + finish: concurrent, no global lock.
// -------------------------------------------------------------------------------------

/// Atomically claim up to `limit` queued jobs for this worker. Concurrent workers get
/// disjoint sets (`FOR UPDATE SKIP LOCKED`); no job is processed twice.
pub async fn claim_jobs(db: &PgPool, limit: i64) -> Result<Vec<BatchJobRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchJobRow>(
        "UPDATE runtime_profile_batch_jobs
            SET status = 'processing', attempts = attempts + 1, updated_at = now()
          WHERE id IN (
              SELECT id FROM runtime_profile_batch_jobs
               WHERE status = 'queued'
               ORDER BY created_at
                 FOR UPDATE SKIP LOCKED
               LIMIT $1
          )
      RETURNING id, account_id, org_scope, status, attempts, run_count, created_at",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Terminal outcome (applied|shadow|rejected|skipped|failed) + window metadata.
#[allow(clippy::too_many_arguments)]
pub async fn finish_job(
    db: &PgPool,
    job_id: Uuid,
    status: &str,
    skip_reason: Option<&str>,
    run_count: i32,
    window_from: Option<DateTime<Utc>>,
    window_to: Option<DateTime<Utc>>,
    latency_ms: Option<i64>,
    token_usage: Option<i64>,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE runtime_profile_batch_jobs
            SET status = $2, skip_reason = $3, run_count = $4,
                window_from = $5, window_to = $6, latency_ms = $7, token_usage = $8,
                error = $9, processed_at = now(), updated_at = now()
          WHERE id = $1",
    )
    .bind(job_id)
    .bind(status)
    .bind(skip_reason)
    .bind(run_count)
    .bind(window_from)
    .bind(window_to)
    .bind(latency_ms)
    .bind(token_usage)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

/// Reclaim jobs stuck in `processing` (worker crash). Under the attempt ceiling they
/// return to `queued`; at/over it they are dead-lettered as `failed`. Returns count.
pub async fn reap_stuck(db: &PgPool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE runtime_profile_batch_jobs
            SET status = CASE WHEN attempts >= $1 THEN 'failed' ELSE 'queued' END,
                error = CASE WHEN attempts >= $1 THEN 'stuck: exceeded max attempts' ELSE error END,
                processed_at = CASE WHEN attempts >= $1 THEN now() ELSE processed_at END,
                updated_at = now()
          WHERE status = 'processing'
            AND updated_at < now() - ($2::bigint * interval '1 second')",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STUCK_AFTER_SECS)
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

/// Update the per-user run rollup on `runtime_user_profiles`.
pub async fn bump_run_stats(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    outcome: &str,
    skipped: bool,
) -> Result<(), sqlx::Error> {
    store::ensure_profile_row(db, account_id, org_scope).await?;
    sqlx::query(
        "UPDATE runtime_user_profiles
            SET profile_run_count = profile_run_count + 1,
                skipped_run_count = skipped_run_count + CASE WHEN $3 THEN 1 ELSE 0 END,
                last_run_at = now(),
                last_run_outcome = $4,
                updated_at = now()
          WHERE account_id = $1 AND org_scope = $2",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(skipped)
    .bind(outcome)
    .execute(db)
    .await?;
    Ok(())
}

// -------------------------------------------------------------------------------------
// Window collector + signal gate.
// -------------------------------------------------------------------------------------

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WindowRun {
    pub raw_transcript: Option<String>,
    pub polished_output: Option<String>,
    pub final_text: Option<String>,
    pub target_app: Option<String>,
    pub edit_feedback_json: Value,
    pub created_at: DateTime<Utc>,
}

/// A window run with its resolved bucket and whether the user edited the output.
#[derive(Clone, Debug)]
pub struct BucketedRun {
    pub bucket: Bucket,
    pub was_edited: bool,
    pub run: WindowRun,
}

/// True when the user changed the polished output (final differs) or an edit-feedback
/// object was recorded — the direct correction signal.
pub fn run_was_edited(run: &WindowRun) -> bool {
    if let (Some(polished), Some(final_text)) = (&run.polished_output, &run.final_text)
        && polished != final_text
    {
        return true;
    }
    run.edit_feedback_json
        .as_object()
        .map(|o| !o.is_empty())
        .unwrap_or(false)
}

/// Collect the most recent runs (chronological) after `since`, capped at `MAX_WINDOW_RUNS`,
/// each tagged with its resolved bucket.
pub async fn collect_window(
    db: &PgPool,
    account_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<BucketedRun>, sqlx::Error> {
    let mut rows = sqlx::query_as::<_, WindowRun>(
        "SELECT raw_transcript, polished_output, final_text, target_app,
                edit_feedback_json, created_at
           FROM runtime_history_items
          WHERE account_id = $1 AND deleted_at IS NULL AND created_at > $2
          ORDER BY created_at DESC
          LIMIT $3",
    )
    .bind(account_id)
    .bind(since)
    .bind(MAX_WINDOW_RUNS)
    .fetch_all(db)
    .await?;
    rows.reverse(); // chronological

    let mut out = Vec::with_capacity(rows.len());
    for run in rows {
        let bucket = match &run.target_app {
            Some(app) => bucket::resolve_bucket(db, app).await,
            None => Bucket::Default,
        };
        let was_edited = run_was_edited(&run);
        out.push(BucketedRun {
            bucket,
            was_edited,
            run,
        });
    }
    Ok(out)
}

/// Distinct buckets present in a window.
pub fn buckets_present(runs: &[BucketedRun]) -> Vec<Bucket> {
    let mut seen = Vec::new();
    for r in runs {
        if !seen.contains(&r.bucket) {
            seen.push(r.bucket);
        }
    }
    seen
}

/// Cheap pre-call signal check: any edit in the window is signal worth a DeepSeek run.
/// (The worker additionally treats a not-yet-established bucket as signal; that needs an
/// overlay lookup, so it lives at the call site.)
pub fn has_edit_signal(runs: &[BucketedRun]) -> bool {
    runs.iter().any(|r| r.was_edited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(polished: &str, final_text: &str, feedback: Value) -> WindowRun {
        WindowRun {
            raw_transcript: None,
            polished_output: Some(polished.to_string()),
            final_text: Some(final_text.to_string()),
            target_app: None,
            edit_feedback_json: feedback,
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    fn bucketed(bucket: Bucket, was_edited: bool) -> BucketedRun {
        BucketedRun {
            bucket,
            was_edited,
            run: run("a", "a", json!({})),
        }
    }

    #[test]
    fn edited_when_final_differs() {
        assert!(run_was_edited(&run("hello", "Hello team", json!({}))));
    }

    #[test]
    fn edited_when_feedback_present() {
        assert!(run_was_edited(&run(
            "x",
            "x",
            json!({"class": "polish_error"})
        )));
    }

    #[test]
    fn not_edited_when_accepted_as_is() {
        assert!(!run_was_edited(&run("same", "same", json!({}))));
    }

    #[test]
    fn signal_requires_an_edit() {
        let clean = vec![
            bucketed(Bucket::Coding, false),
            bucketed(Bucket::Messaging, false),
        ];
        assert!(!has_edit_signal(&clean));
        let mut dirty = clean.clone();
        dirty.push(bucketed(Bucket::Messaging, true));
        assert!(has_edit_signal(&dirty));
    }

    #[test]
    fn buckets_present_dedups_in_order() {
        let runs = vec![
            bucketed(Bucket::Messaging, false),
            bucketed(Bucket::Coding, true),
            bucketed(Bucket::Messaging, true),
        ];
        assert_eq!(
            buckets_present(&runs),
            vec![Bucket::Messaging, Bucket::Coding]
        );
    }
}
