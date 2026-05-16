//! Watchdog — bare OS thread that monitors runtime health and sheds load on
//! anomaly.  Runs every 3 s; near-zero overhead when healthy.
//!
//! Detection canaries:
//!   1. Tokio responsiveness — spawn a tiny task, wait for its completion.
//!   2. SQLite pool — check `Pool::state().idle_connections`.
//!
//! Escalation:
//!   Yellow  (1-2 strikes) → log warning
//!   Orange  (≥3 strikes)  → set `shed_load` flag → bg tasks skip work
//!   Red     (≥6 strikes)  → `health_status` → "critical" (frontend shows toast)
//!
//! Recovery: when canaries pass again, strikes reset and shed flag clears.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tracing::{error, info, warn};

use crate::store::DbPool;

// ── Tuning constants ─────────────────────────────────────────────────────────

const INTERVAL: Duration = Duration::from_secs(3);
const CANARY_TIMEOUT: Duration = Duration::from_secs(2);
const SHED_THRESHOLD: u64 = 3;
const CRITICAL_THRESHOLD: u64 = 6;

// ── Shared state ─────────────────────────────────────────────────────────────

pub struct WatchdogState {
    /// When true background tasks should skip their work to free resources.
    pub shed_load: AtomicBool,
    /// Consecutive canary failures.
    pub strike_count: AtomicU64,
    /// Unix-millis of last fully-healthy tick.
    pub last_healthy_ms: AtomicU64,
    /// Current health level: 0 = green, 1 = yellow, 2 = orange, 3 = red.
    pub level: AtomicU64,
}

impl WatchdogState {
    pub fn new() -> Self {
        Self {
            shed_load: AtomicBool::new(false),
            strike_count: AtomicU64::new(0),
            last_healthy_ms: AtomicU64::new(now_ms()),
            level: AtomicU64::new(0),
        }
    }

    pub fn is_shedding(&self) -> bool {
        self.shed_load.load(Ordering::Relaxed)
    }

    pub fn health_level(&self) -> u64 {
        self.level.load(Ordering::Relaxed)
    }

    pub fn strikes(&self) -> u64 {
        self.strike_count.load(Ordering::Relaxed)
    }
}

// ── Boot ─────────────────────────────────────────────────────────────────────

pub fn spawn_watchdog(
    pool: DbPool,
    watchdog: Arc<WatchdogState>,
    tokio_handle: tokio::runtime::Handle,
) {
    std::thread::Builder::new()
        .name("said-watchdog".into())
        .spawn(move || watchdog_loop(pool, watchdog, tokio_handle))
        .expect("failed to spawn watchdog thread");
    info!(
        "[watchdog] started — interval={INTERVAL:?} shed@{SHED_THRESHOLD} critical@{CRITICAL_THRESHOLD}"
    );
}

// ── Main loop (runs on bare OS thread) ───────────────────────────────────────

fn watchdog_loop(pool: DbPool, state: Arc<WatchdogState>, handle: tokio::runtime::Handle) {
    loop {
        std::thread::sleep(INTERVAL);

        let mut failed = false;

        // ── Canary 1: tokio responsiveness ───────────────────────────────
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = handle.spawn(async move {
                let _ = tx.send(());
            });
            if rx.recv_timeout(CANARY_TIMEOUT).is_err() {
                warn!("[watchdog] tokio canary timeout — runtime may be starved");
                failed = true;
            }
        }

        // ── Canary 2: SQLite pool ────────────────────────────────────────
        {
            let ps = pool.state();
            if ps.idle_connections == 0 && ps.connections >= 5 {
                warn!(
                    "[watchdog] pool exhausted: idle={} total={}",
                    ps.idle_connections, ps.connections
                );
                failed = true;
            }
        }

        // ── Escalation / recovery ────────────────────────────────────────
        if failed {
            let strikes = state.strike_count.fetch_add(1, Ordering::Relaxed) + 1;

            if strikes >= CRITICAL_THRESHOLD {
                state.level.store(3, Ordering::Relaxed);
                error!("[watchdog] RED: {strikes} consecutive failures — system critical");
            } else if strikes >= SHED_THRESHOLD {
                state.level.store(2, Ordering::Relaxed);
                if !state.is_shedding() {
                    warn!("[watchdog] ORANGE: shedding background load after {strikes} strikes");
                    state.shed_load.store(true, Ordering::Relaxed);
                }
            } else {
                state.level.store(1, Ordering::Relaxed);
            }
        } else {
            let prev_strikes = state.strike_count.swap(0, Ordering::Relaxed);
            let prev_level = state.level.swap(0, Ordering::Relaxed);
            if prev_strikes >= SHED_THRESHOLD {
                info!(
                    "[watchdog] recovered from level {prev_level} ({prev_strikes} strikes) — clearing shed flag"
                );
                state.shed_load.store(false, Ordering::Relaxed);
            }
            state.last_healthy_ms.store(now_ms(), Ordering::Relaxed);
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
