//! Server-owned runtime user profiles — storage, cache, and scope resolution.

pub mod alias;
pub mod alias_safety;
pub mod bucket;
pub mod store;
pub mod updater;

use axum::http::HeaderMap;
use serde_json::Value;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::tenant::{self, TenantContext};

pub use store::ProfileRow;

/// Cache key for per-account/org profile rows.
#[derive(Clone, Debug, Eq)]
pub struct ProfileCacheKey {
    pub account_id: Uuid,
    pub org_scope: Uuid,
}

impl PartialEq for ProfileCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.account_id == other.account_id && self.org_scope == other.org_scope
    }
}

impl Hash for ProfileCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.account_id.hash(state);
        self.org_scope.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct CachedRuntimeProfile {
    pub version: i64,
    pub schema_version: i32,
    pub status: String,
    pub source_hash: String,
    /// Raw DB markdown; sanitize once via `render_profile_block` at prompt build.
    pub profile_markdown: String,
    pub profile_json: Value,
    pub org_scope: Uuid,
}

impl CachedRuntimeProfile {
    pub fn from_row(row: &ProfileRow) -> Self {
        Self {
            version: row.version,
            schema_version: row.schema_version,
            status: row.status.clone(),
            source_hash: row.source_hash.clone(),
            profile_markdown: row.profile_markdown.clone(),
            profile_json: row.profile_json.clone(),
            org_scope: row.org_scope,
        }
    }

    /// Sanitized, prompt-ready markdown (single sanitize pass at render time).
    pub fn sanitized_markdown(&self) -> String {
        said_core::polish::prompt::sanitize_profile_markdown(&self.profile_markdown)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AppBucketCacheKey {
    pub app_key: String,
}

#[derive(Clone, Debug)]
pub struct CachedAppBucket {
    pub bucket: bucket::Bucket,
    pub source: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct BucketProfileCacheKey {
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub bucket_key: String,
}

#[derive(Clone, Debug)]
pub struct CachedBucketProfile {
    pub profile_markdown: String,
    pub version: i64,
    pub status: String,
}

impl CachedBucketProfile {
    pub fn from_row(row: &bucket::BucketProfileRow) -> Self {
        Self {
            profile_markdown: row.profile_markdown.clone(),
            version: row.version,
            status: row.status.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PromptProfileContextCacheKey {
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub app_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CachedPromptProfileContext {
    pub markdown: String,
    pub profile_version: Option<i64>,
    pub bucket_key: Option<String>,
    pub bucket_source: Option<&'static str>,
}

impl CachedPromptProfileContext {
    pub fn profile_chars(&self) -> usize {
        self.markdown.chars().count()
    }
}

#[derive(Clone, Debug)]
pub struct PromptProfileContext {
    pub markdown: String,
    pub profile_version: Option<i64>,
    pub cache_hit: bool,
    pub global_profile_cache_hit: bool,
    pub app_bucket_cache_hit: bool,
    pub bucket_profile_cache_hit: bool,
    pub bucket_key: Option<String>,
    pub bucket_source: Option<&'static str>,
}

impl PromptProfileContext {
    pub fn profile_chars(&self) -> usize {
        self.markdown.chars().count()
    }

    fn from_cached(cached: CachedPromptProfileContext, cache_hit: bool) -> Self {
        Self {
            markdown: cached.markdown,
            profile_version: cached.profile_version,
            cache_hit,
            global_profile_cache_hit: cache_hit,
            app_bucket_cache_hit: cache_hit,
            bucket_profile_cache_hit: cache_hit,
            bucket_key: cached.bucket_key,
            bucket_source: cached.bucket_source,
        }
    }
}

pub fn resolve_org_scope(tenant: &TenantContext) -> Uuid {
    store::resolve_org_scope(tenant.active_org_id)
}

pub fn invalidate_profile_cache(state: &AppState, key: &ProfileCacheKey) {
    state.profile_cache.invalidate(key);
}

pub fn invalidate_profile_scope_caches(state: &AppState, account_id: Uuid, org_scope: Uuid) {
    let global_scope = alias_safety::global_org_scope();
    if org_scope == global_scope {
        state
            .profile_cache
            .invalidate_where(|key| key.account_id == account_id);
        state
            .prompt_profile_context_cache
            .invalidate_where(|key| key.account_id == account_id);
    } else {
        state.profile_cache.invalidate(&ProfileCacheKey {
            account_id,
            org_scope,
        });
        state
            .prompt_profile_context_cache
            .invalidate_where(|key| key.account_id == account_id && key.org_scope == org_scope);
    }
}

pub fn invalidate_bucket_profile_cache(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
    bucket: bucket::Bucket,
) {
    let bucket_key = bucket.as_key().to_string();
    state.bucket_profile_cache.invalidate(&BucketProfileCacheKey {
        account_id,
        org_scope,
        bucket_key: bucket_key.clone(),
    });
    state
        .prompt_profile_context_cache
        .invalidate_where(|key| key.account_id == account_id && key.org_scope == org_scope);
}

pub fn invalidate_app_bucket_cache(state: &AppState, app_key: &str) {
    let Some(app_key) = normalize_app_key(app_key) else {
        return;
    };
    state
        .app_bucket_cache
        .invalidate(&AppBucketCacheKey { app_key: app_key.clone() });
    state
        .prompt_profile_context_cache
        .invalidate_where(|key| key.app_key.as_deref() == Some(app_key.as_str()));
}

pub async fn load_profile_cached(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
) -> Result<Option<CachedRuntimeProfile>, sqlx::Error> {
    let tenant = tenant::resolve_tenant(state, user, headers)
        .await
        .map_err(|_| sqlx::Error::RowNotFound)?;
    let org_scope = resolve_org_scope(&tenant);
    let key = ProfileCacheKey {
        account_id: user.account_id,
        org_scope,
    };

    if let Some(hit) = state.profile_cache.get(&key) {
        return Ok(Some(hit));
    }

    let row = store::get_profile_with_fallback(&state.db, user.account_id, org_scope).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let cached = CachedRuntimeProfile::from_row(&row);
    state.profile_cache.insert(key, cached.clone());
    Ok(Some(cached))
}

/// Load profile using a pre-resolved org scope (for paths that already resolved tenant).
pub async fn load_profile_cached_for_scope(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<Option<CachedRuntimeProfile>, sqlx::Error> {
    load_profile_cached_for_scope_with_hit(state, account_id, org_scope)
        .await
        .map(|(profile, _)| profile)
}

pub async fn load_profile_cached_for_scope_with_hit(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<(Option<CachedRuntimeProfile>, bool), sqlx::Error> {
    let key = ProfileCacheKey {
        account_id,
        org_scope,
    };
    if let Some(hit) = state.profile_cache.get(&key) {
        return Ok((Some(hit), true));
    }
    let row = store::get_profile_with_fallback(&state.db, account_id, org_scope).await?;
    let Some(row) = row else {
        return Ok((None, false));
    };
    let cached = CachedRuntimeProfile::from_row(&row);
    state.profile_cache.insert(key, cached.clone());
    Ok((Some(cached), false))
}

pub async fn resolve_bucket_cached(state: &AppState, app_key: &str) -> (CachedAppBucket, bool) {
    let Some(app_key) = normalize_app_key(app_key) else {
        return (
            CachedAppBucket {
                bucket: bucket::Bucket::Default,
                source: "default",
            },
            false,
        );
    };
    let key = AppBucketCacheKey { app_key: app_key.clone() };
    if let Some(hit) = state.app_bucket_cache.get(&key) {
        return (hit, true);
    }
    let (bucket, source) = bucket::resolve_bucket_with_source(&state.db, &app_key).await;
    let cached = CachedAppBucket { bucket, source };
    state.app_bucket_cache.insert(key, cached.clone());
    (cached, false)
}

pub async fn load_bucket_profile_cached(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
    bucket: bucket::Bucket,
) -> Result<(Option<CachedBucketProfile>, bool), sqlx::Error> {
    let key = BucketProfileCacheKey {
        account_id,
        org_scope,
        bucket_key: bucket.as_key().to_string(),
    };
    if let Some(hit) = state.bucket_profile_cache.get(&key) {
        return Ok((hit, true));
    }
    let row = bucket::get_bucket_profile(&state.db, account_id, org_scope, bucket).await?;
    let cached = row.as_ref().map(CachedBucketProfile::from_row);
    state.bucket_profile_cache.insert(key, cached.clone());
    Ok((cached, false))
}

pub async fn load_prompt_profile_context_cached(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
    target_app: Option<&str>,
) -> PromptProfileContext {
    let app_key = target_app.and_then(normalize_app_key);
    let context_key = PromptProfileContextCacheKey {
        account_id,
        org_scope,
        app_key: app_key.clone(),
    };
    if let Some(hit) = state.prompt_profile_context_cache.get(&context_key) {
        return PromptProfileContext::from_cached(hit, true);
    }

    let mut parts: Vec<String> = Vec::new();
    let mut profile_version = None;
    let mut global_profile_cache_hit = false;
    if let Ok((Some(profile), hit)) =
        load_profile_cached_for_scope_with_hit(state, account_id, org_scope).await
    {
        global_profile_cache_hit = hit;
        profile_version = Some(profile.version);
        if !profile.profile_markdown.trim().is_empty() {
            parts.push(profile.profile_markdown);
        }
    }

    let mut app_bucket_cache_hit = false;
    let mut bucket_profile_cache_hit = false;
    let mut bucket_key = None;
    let mut bucket_source = None;
    if let Some(app_key) = app_key.as_deref() {
        let (app_bucket, app_hit) = resolve_bucket_cached(state, app_key).await;
        app_bucket_cache_hit = app_hit;
        bucket_key = Some(app_bucket.bucket.as_key().to_string());
        bucket_source = Some(app_bucket.source);
        if let Ok((Some(overlay), bucket_hit)) =
            load_bucket_profile_cached(state, account_id, org_scope, app_bucket.bucket).await
        {
            bucket_profile_cache_hit = bucket_hit;
            if !overlay.profile_markdown.trim().is_empty() {
                parts.push(overlay.profile_markdown);
            }
        }
    }

    let cached = CachedPromptProfileContext {
        markdown: parts.join("\n\n"),
        profile_version,
        bucket_key,
        bucket_source,
    };
    state
        .prompt_profile_context_cache
        .insert(context_key, cached.clone());

    PromptProfileContext {
        markdown: cached.markdown,
        profile_version: cached.profile_version,
        cache_hit: false,
        global_profile_cache_hit,
        app_bucket_cache_hit,
        bucket_profile_cache_hit,
        bucket_key: cached.bucket_key,
        bucket_source: cached.bucket_source,
    }
}

fn normalize_app_key(app_key: &str) -> Option<String> {
    let app_key = app_key.trim().to_lowercase();
    if app_key.is_empty() {
        None
    } else {
        Some(app_key)
    }
}

#[cfg(test)]
mod tests {
    use super::alias_safety::{global_org_scope, is_common_alias_source};

    #[test]
    fn global_scope_is_sentinel_uuid() {
        assert_eq!(
            global_org_scope().to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn common_alias_guard_blocks_kaam() {
        assert!(is_common_alias_source("kaam"));
    }
}
