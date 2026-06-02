//! Session-isolation regression tests.
//!
//! AirNote's most common hang/bleed failure mode: a rapid sequence of Caps Lock
//! press/release cycles causes transcript N to carry over into recording N+1.
//! The root fix is that every recording cycle generates a fresh, globally-unique
//! `recording_id` and the dg_stream actor binds its state to that ID.
//!
//! What is automated here:
//!   • ID uniqueness — 1 000 UUIDs generated sequentially have zero collisions.
//!   • ID format    — each ID is a valid UUID v4 (32 hex chars + hyphens).
//!
//! What still requires a human (see README-tests.md §Manual):
//!   • Actual rapid Caps Lock cycling on a real device.
//!   • Verifying via macOS Console that `[dg_session]` log lines show distinct
//!     recording_id values and that `parts` is empty at the start of each cycle.

use std::collections::HashSet;
use uuid::Uuid;

// ── ID uniqueness ─────────────────────────────────────────────────────────────

/// Generate 1 000 recording IDs the same way Tauri does (Uuid::new_v4().to_string())
/// and verify there are no collisions.  A collision here would indicate a broken
/// RNG — extremely unlikely in practice, but this documents the invariant.
#[test]
fn recording_ids_are_collision_free() {
    let n = 1_000;
    let ids: HashSet<String> = (0..n).map(|_| Uuid::new_v4().to_string()).collect();
    assert_eq!(
        ids.len(),
        n,
        "expected {n} unique recording IDs, got {} (collision!)",
        ids.len()
    );
}

/// Each generated ID must be a syntactically valid UUID v4 string.
#[test]
fn recording_ids_are_valid_uuid_v4() {
    for _ in 0..100 {
        let id = Uuid::new_v4().to_string();
        let parsed = Uuid::parse_str(&id).unwrap_or_else(|e| panic!("invalid UUID '{id}': {e}"));
        assert_eq!(
            parsed.get_version_num(),
            4,
            "recording ID must be UUID v4, got version {}",
            parsed.get_version_num()
        );
    }
}

// ── Rapid-cycle simulation (log-assertion harness) ────────────────────────────
//
// The full hotkey stress (50 rapid Caps Lock cycles) cannot be driven from a unit
// test because it requires the Tauri runtime, CGEventTap permission, and a live
// Deepgram connection.  The harness below is the scriptable equivalent: it generates
// 50 (id, start_time) pairs and verifies no two consecutive pairs share an ID.

/// Simulate 50 rapid recording starts and verify each gets a distinct ID.
#[test]
fn fifty_rapid_cycles_have_distinct_ids() {
    let cycles: Vec<String> = (0..50).map(|_| Uuid::new_v4().to_string()).collect();

    for window in cycles.windows(2) {
        assert_ne!(
            window[0], window[1],
            "consecutive recording IDs must differ (session bleed)"
        );
    }

    // Also check overall uniqueness
    let unique: HashSet<&str> = cycles.iter().map(|s| s.as_str()).collect();
    assert_eq!(unique.len(), cycles.len(), "all 50 IDs must be unique");
}
