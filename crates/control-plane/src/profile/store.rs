//! Postgres helpers for `runtime_user_profiles`.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::alias::validate_aliases_in_json;
use super::alias_safety::global_org_scope;

pub const PROFILE_MARKDOWN_MAX_BYTES: usize = 2048;
pub const PROFILE_JSON_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ProfileRow {
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub profile_json: Value,
    pub profile_markdown: String,
    pub version: i64,
    pub schema_version: i32,
    pub status: String,
    pub source_hash: String,
    pub dirty_at: Option<DateTime<Utc>>,
    pub last_rebuilt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn resolve_org_scope(active_org_id: Option<Uuid>) -> Uuid {
    active_org_id.unwrap_or_else(global_org_scope)
}

pub async fn get_profile(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<Option<ProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, ProfileRow>(
        "SELECT account_id, org_scope, profile_json, profile_markdown, version, schema_version,
                status, source_hash, dirty_at, last_rebuilt_at, last_error, created_at, updated_at
           FROM runtime_user_profiles
          WHERE account_id = $1 AND org_scope = $2",
    )
    .bind(account_id)
    .bind(org_scope)
    .fetch_optional(db)
    .await
}

pub async fn ensure_profile_row(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO runtime_user_profiles (account_id, org_scope)
         VALUES ($1, $2)
         ON CONFLICT (account_id, org_scope) DO NOTHING",
    )
    .bind(account_id)
    .bind(org_scope)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_profile_with_fallback(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<Option<ProfileRow>, sqlx::Error> {
    if let Some(row) = get_profile(db, account_id, org_scope).await? {
        return Ok(Some(row));
    }
    let sentinel = global_org_scope();
    if org_scope != sentinel {
        return get_profile(db, account_id, sentinel).await;
    }
    Ok(None)
}

pub struct ProfilePatch {
    pub profile_json: Option<Value>,
    pub profile_markdown: Option<String>,
    pub mark_dirty: bool,
    pub source: &'static str,
}

pub fn validate_profile_sizes(profile_json: &Value, profile_markdown: &str) -> Result<(), String> {
    let json_bytes = serde_json::to_vec(profile_json).map_err(|e| e.to_string())?;
    if json_bytes.len() > PROFILE_JSON_MAX_BYTES {
        return Err(format!(
            "profile_json exceeds {} bytes",
            PROFILE_JSON_MAX_BYTES
        ));
    }
    if profile_markdown.len() > PROFILE_MARKDOWN_MAX_BYTES {
        return Err(format!(
            "profile_markdown exceeds {} bytes",
            PROFILE_MARKDOWN_MAX_BYTES
        ));
    }
    let rejects = validate_aliases_in_json(profile_json);
    if !rejects.is_empty() {
        return Err(format!(
            "invalid alias at index {}: {}",
            rejects[0].0, rejects[0].1
        ));
    }
    Ok(())
}

pub async fn upsert_profile_patch(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    patch: ProfilePatch,
) -> Result<ProfileRow, sqlx::Error> {
    ensure_profile_row(db, account_id, org_scope).await?;
    let current = get_profile(db, account_id, org_scope)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound)?;

    let profile_json_updated = patch.profile_json.is_some();
    let profile_markdown_updated = patch.profile_markdown.is_some();
    let next_json = patch.profile_json.unwrap_or(current.profile_json);
    let next_markdown = patch.profile_markdown.unwrap_or(current.profile_markdown);

    let dirty_at_expr = if patch.mark_dirty {
        "now()"
    } else {
        "dirty_at"
    };
    let status_expr = if patch.mark_dirty {
        "'dirty'"
    } else {
        "status"
    };

    let row = sqlx::query_as::<_, ProfileRow>(
        &format!(
            "UPDATE runtime_user_profiles
                SET profile_json = $3,
                    profile_markdown = $4,
                    version = $5,
                    status = {status_expr},
                    dirty_at = {dirty_at_expr},
                    updated_at = now()
              WHERE account_id = $1 AND org_scope = $2
          RETURNING account_id, org_scope, profile_json, profile_markdown, version, schema_version,
                    status, source_hash, dirty_at, last_rebuilt_at, last_error, created_at, updated_at"
        ),
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(next_json)
    .bind(next_markdown)
    .bind(current.version + 1)
    .fetch_one(db)
    .await?;

    let from_version = current.version;
    let to_version = row.version;

    write_audit_log(
        db,
        account_id,
        org_scope,
        from_version,
        to_version,
        "patch",
        serde_json::json!({
            "mark_dirty": patch.mark_dirty,
            "profile_json_updated": profile_json_updated,
            "profile_markdown_updated": profile_markdown_updated,
        }),
        patch.source,
    )
    .await?;

    Ok(row)
}

pub async fn mark_profile_rebuilding(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<ProfileRow, sqlx::Error> {
    ensure_profile_row(db, account_id, org_scope).await?;
    let current = get_profile(db, account_id, org_scope)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound)?;
    let from_version = current.version;

    let row = sqlx::query_as::<_, ProfileRow>(
        "UPDATE runtime_user_profiles
            SET status = 'rebuilding',
                last_error = NULL,
                updated_at = now()
          WHERE account_id = $1 AND org_scope = $2
      RETURNING account_id, org_scope, profile_json, profile_markdown, version, schema_version,
                status, source_hash, dirty_at, last_rebuilt_at, last_error, created_at, updated_at",
    )
    .bind(account_id)
    .bind(org_scope)
    .fetch_one(db)
    .await?;

    write_audit_log(
        db,
        account_id,
        org_scope,
        from_version,
        row.version,
        "rebuild",
        serde_json::json!({"queued": true, "deepseek": false}),
        "api",
    )
    .await?;

    Ok(row)
}

async fn write_audit_log(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    from_version: i64,
    to_version: i64,
    action: &str,
    patch_json: Value,
    source: &str,
) -> Result<(), sqlx::Error> {
    write_profile_audit(
        db,
        account_id,
        org_scope,
        from_version,
        to_version,
        action,
        patch_json,
        source,
    )
    .await
}

pub async fn write_profile_audit(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    from_version: i64,
    to_version: i64,
    action: &str,
    patch_json: Value,
    source: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO runtime_profile_audit_log
            (account_id, org_scope, from_version, to_version, action, patch_json, source)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(from_version)
    .bind(to_version)
    .bind(action)
    .bind(patch_json)
    .bind(source)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn apply_learned_profile(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    from_version: i64,
    profile_json: Value,
    profile_markdown: String,
    review_required: bool,
    audit_patch_json: Value,
) -> Result<ProfileRow, sqlx::Error> {
    ensure_profile_row(db, account_id, org_scope).await?;

    let status = if review_required { "dirty" } else { "ready" };

    let row = sqlx::query_as::<_, ProfileRow>(
        "UPDATE runtime_user_profiles
            SET profile_json = $3,
                profile_markdown = $4,
                version = $5,
                status = $6,
                last_error = NULL,
                dirty_at = CASE WHEN $7 THEN now() ELSE dirty_at END,
                updated_at = now()
          WHERE account_id = $1 AND org_scope = $2
      RETURNING account_id, org_scope, profile_json, profile_markdown, version, schema_version,
                status, source_hash, dirty_at, last_rebuilt_at, last_error, created_at, updated_at",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(profile_json)
    .bind(profile_markdown)
    .bind(from_version + 1)
    .bind(status)
    .bind(review_required)
    .fetch_one(db)
    .await?;

    write_profile_audit(
        db,
        account_id,
        org_scope,
        from_version,
        row.version,
        "learn_applied",
        audit_patch_json,
        "validator",
    )
    .await?;

    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_oversized_profile_json() {
        let huge = json!({ "blob": "x".repeat(PROFILE_JSON_MAX_BYTES + 1) });
        assert!(validate_profile_sizes(&huge, "").is_err());
    }

    #[test]
    fn rejects_oversized_profile_markdown() {
        let md = "x".repeat(PROFILE_MARKDOWN_MAX_BYTES + 1);
        assert!(validate_profile_sizes(&json!({}), &md).is_err());
    }

    #[test]
    fn rejects_common_word_alias_in_json() {
        let profile = json!({
            "aliases": [{
                "source_phrase": "kaam",
                "canonical_phrase": "Kafka",
                "status": "candidate",
                "confidence": 0.9,
                "evidence_count": 1,
                "reason": "",
                "last_seen_at": null,
                "profile_version": 1
            }]
        });
        assert!(validate_profile_sizes(&profile, "").is_err());
    }

    #[test]
    fn accepts_valid_multi_word_alias_in_json() {
        let profile = json!({
            "aliases": [{
                "source_phrase": "n 10",
                "canonical_phrase": "n8n",
                "status": "candidate",
                "confidence": 0.9,
                "evidence_count": 2,
                "reason": "",
                "last_seen_at": null,
                "profile_version": 1
            }]
        });
        assert!(validate_profile_sizes(&profile, "Domains: automation").is_ok());
    }
}
