//! Personal server-side memory (Wave 1/5/6): the per-user vocabulary, learned
//! STT replacements, and blocked aliases that feed the prompt and the resolver,
//! plus the writers used by the learning path. Strictly per-account; never
//! cross-user.

use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

/// Canonical personal vocab terms (highest priority first) for the polish prompt.
pub async fn load_terms_for_prompt(db: &PgPool, account_id: Uuid) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT term FROM personal_vocab_terms
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

/// Learned `(spoken_lowercased, canonical)` replacements for the resolver,
/// longest spoken first so multi-word terms win over substrings.
pub async fn load_replacements(db: &PgPool, account_id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT spoken, canonical FROM personal_stt_replacements
          WHERE account_id = $1
          ORDER BY length(spoken) DESC",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap_or_default()
}

/// Blocked spoken forms (lowercased) the resolver must never apply.
pub async fn load_blocked(db: &PgPool, account_id: Uuid) -> HashSet<String> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT spoken FROM personal_blocked_aliases WHERE account_id = $1")
            .bind(account_id)
            .fetch_all(db)
            .await
            .unwrap_or_default();
    rows.into_iter().map(|(s,)| s).collect()
}

pub async fn record_replacement(
    db: &PgPool,
    account_id: Uuid,
    spoken: &str,
    canonical: &str,
    source: &str,
) {
    let _ = sqlx::query(
        "INSERT INTO personal_stt_replacements (account_id, spoken, canonical, source)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (account_id, spoken) DO UPDATE
           SET canonical = EXCLUDED.canonical,
               source = EXCLUDED.source,
               hit_count = personal_stt_replacements.hit_count + 1,
               updated_at = now()",
    )
    .bind(account_id)
    .bind(spoken)
    .bind(canonical)
    .bind(source)
    .execute(db)
    .await;
}

pub async fn record_blocked(db: &PgPool, account_id: Uuid, spoken: &str, reason: &str) {
    let _ = sqlx::query(
        "INSERT INTO personal_blocked_aliases (account_id, spoken, reason)
         VALUES ($1, $2, $3)
         ON CONFLICT (account_id, spoken) DO NOTHING",
    )
    .bind(account_id)
    .bind(spoken)
    .bind(reason)
    .execute(db)
    .await;
}

pub async fn record_term(db: &PgPool, account_id: Uuid, term: &str) {
    let _ = sqlx::query(
        "INSERT INTO personal_vocab_terms (account_id, term, source)
         VALUES ($1, $2, 'user')
         ON CONFLICT (account_id, term) DO UPDATE
           SET archived_at = NULL, updated_at = now()",
    )
    .bind(account_id)
    .bind(term)
    .execute(db)
    .await;
}

pub async fn record_learning_event(
    db: &PgPool,
    account_id: Uuid,
    run_id: Option<Uuid>,
    kind: &str,
) {
    let _ = sqlx::query("INSERT INTO learning_events (account_id, run_id, kind) VALUES ($1, $2, $3)")
        .bind(account_id)
        .bind(run_id)
        .bind(kind)
        .execute(db)
        .await;
}
