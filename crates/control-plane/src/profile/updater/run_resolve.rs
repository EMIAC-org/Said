//! Resolve desktop `client_run_id` → `runtime_sessions.id` for profile learn jobs.

use sqlx::PgPool;
use uuid::Uuid;

use crate::profile::alias_safety::global_org_scope;

/// Lookup `runtime_sessions.id` for the same account and org scope.
/// Returns `Ok(None)` on miss — never fails the learn-from-edit path.
pub async fn resolve_run_id_for_learn(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    client_run_id: Option<&str>,
    explicit_run_id: Option<Uuid>,
) -> Result<(Option<String>, Option<Uuid>), sqlx::Error> {
    let client_run_id = client_run_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(run_id) = explicit_run_id {
        if run_id_owned(db, account_id, org_scope, run_id).await? {
            return Ok((client_run_id, Some(run_id)));
        }
        // Explicit run_id not owned — fall through to client_run_id lookup if present.
    }

    let Some(ref client_id) = client_run_id else {
        return Ok((None, None));
    };

    let resolved = lookup_run_id_by_client_run_id(db, account_id, org_scope, client_id).await?;
    Ok((Some(client_id.clone()), resolved))
}

async fn run_id_owned(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    run_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let owned: bool = if org_scope == global_org_scope() {
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM runtime_sessions
                 WHERE id = $1 AND account_id = $2
            )",
        )
        .bind(run_id)
        .bind(account_id)
        .fetch_one(db)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM runtime_sessions
                 WHERE id = $1 AND account_id = $2 AND org_id = $3
            )",
        )
        .bind(run_id)
        .bind(account_id)
        .bind(org_scope)
        .fetch_one(db)
        .await?
    };
    Ok(owned)
}

async fn lookup_run_id_by_client_run_id(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    client_run_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    if org_scope == global_org_scope() {
        sqlx::query_scalar(
            "SELECT id
               FROM runtime_sessions
              WHERE account_id = $1 AND client_run_id = $2
              ORDER BY created_at DESC
              LIMIT 1",
        )
        .bind(account_id)
        .bind(client_run_id)
        .fetch_optional(db)
        .await
    } else {
        sqlx::query_scalar(
            "SELECT id
               FROM runtime_sessions
              WHERE account_id = $1 AND client_run_id = $2 AND org_id = $3
              ORDER BY created_at DESC
              LIMIT 1",
        )
        .bind(account_id)
        .bind(client_run_id)
        .bind(org_scope)
        .fetch_optional(db)
        .await
    }
}

#[cfg(test)]
mod tests {
    fn normalize_client_run_id(raw: Option<&str>) -> Option<String> {
        raw.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    #[test]
    fn empty_client_run_id_is_none() {
        assert!(normalize_client_run_id(None).is_none());
        assert!(normalize_client_run_id(Some("")).is_none());
        assert!(normalize_client_run_id(Some("   ")).is_none());
    }

    #[test]
    fn trims_client_run_id() {
        assert_eq!(
            normalize_client_run_id(Some("  run-abc  ")).as_deref(),
            Some("run-abc")
        );
    }
}
