//! Store-level freeze tests for legacy learning tables.

#![cfg(test)]

use crate::legacy_learning::{self, DEBUG_LEGACY_WRITES_ENV};
use crate::store::vocabulary;
use r2d2_sqlite::SqliteConnectionManager;

fn mem_pool() -> crate::store::DbPool {
    let mgr = SqliteConnectionManager::memory();
    let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
    let conn = pool.get().unwrap();
    conn.execute_batch(
        "CREATE TABLE local_user (id TEXT PRIMARY KEY);
         INSERT INTO local_user(id) VALUES ('u1');
         CREATE TABLE vocabulary (
             user_id                 TEXT NOT NULL REFERENCES local_user(id),
             term                    TEXT NOT NULL,
             weight                  REAL NOT NULL DEFAULT 1.0,
             use_count               INTEGER NOT NULL DEFAULT 1,
             last_used               INTEGER NOT NULL,
             source                  TEXT NOT NULL DEFAULT 'auto',
             language                TEXT,
             example_context         TEXT,
             term_type               TEXT,
             meaning                 TEXT,
             meaning_updated_at      INTEGER,
             examples_since_meaning  INTEGER NOT NULL DEFAULT 0,
             UNIQUE(user_id, term)
         );",
    )
    .unwrap();
    pool
}

#[test]
fn vocabulary_upsert_frozen_by_default_debug_env_allows() {
    let pool = mem_pool();
    legacy_learning::disable_debug_legacy_writes_for_tests();
    assert!(!vocabulary::upsert(&pool, "u1", "n8n", 1.0, "auto"));

    legacy_learning::enable_debug_legacy_writes_for_tests();
    assert!(vocabulary::upsert(&pool, "u1", "n8n", 1.0, "auto"));
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM vocabulary", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    legacy_learning::disable_debug_legacy_writes_for_tests();
}

#[test]
fn debug_env_constant_matches_runtime() {
    assert_eq!(
        DEBUG_LEGACY_WRITES_ENV,
        "AIRNOTE_DEBUG_LEGACY_LEARNING_WRITES"
    );
}
