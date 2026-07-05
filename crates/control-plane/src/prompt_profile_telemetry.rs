//! Persist the sanitized profile markdown used in voice polish for telemetry.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use said_core::polish::prompt::sanitize_profile_markdown;

pub const PROFILE_SOURCE_CLIENT_LOCAL: &str = "client_local";
pub const PROFILE_SOURCE_SERVER_DB: &str = "server_db";
pub const PROFILE_SOURCE_NONE: &str = "none";

pub struct PromptProfileSnapshot {
    pub profile_source: &'static str,
    pub profile_markdown: String,
    pub profile_chars: usize,
    pub profile_hash: String,
}

/// Build the same sanitized body that `render_profile_block` injects into the system prompt.
pub fn snapshot_from_raw(raw: Option<&str>) -> PromptProfileSnapshot {
    let sanitized = raw
        .map(sanitize_profile_markdown)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_default();
    let profile_chars = sanitized.chars().count();
    let profile_hash = hash_profile(&sanitized);
    let profile_source = if profile_chars > 0 {
        PROFILE_SOURCE_CLIENT_LOCAL
    } else {
        PROFILE_SOURCE_NONE
    };
    PromptProfileSnapshot {
        profile_source,
        profile_markdown: sanitized,
        profile_chars,
        profile_hash,
    }
}

/// Same as [`snapshot_from_raw`] but tags the source as the server-learned KB
/// (`server_db`) instead of the legacy client-shipped markdown. Used by the voice
/// polish path now that the profile is injected from the server profile store.
pub fn snapshot_from_server(raw: Option<&str>) -> PromptProfileSnapshot {
    let mut snap = snapshot_from_raw(raw);
    if snap.profile_chars > 0 {
        snap.profile_source = PROFILE_SOURCE_SERVER_DB;
    }
    snap
}

pub fn hash_profile(markdown: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn prompt_built_metadata(
    snapshot: &PromptProfileSnapshot,
    client_profile_version: Option<i64>,
    bucket_key: Option<&str>,
    bucket_source: Option<&str>,
) -> Value {
    json!({
        "prompt_version": said_core::polish::prompt::VOICE_PROMPT_BASE_VERSION,
        "profile_source": snapshot.profile_source,
        "profile_chars": snapshot.profile_chars,
        "profile_hash": snapshot.profile_hash,
        "profile_markdown": snapshot.profile_markdown,
        "client_profile_version": client_profile_version,
        // Which app-bucket resolved for this run + where the mapping came from
        // (user override / static / agent). Powers the "Context applied" section.
        "bucket_key": bucket_key,
        "bucket_source": bucket_source,
    })
}

pub async fn upsert_latest(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    run_id: Uuid,
    snapshot: &PromptProfileSnapshot,
    client_profile_version: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO runtime_prompt_profile_latest
            (account_id, org_scope, profile_source, profile_markdown, profile_chars,
             profile_hash, client_profile_version, last_run_id, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (account_id, org_scope) DO UPDATE SET
            profile_source = EXCLUDED.profile_source,
            profile_markdown = EXCLUDED.profile_markdown,
            profile_chars = EXCLUDED.profile_chars,
            profile_hash = EXCLUDED.profile_hash,
            client_profile_version = EXCLUDED.client_profile_version,
            last_run_id = EXCLUDED.last_run_id,
            updated_at = now()",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(snapshot.profile_source)
    .bind(&snapshot.profile_markdown)
    .bind(snapshot.profile_chars as i32)
    .bind(&snapshot.profile_hash)
    .bind(client_profile_version)
    .bind(run_id)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_is_none_source() {
        let snap = snapshot_from_raw(None);
        assert_eq!(snap.profile_source, PROFILE_SOURCE_NONE);
        assert!(snap.profile_markdown.is_empty());
        assert_eq!(snap.profile_chars, 0);
    }

    #[test]
    fn non_empty_profile_hashes_consistently() {
        let snap = snapshot_from_raw(Some("Background: developer"));
        assert_eq!(snap.profile_source, PROFILE_SOURCE_CLIENT_LOCAL);
        assert_eq!(snap.profile_hash, hash_profile(&snap.profile_markdown));
    }
}
