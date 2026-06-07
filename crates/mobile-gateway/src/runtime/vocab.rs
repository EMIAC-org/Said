//! Personal vocabulary: the snapshot served to the client and the term list
//! injected into the polish prompt. v1 is personal-scope only; company/org
//! promotion is a later phase.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Canonical terms (highest priority first) for the polish prompt.
pub async fn load_terms_for_prompt(db: &PgPool, account_id: Uuid) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT term FROM vocab_terms
          WHERE account_id = $1 AND archived_at IS NULL
          ORDER BY priority DESC, created_at ASC
          LIMIT 60",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    rows.into_iter().map(|(term,)| term).collect()
}

/// A content hash for the account's current vocab, used as an ETag so clients
/// only refetch when terms change.
pub async fn current_hash(db: &PgPool, account_id: Uuid) -> String {
    let (count, latest): (i64, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(updated_at) FROM vocab_terms
          WHERE account_id = $1 AND archived_at IS NULL",
    )
    .bind(account_id)
    .fetch_one(db)
    .await
    .unwrap_or((0, None));
    compute_hash(count, latest)
}

fn compute_hash(count: i64, latest: Option<DateTime<Utc>>) -> String {
    let ts = latest.map(|d| d.timestamp()).unwrap_or(0);
    format!("personal-{count}-{ts}")
}

/// Full snapshot payload: `(hash, terms_json)`.
pub async fn load_snapshot(db: &PgPool, account_id: Uuid) -> (String, Vec<Value>) {
    let rows: Vec<(String, Value, Option<String>, f32)> = sqlx::query_as(
        "SELECT term, spoken_aliases, term_type, priority FROM vocab_terms
          WHERE account_id = $1 AND archived_at IS NULL
          ORDER BY priority DESC, created_at ASC",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let terms = rows
        .into_iter()
        .map(|(term, aliases, term_type, priority)| {
            json!({
                "term": term,
                "spoken_aliases": aliases,
                "type": term_type.unwrap_or_else(|| "other".into()),
                "scope": "personal",
                "priority": priority,
            })
        })
        .collect();

    let hash = current_hash(db, account_id).await;
    (hash, terms)
}
