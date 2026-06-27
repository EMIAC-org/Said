//! Local HITL learning gate.
//!
//! The canonical learning path is local and human-in-the-loop:
//! edit evidence is captured locally, classified, shown in the review card, and
//! only approved candidates write to `vocabulary`, `stt_replacements`,
//! `word_corrections`, and related support tables.
//!
//! This module remains as a central choke point so request-level
//! `learning_enabled=false` can still suppress writes, but there is no product
//! env switch that freezes the pipeline by default.

use std::future::Future;
use tracing::warn;

tokio::task_local! {
    static REQUEST_LEGACY_WRITES: bool;
}

/// True when legacy SQLite learning tables may receive new writes.
///
/// Default: **true** outside a scoped request. Inside a request, this mirrors
/// the user's `learning_enabled` preference.
#[inline]
pub fn legacy_learning_writes_allowed() -> bool {
    REQUEST_LEGACY_WRITES
        .try_with(|allowed| *allowed)
        .unwrap_or(true)
}

/// True when local learning table writes are disabled for the current request.
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
    warn!(
        "[learning] skipped write table={table} op={op} caller={caller} \
         — user learning is disabled for this request"
    );
}

/// Classify/confirm routes: suppress learning side effects only when the user
/// disabled learning for the current request.
#[inline]
pub fn audit_only_legacy_mutations() -> bool {
    legacy_table_writes_frozen()
}

#[cfg(test)]
pub fn enable_debug_legacy_writes_for_tests() {
    // Kept for older tests; writes are enabled by default now.
}

#[cfg(test)]
pub fn disable_debug_legacy_writes_for_tests() {
    // Kept for older tests; use `with_legacy_write_scope(false, ...)` to test blocking.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_writes_allowed_by_default() {
        assert!(legacy_learning_writes_allowed());
        assert!(!legacy_table_writes_frozen());
        assert!(!audit_only_legacy_mutations());
    }

    #[tokio::test]
    async fn scoped_learning_disabled_blocks_writes() {
        let blocked =
            with_legacy_write_scope(false, async { legacy_learning_writes_allowed() }).await;
        assert!(!blocked);
        let allowed =
            with_legacy_write_scope(true, async { legacy_learning_writes_allowed() }).await;
        assert!(allowed);
    }
}
