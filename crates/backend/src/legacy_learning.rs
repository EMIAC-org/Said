//! Legacy fragmented learning — frozen by default.
//!
//! # Canonical path (target architecture)
//! Edit evidence is captured locally (`edit_events`, `recordings.final_text`) and
//! will be consumed by the server-side profile updater (Agent B). Runtime polish
//! may **read** existing legacy rows for compatibility until profile injection
//! fully replaces them.
//!
//! # What is frozen
//! Direct writes to legacy learning tables (`vocabulary`, `stt_replacements`,
//! `word_corrections`, `pending_promotions`, tier2 edit-policy side effects,
//! vocab embeddings/FTS, email memory, preference vectors, pending_edits insert)
//! are **audit-only by default**. This is not a product feature flag — legacy
//! mutation is retired, not offered as an alternative runtime mode.
//!
//! # Temporary debug escape hatch
//! Set `AIRNOTE_DEBUG_LEGACY_LEARNING_WRITES=1` locally to re-enable the old
//! fragmented write path for regression/eval only. Remove this env once the
//! DeepSeek profile updater is live and old write call sites are deleted.
//!
//! # Retirement checklist (delete after profile updater ships)
//! - Store write paths gated here (`vocabulary::upsert*`, `stt_replacements::*`,
//!   `corrections::upsert`, `tier2_edit_policy` record/penalize/activate, etc.)
//! - `routes/classify.rs` legacy promotion branches + `spawn_vocab_embedding`
//! - `routes/confirm.rs` local learn paths (keep server confirm if still needed)
//! - `routes/feedback.rs` word_corrections + vector embed side effects
//! - `stt/background.rs` alias review job
//! - `routes/vocabulary.rs` `spawn_prompt_artifact_repair`
//! - `schedule_onnx_retrain` triggers from edit learning
//! - This module + `AIRNOTE_DEBUG_LEGACY_LEARNING_WRITES` env
//! - Tables remain read-only for one release window, then migrate → drop

use std::future::Future;
use tracing::info;

/// Debug-only env var — **not** product configuration.
pub const DEBUG_LEGACY_WRITES_ENV: &str = "AIRNOTE_DEBUG_LEGACY_LEARNING_WRITES";

tokio::task_local! {
    static REQUEST_LEGACY_WRITES: bool;
}

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

/// Temporary debug switch — re-enables old fragmented learning writes.
#[inline]
pub fn debug_legacy_writes_enabled() -> bool {
    env_truthy(DEBUG_LEGACY_WRITES_ENV)
}

/// True when legacy SQLite learning tables may receive new writes.
///
/// Default: **false** (frozen). True only when the debug env is set **and**
/// the current request has `learning_enabled` (via [`with_legacy_write_scope`]).
#[inline]
pub fn legacy_learning_writes_allowed() -> bool {
    if !debug_legacy_writes_enabled() {
        return false;
    }
    REQUEST_LEGACY_WRITES
        .try_with(|allowed| *allowed)
        // Debug on but outside an HTTP handler (tests, eval binaries): allow writes.
        .unwrap_or(true)
}

/// True when legacy table writes are frozen (normal production default).
#[inline]
pub fn legacy_table_writes_frozen() -> bool {
    !legacy_learning_writes_allowed()
}

/// Run an async handler with per-request `learning_enabled` for the debug path.
pub async fn with_legacy_write_scope<F, T>(learning_enabled: bool, f: F) -> T
where
    F: Future<Output = T>,
{
    REQUEST_LEGACY_WRITES.scope(learning_enabled, f).await
}

/// Log a skipped legacy write at the store boundary.
pub fn skip_legacy_write(table: &str, op: &str, caller: &str) {
    info!(
        "[legacy-learning] frozen write table={table} op={op} caller={caller} \
         — profile-driven path owns mutations \
         (debug old writes: {DEBUG_LEGACY_WRITES_ENV}=1)"
    );
}

/// Classify/confirm routes: suppress legacy promotion side effects in the response.
#[inline]
pub fn audit_only_legacy_mutations() -> bool {
    legacy_table_writes_frozen()
}

#[cfg(test)]
pub fn enable_debug_legacy_writes_for_tests() {
    unsafe {
        std::env::set_var(DEBUG_LEGACY_WRITES_ENV, "1");
    }
}

#[cfg(test)]
pub fn disable_debug_legacy_writes_for_tests() {
    unsafe {
        std::env::remove_var(DEBUG_LEGACY_WRITES_ENV);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_debug_env() {
        unsafe {
            std::env::remove_var(DEBUG_LEGACY_WRITES_ENV);
        }
    }

    #[test]
    fn legacy_writes_frozen_by_default() {
        clear_debug_env();
        assert!(!debug_legacy_writes_enabled());
        assert!(!legacy_learning_writes_allowed());
        assert!(legacy_table_writes_frozen());
        assert!(audit_only_legacy_mutations());
    }

    #[test]
    fn debug_env_parses_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            unsafe {
                std::env::set_var(DEBUG_LEGACY_WRITES_ENV, value);
            }
            assert!(
                debug_legacy_writes_enabled(),
                "expected truthy for {value:?}"
            );
        }
        clear_debug_env();
    }

    #[tokio::test]
    async fn debug_writes_need_scope_and_learning_enabled() {
        clear_debug_env();
        unsafe {
            std::env::set_var(DEBUG_LEGACY_WRITES_ENV, "1");
        }
        let blocked =
            with_legacy_write_scope(false, async { legacy_learning_writes_allowed() }).await;
        assert!(!blocked);
        let allowed =
            with_legacy_write_scope(true, async { legacy_learning_writes_allowed() }).await;
        assert!(allowed);
        clear_debug_env();
    }

    #[tokio::test]
    async fn without_debug_env_scope_never_allows_writes() {
        clear_debug_env();
        let allowed =
            with_legacy_write_scope(true, async { legacy_learning_writes_allowed() }).await;
        assert!(!allowed);
    }
}
