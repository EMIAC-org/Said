//! Legacy server personal memory — frozen by default.
//!
//! # Canonical path (target architecture)
//! Edit evidence lands in `runtime_learning_events` and `runtime_history_items`.
//! Profile mutations will be owned by the server-side profile updater (Agent B)
//! writing `runtime_user_profiles`. Runtime polish may **read** existing
//! `personal_*` rows until profile injection replaces them.
//!
//! # What is frozen
//! Direct writes to `personal_vocab_terms`, `personal_stt_replacements`, and
//! `personal_edit_policy_rules`, plus hygiene mutations that rewrite those tables.
//! This is not a product feature flag — legacy personal-memory mutation is
//! retired, not offered as an alternative runtime mode.
//!
//! # Temporary debug escape hatch
//! Set `AIRNOTE_DEBUG_LEGACY_PERSONAL_MEMORY_WRITES=1` locally to re-enable the
//! old fragmented server write path for regression only. Remove once the profile
//! updater is live and old write call sites are deleted.
//!
//! # Retirement checklist (delete after profile updater ships)
//! - `judge_and_upsert_client_learning_event` personal_* upserts
//! - `routes/runtime_history.rs` `sync_memory` writers
//! - `memory_hygiene.rs` `apply_hygiene_action` mutators
//! - `memory_hygiene_worker` (or retarget to profile-only hygiene)
//! - This module + debug env

use tracing::info;
use uuid::Uuid;

/// Debug-only env var — **not** product configuration.
pub const DEBUG_LEGACY_PERSONAL_WRITES_ENV: &str = "AIRNOTE_DEBUG_LEGACY_PERSONAL_MEMORY_WRITES";

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Temporary debug switch — re-enables old server personal-memory writes.
#[inline]
pub fn debug_legacy_personal_writes_enabled() -> bool {
    env_truthy(DEBUG_LEGACY_PERSONAL_WRITES_ENV)
}

/// True when legacy `personal_*` tables may receive new writes.
#[inline]
pub fn legacy_personal_memory_writes_allowed() -> bool {
    debug_legacy_personal_writes_enabled()
}

/// True when `personal_*` table writes are frozen (normal production default).
#[inline]
pub fn legacy_personal_table_writes_frozen() -> bool {
    !legacy_personal_memory_writes_allowed()
}

/// Learning routes: suppress personal-memory promotion side effects.
#[inline]
pub fn audit_only_personal_mutations() -> bool {
    legacy_personal_table_writes_frozen()
}

/// Log a skipped legacy personal-memory write at a route boundary.
pub fn skip_legacy_personal_write(caller: &str, op: &str, account_id: Uuid, item_count: usize) {
    info!(
        "[legacy-personal-memory] frozen writer caller={caller} op={op} account={account_id} \
         items={item_count} — profile pipeline owns mutations; evidence retained \
         (debug old writes: {DEBUG_LEGACY_PERSONAL_WRITES_ENV}=1)"
    );
}

#[cfg(test)]
pub fn enable_debug_legacy_personal_writes_for_tests() {
    unsafe {
        std::env::set_var(DEBUG_LEGACY_PERSONAL_WRITES_ENV, "1");
    }
}

#[cfg(test)]
pub fn disable_debug_legacy_personal_writes_for_tests() {
    unsafe {
        std::env::remove_var(DEBUG_LEGACY_PERSONAL_WRITES_ENV);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_debug_env() {
        unsafe {
            std::env::remove_var(DEBUG_LEGACY_PERSONAL_WRITES_ENV);
        }
    }

    #[test]
    fn personal_writes_frozen_by_default_and_debug_env() {
        clear_debug_env();
        assert!(!debug_legacy_personal_writes_enabled());
        assert!(!legacy_personal_memory_writes_allowed());
        assert!(legacy_personal_table_writes_frozen());
        assert!(audit_only_personal_mutations());

        for value in ["1", "true", "TRUE", "on"] {
            unsafe {
                std::env::set_var(DEBUG_LEGACY_PERSONAL_WRITES_ENV, value);
            }
            assert!(
                debug_legacy_personal_writes_enabled(),
                "expected truthy for {value:?}"
            );
            clear_debug_env();
        }
    }
}
