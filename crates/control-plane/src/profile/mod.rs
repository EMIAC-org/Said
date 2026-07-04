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

pub fn resolve_org_scope(tenant: &TenantContext) -> Uuid {
    store::resolve_org_scope(tenant.active_org_id)
}

pub fn invalidate_profile_cache(state: &AppState, key: &ProfileCacheKey) {
    state.profile_cache.invalidate(key);
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
    let key = ProfileCacheKey {
        account_id,
        org_scope,
    };
    if let Some(hit) = state.profile_cache.get(&key) {
        return Ok(Some(hit));
    }
    let row = store::get_profile_with_fallback(&state.db, account_id, org_scope).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let cached = CachedRuntimeProfile::from_row(&row);
    state.profile_cache.insert(key, cached.clone());
    Ok(Some(cached))
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
