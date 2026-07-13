//! Org-level usage metering.
//!
//! Runtime polish is intentionally unlimited. Usage is still recorded for
//! operational visibility, but it must never block a request.

use chrono::NaiveDate;
use uuid::Uuid;

use crate::AppState;

pub async fn org_tier(state: &AppState, org_id: Uuid) -> Result<String, sqlx::Error> {
    let tier: Option<String> =
        sqlx::query_scalar("SELECT tier FROM org_subscriptions WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(&state.db)
            .await?;

    Ok(tier.unwrap_or_else(|| "team".into()))
}

pub async fn org_polish_count_today(state: &AppState, org_id: Uuid) -> Result<i32, sqlx::Error> {
    let count: Option<i32> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(polish_count), 0)::int
           FROM org_usage_daily
          WHERE org_id = $1 AND event_date = CURRENT_DATE",
    )
    .bind(org_id)
    .fetch_optional(&state.db)
    .await?;

    Ok(count.unwrap_or(0))
}

pub async fn record_org_usage(
    state: &AppState,
    org_id: Uuid,
    account_id: Uuid,
    date: NaiveDate,
    polish_count: i32,
    word_count: i32,
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO org_usage_daily
            (org_id, account_id, event_date, polish_count, word_count, model_used)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (org_id, account_id, event_date, model_used) DO UPDATE
           SET polish_count = org_usage_daily.polish_count + EXCLUDED.polish_count,
               word_count   = org_usage_daily.word_count   + EXCLUDED.word_count",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(date)
    .bind(polish_count)
    .bind(word_count)
    .bind(model)
    .execute(&state.db)
    .await?;
    Ok(())
}
