//! App-context buckets: fixed taxonomy + app->bucket resolution + per-bucket
//! ("conditional block") overlay storage.
//!
//! The GLOBAL profile (the person) lives in `runtime_user_profiles` and is untouched
//! here. This module owns the per-bucket STYLE overlays that condition polish on WHERE
//! the user is dictating (a coding IDE vs a messenger vs a formal-writing app).
//!
//! Bucketing is done in two layers:
//!   1. `STATIC_BUCKETS` — compiled-in, authoritative mappings for well-known apps.
//!   2. `app_bucket_map` — persisted AI-agent classifications of previously-unknown
//!      apps (bounded output: exactly one of the fixed `Bucket` variants).
//! Resolution order: static -> agent-cached -> Default.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// The fixed bucket taxonomy. Adding a variant means updating `as_key`, `from_key`,
/// `ALL`, and the two DB CHECK constraints in migration 032.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// IDEs, editors, terminals, AI coding surfaces (VS Code, Cursor, Claude Code…).
    Coding,
    /// Chat / messengers (WhatsApp, Telegram, Slack, Discord…).
    Messaging,
    /// Issue trackers / work-management (Jira, Linear, Lark…).
    WorkTracker,
    /// Long-form formal writing (Mail, Outlook, docs…).
    FormalWriting,
    /// Unknown / ambiguous — behaves as today's un-bucketed polish.
    Default,
}

impl Bucket {
    pub const ALL: [Bucket; 5] = [
        Bucket::Coding,
        Bucket::Messaging,
        Bucket::WorkTracker,
        Bucket::FormalWriting,
        Bucket::Default,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            Bucket::Coding => "coding",
            Bucket::Messaging => "messaging",
            Bucket::WorkTracker => "work_tracker",
            Bucket::FormalWriting => "formal_writing",
            Bucket::Default => "default",
        }
    }

    pub fn from_key(key: &str) -> Option<Bucket> {
        match key {
            "coding" => Some(Bucket::Coding),
            "messaging" => Some(Bucket::Messaging),
            "work_tracker" => Some(Bucket::WorkTracker),
            "formal_writing" => Some(Bucket::FormalWriting),
            "default" => Some(Bucket::Default),
            _ => None,
        }
    }
}

/// Compiled-in authoritative mappings for well-known apps. Keys are lowercase and
/// matched against BOTH the full `target_app` (macOS bundle id) and its basename
/// (Windows exe path). Everything not listed here falls to the agent-cache / Default.
const STATIC_BUCKETS: &[(&str, Bucket)] = &[
    // --- Coding: editors / IDEs / terminals / AI coding surfaces ---
    ("com.microsoft.vscode", Bucket::Coding),
    ("com.microsoft.vscodeinsiders", Bucket::Coding),
    ("com.todesktop.230313mzl4w4u92", Bucket::Coding), // Cursor
    ("dev.zed.zed", Bucket::Coding),
    ("com.apple.dt.xcode", Bucket::Coding),
    ("com.sublimetext.4", Bucket::Coding),
    ("com.googlecode.iterm2", Bucket::Coding),
    ("com.apple.terminal", Bucket::Coding),
    ("com.github.wez.wezterm", Bucket::Coding),
    ("com.mitchellh.ghostty", Bucket::Coding),
    ("code.exe", Bucket::Coding),
    ("cursor.exe", Bucket::Coding),
    ("devenv.exe", Bucket::Coding),
    ("windowsterminal.exe", Bucket::Coding),
    ("idea64.exe", Bucket::Coding),
    // --- Messaging: chat / messengers ---
    ("net.whatsapp.whatsapp", Bucket::Messaging),
    ("ru.keepcoder.telegram", Bucket::Messaging),
    ("com.tinyspeck.slackmacgap", Bucket::Messaging),
    ("com.hnc.discord", Bucket::Messaging),
    ("com.apple.messages", Bucket::Messaging),
    ("whatsapp.exe", Bucket::Messaging),
    ("telegram.exe", Bucket::Messaging),
    ("slack.exe", Bucket::Messaging),
    ("discord.exe", Bucket::Messaging),
    // --- WorkTracker: issue trackers / work management ---
    ("com.linear.linear", Bucket::WorkTracker),
    ("com.electron.lark", Bucket::WorkTracker),
    ("linear.exe", Bucket::WorkTracker),
    // --- FormalWriting: mail / long-form ---
    ("com.apple.mail", Bucket::FormalWriting),
    ("com.microsoft.outlook", Bucket::FormalWriting),
    ("olk.exe", Bucket::FormalWriting),
    ("outlook.exe", Bucket::FormalWriting),
];

/// Lowercased match keys for an `app_key`: the full value plus its path basename
/// (Windows `target_app` is a full exe path; macOS is a bundle id with no slashes).
fn match_keys(app_key: &str) -> Vec<String> {
    let norm = app_key.trim().to_lowercase();
    if norm.is_empty() {
        return Vec::new();
    }
    let base = norm
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(norm.as_str())
        .to_string();
    if base == norm {
        vec![norm]
    } else {
        vec![norm, base]
    }
}

/// Static (compiled-in) bucket for a known app, if any.
pub fn static_bucket(app_key: &str) -> Option<Bucket> {
    let keys = match_keys(app_key);
    for k in &keys {
        if let Some((_, b)) = STATIC_BUCKETS.iter().find(|(app, _)| app == k) {
            return Some(*b);
        }
    }
    None
}

/// Resolve an `app_key` (macOS bundle id or Windows exe path) to its bucket.
///
/// Order: static table -> persisted agent classification (`app_bucket_map`) -> Default.
/// Returns `Default` for unknown apps; the loop layer is responsible for enqueuing an
/// agent classification so a future call resolves to a real bucket.
pub async fn resolve_bucket(db: &PgPool, app_key: &str) -> Bucket {
    resolve_bucket_with_source(db, app_key).await.0
}

/// Like [`resolve_bucket`] but also reports where the mapping came from:
/// `"user"` (explicit override) > `"static"` (compiled-in) > `"agent"` (classifier) > `"default"`.
/// A user override wins over the static map so re-filing in the Buckets UI always sticks.
pub async fn resolve_bucket_with_source(db: &PgPool, app_key: &str) -> (Bucket, &'static str) {
    let keys = match_keys(app_key);
    if keys.is_empty() {
        return (Bucket::Default, "default");
    }
    let mapping = get_mapped_bucket_full(db, &keys[0]).await.ok().flatten();
    // 1. Explicit user override wins over everything.
    if let Some((key, source)) = &mapping
        && source == "user"
        && let Some(b) = Bucket::from_key(key)
    {
        return (b, "user");
    }
    // 2. Compiled-in authoritative static rule.
    if let Some(b) = static_bucket(app_key) {
        return (b, "static");
    }
    // 3. Agent-cached classification.
    if let Some((key, _)) = &mapping
        && let Some(b) = Bucket::from_key(key)
    {
        return (b, "agent");
    }
    (Bucket::Default, "default")
}

/// Fetch the persisted mapping (bucket_key, source) for an app_key (already lowercased).
pub async fn get_mapped_bucket_full(
    db: &PgPool,
    app_key: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT bucket_key, source FROM app_bucket_map WHERE app_key = $1")
            .bind(app_key)
            .fetch_optional(db)
            .await?;
    Ok(row)
}

/// Fetch a persisted agent/admin mapping for an app_key (already lowercased).
pub async fn get_mapped_bucket(db: &PgPool, app_key: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT bucket_key FROM app_bucket_map WHERE app_key = $1")
            .bind(app_key)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(b,)| b))
}

/// Persist an app->bucket mapping (used by the agent classifier / admin overrides).
/// `app_key` is stored lowercased to match `resolve_bucket`.
pub async fn upsert_app_bucket(
    db: &PgPool,
    app_key: &str,
    bucket: Bucket,
    source: &str,
    confidence: f64,
) -> Result<(), sqlx::Error> {
    let norm = app_key.trim().to_lowercase();
    sqlx::query(
        "INSERT INTO app_bucket_map (app_key, bucket_key, source, confidence)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (app_key) DO UPDATE
            SET bucket_key = EXCLUDED.bucket_key,
                source = EXCLUDED.source,
                confidence = EXCLUDED.confidence,
                updated_at = now()",
    )
    .bind(norm)
    .bind(bucket.as_key())
    .bind(source)
    .bind(confidence)
    .execute(db)
    .await?;
    Ok(())
}

// -------------------------------------------------------------------------------------
// Per-bucket overlay profile storage (runtime_user_bucket_profiles).
// Mirrors profile::store, keyed additionally by bucket_key.
// -------------------------------------------------------------------------------------

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct BucketProfileRow {
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub bucket_key: String,
    pub profile_json: Value,
    pub profile_markdown: String,
    pub version: i64,
    pub schema_version: i32,
    pub status: String,
    pub dirty_at: Option<DateTime<Utc>>,
    pub last_rebuilt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const BUCKET_PROFILE_COLS: &str = "account_id, org_scope, bucket_key, profile_json, profile_markdown, \
     version, schema_version, status, dirty_at, last_rebuilt_at, last_error, created_at, updated_at";

pub async fn get_bucket_profile(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    bucket: Bucket,
) -> Result<Option<BucketProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, BucketProfileRow>(&format!(
        "SELECT {BUCKET_PROFILE_COLS} FROM runtime_user_bucket_profiles
          WHERE account_id = $1 AND org_scope = $2 AND bucket_key = $3"
    ))
    .bind(account_id)
    .bind(org_scope)
    .bind(bucket.as_key())
    .fetch_optional(db)
    .await
}

/// All bucket overlays for an account/org scope (for injection + admin/inspection).
pub async fn list_bucket_profiles(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<Vec<BucketProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, BucketProfileRow>(&format!(
        "SELECT {BUCKET_PROFILE_COLS} FROM runtime_user_bucket_profiles
          WHERE account_id = $1 AND org_scope = $2
          ORDER BY bucket_key"
    ))
    .bind(account_id)
    .bind(org_scope)
    .fetch_all(db)
    .await
}

pub async fn ensure_bucket_profile_row(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    bucket: Bucket,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO runtime_user_bucket_profiles (account_id, org_scope, bucket_key)
         VALUES ($1, $2, $3)
         ON CONFLICT (account_id, org_scope, bucket_key) DO NOTHING",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(bucket.as_key())
    .execute(db)
    .await?;
    Ok(())
}

/// Overwrite a bucket overlay's json+markdown and bump its version. The rebuild/merge
/// (loop) layer produces `profile_json` / `profile_markdown`; this just persists them.
pub async fn upsert_bucket_profile(
    db: &PgPool,
    account_id: Uuid,
    org_scope: Uuid,
    bucket: Bucket,
    profile_json: Value,
    profile_markdown: String,
) -> Result<BucketProfileRow, sqlx::Error> {
    ensure_bucket_profile_row(db, account_id, org_scope, bucket).await?;
    sqlx::query_as::<_, BucketProfileRow>(&format!(
        "UPDATE runtime_user_bucket_profiles
            SET profile_json = $4,
                profile_markdown = $5,
                version = version + 1,
                status = 'ready',
                last_error = NULL,
                last_rebuilt_at = now(),
                dirty_at = NULL,
                updated_at = now()
          WHERE account_id = $1 AND org_scope = $2 AND bucket_key = $3
      RETURNING {BUCKET_PROFILE_COLS}"
    ))
    .bind(account_id)
    .bind(org_scope)
    .bind(bucket.as_key())
    .bind(profile_json)
    .bind(profile_markdown)
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_key_roundtrips() {
        for b in Bucket::ALL {
            assert_eq!(Bucket::from_key(b.as_key()), Some(b));
        }
        assert_eq!(Bucket::from_key("nope"), None);
    }

    #[test]
    fn static_bucket_resolves_known_macos_bundle() {
        assert_eq!(static_bucket("com.microsoft.VSCode"), Some(Bucket::Coding));
        assert_eq!(
            static_bucket("net.whatsapp.WhatsApp"),
            Some(Bucket::Messaging)
        );
        assert_eq!(static_bucket("com.apple.Mail"), Some(Bucket::FormalWriting));
    }

    #[test]
    fn static_bucket_resolves_windows_exe_path_by_basename() {
        assert_eq!(
            static_bucket(r"C:\Users\me\AppData\Local\Programs\cursor\Cursor.exe"),
            Some(Bucket::Coding)
        );
        assert_eq!(
            static_bucket(r"C:\Program Files\Slack\slack.exe"),
            Some(Bucket::Messaging)
        );
    }

    #[test]
    fn unknown_app_has_no_static_bucket() {
        assert_eq!(static_bucket("com.acme.unknownapp"), None);
        assert_eq!(static_bucket(""), None);
    }

    #[test]
    fn match_keys_splits_windows_path() {
        let keys = match_keys(r"C:\a\b\Code.exe");
        assert!(keys.contains(&"code.exe".to_string()));
    }
}
