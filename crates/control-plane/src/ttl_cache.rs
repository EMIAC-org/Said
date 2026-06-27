//! Minimal in-process TTL cache for hot per-account lookups.
//!
//! The control-plane serves a small user base (a few hundred accounts), so a
//! plain `RwLock<HashMap>` with a time-to-live and explicit invalidation is all
//! we need — no Redis, no external store. Mirrors the desktop backend's
//! prefs/lexicon cache pattern (short TTL + invalidate-on-write).
//!
//! Used to collapse the per-dictation setup round-trips (tenant resolution,
//! runtime learning memory) that otherwise cost one DB round-trip each. Over a
//! tunnelled dev DB that is ~400ms/query; in production (co-located DB) it still
//! removes redundant queries and DB load.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Thread-safe map of `key -> (expires_at, value)`. Cheap to clone the values
/// out; readers never block writers for long (lookups clone under a read lock).
pub struct TtlCache<K, V> {
    ttl: Duration,
    entries: RwLock<HashMap<K, (Instant, V)>>,
}

impl<K, V> TtlCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Return a live (non-expired) cached value, or `None` on miss/expiry.
    pub fn get(&self, key: &K) -> Option<V> {
        let guard = self.entries.read().ok()?;
        match guard.get(key) {
            Some((expires_at, value)) if *expires_at > Instant::now() => Some(value.clone()),
            _ => None,
        }
    }

    /// Insert/refresh a value with a fresh TTL. Opportunistically evicts expired
    /// entries on write so the map stays bounded for a long-lived process.
    pub fn insert(&self, key: K, value: V) {
        if let Ok(mut guard) = self.entries.write() {
            let now = Instant::now();
            guard.retain(|_, (expires_at, _)| *expires_at > now);
            guard.insert(key, (now + self.ttl, value));
        }
    }

    /// Drop a key. Call after any write that changes the cached data so the next
    /// read re-loads from the database.
    pub fn invalidate(&self, key: &K) {
        if let Ok(mut guard) = self.entries.write() {
            guard.remove(key);
        }
    }
}
