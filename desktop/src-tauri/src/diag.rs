//! Lightweight diagnostics instrumentation for hard-to-reproduce desktop hangs.
//!
//! Two facilities, both cheap and safe to call from any thread:
//!
//!   1. **SharedApp lock-holder tracking** — `std::sync::Mutex` can't tell you
//!      who holds it, so instrumented acquisitions publish a label + acquire
//!      time here and clear it on release. When the hotkey state machine stalls
//!      on a busy lock, the diagnostics event can name the actual holder and how
//!      long it has been held instead of just reporting `lock_busy`.
//!
//!   2. **Breadcrumb ring** — a small bounded trail of recording-lifecycle
//!      milestones (start enter, start recording, finish enter, queued-finish
//!      request, …). Attached to stuck-state events so a remote machine's report
//!      shows the exact sequence leading up to the stall.
//!
//! Labels and breadcrumb strings are fixed identifiers only — never transcript,
//! audio, or user text — so they are safe to upload to the diagnostics endpoint.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── SharedApp lock-holder tracking ───────────────────────────────────────────

static LOCK_HOLDER: Mutex<Option<&'static str>> = Mutex::new(None);
/// Epoch-ms when the current holder acquired the lock; `0` when free.
static LOCK_SINCE_MS: AtomicU64 = AtomicU64::new(0);

/// Record that an instrumented caller just acquired the SharedApp mutex.
pub fn note_lock_acquired(label: &'static str) {
    LOCK_SINCE_MS.store(now_ms(), Ordering::SeqCst);
    if let Ok(mut holder) = LOCK_HOLDER.lock() {
        *holder = Some(label);
    }
}

/// Record that the instrumented holder released the SharedApp mutex.
pub fn note_lock_released() {
    if let Ok(mut holder) = LOCK_HOLDER.lock() {
        *holder = None;
    }
    LOCK_SINCE_MS.store(0, Ordering::SeqCst);
}

/// `{ holder, held_ms }` describing the current instrumented lock holder.
/// `holder` is `"none"` when no instrumented caller holds the lock.
pub fn lock_status() -> Value {
    let holder = LOCK_HOLDER.lock().ok().and_then(|h| *h).unwrap_or("none");
    let since = LOCK_SINCE_MS.load(Ordering::SeqCst);
    let held_ms = if since == 0 {
        0
    } else {
        now_ms().saturating_sub(since)
    };
    json!({ "holder": holder, "held_ms": held_ms })
}

// ── Breadcrumb ring ──────────────────────────────────────────────────────────

const MAX_CRUMBS: usize = 32;

struct Crumb {
    t_ms: u64,
    ev: String,
}

static CRUMBS: Mutex<VecDeque<Crumb>> = Mutex::new(VecDeque::new());
static CRUMB_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn breadcrumb_log_path() -> std::path::PathBuf {
    said_core::paths::log_dir().join("crash-breadcrumbs.jsonl")
}

fn append_persistent_breadcrumb(t_ms: u64, ev: &str) {
    let path = breadcrumb_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let sanitized = ev
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' => ' ',
            _ => ch,
        })
        .take(240)
        .collect::<String>();
    let line = json!({
        "seq": CRUMB_SEQ.fetch_add(1, Ordering::Relaxed) + 1,
        "t_ms": t_ms,
        "ev": sanitized,
    })
    .to_string();

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Append a recording-lifecycle milestone. Keep `ev` a fixed identifier
/// (e.g. `"start:recording"`), never user content.
pub fn breadcrumb(ev: impl Into<String>) {
    let t_ms = now_ms();
    let ev = ev.into();
    if let Ok(mut q) = CRUMBS.lock() {
        if q.len() >= MAX_CRUMBS {
            q.pop_front();
        }
        q.push_back(Crumb {
            t_ms,
            ev: ev.clone(),
        });
    }
    append_persistent_breadcrumb(t_ms, &ev);
}

/// The most recent `limit` breadcrumbs, oldest-first, each tagged with how many
/// milliseconds ago it happened. Shape: `[{ "ago_ms": u64, "ev": str }, …]`.
pub fn breadcrumbs(limit: usize) -> Value {
    let now = now_ms();
    let Ok(q) = CRUMBS.lock() else {
        return json!([]);
    };
    let mut items: Vec<Value> = q
        .iter()
        .rev()
        .take(limit)
        .map(|c| {
            json!({
                "ago_ms": now.saturating_sub(c.t_ms),
                "ev": c.ev,
            })
        })
        .collect();
    items.reverse();
    json!(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock acquire → hold → release cycle must report correct state at each step.
    /// Combined into one test to avoid races on the global LOCK_HOLDER static when
    /// `cargo test` runs tests in parallel threads.
    #[test]
    fn lock_acquire_hold_release_sequence() {
        // Start clean
        note_lock_released();
        let free = lock_status();
        assert_eq!(free["holder"], "none", "lock must start free");
        assert_eq!(free["held_ms"], 0, "held_ms must be 0 when free");

        // Acquire
        note_lock_acquired("test:hotkey");
        let held = lock_status();
        assert_eq!(held["holder"], "test:hotkey", "holder must match label");
        // held_ms may be 0 on fast machines — just ensure it doesn't panic or overflow
        assert!(held["held_ms"].as_u64().is_some(), "held_ms must be a u64");

        // Release
        note_lock_released();
        let after = lock_status();
        assert_eq!(after["holder"], "none", "holder must clear after release");
        assert_eq!(after["held_ms"], 0, "held_ms must be 0 after release");
    }

    /// Pushing more than MAX_CRUMBS (32) breadcrumbs must not exceed the capacity.
    #[test]
    fn breadcrumb_ring_wraps_at_capacity() {
        for i in 0..50usize {
            breadcrumb(format!("wrap-test:{i}"));
        }
        let crumbs = breadcrumbs(100);
        let arr = crumbs.as_array().expect("breadcrumbs returns an array");
        assert!(
            arr.len() <= MAX_CRUMBS,
            "ring must not exceed {MAX_CRUMBS}, got {}",
            arr.len()
        );
    }

    /// The `limit` parameter must cap the returned slice.
    #[test]
    fn breadcrumbs_limit_caps_result() {
        // Push at least 5 known events
        for i in 0..5usize {
            breadcrumb(format!("limit-test:{i}"));
        }
        let crumbs = breadcrumbs(3);
        let arr = crumbs.as_array().expect("breadcrumbs returns an array");
        assert!(
            arr.len() <= 3,
            "limit=3 must return at most 3 crumbs, got {}",
            arr.len()
        );
    }

    /// Each breadcrumb entry must have both `ago_ms` and `ev` fields.
    #[test]
    fn breadcrumb_entry_has_required_fields() {
        breadcrumb("schema-test:event");
        let crumbs = breadcrumbs(1);
        let arr = crumbs.as_array().expect("breadcrumbs returns an array");
        assert!(!arr.is_empty(), "at least one crumb must be present");
        let last = arr.last().unwrap();
        assert!(last.get("ago_ms").is_some(), "crumb must have ago_ms");
        assert!(last.get("ev").is_some(), "crumb must have ev");
    }
}
