//! Store-level tests for the local HITL learning gate.

#![cfg(test)]

use crate::legacy_learning;
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
fn vocabulary_upsert_allowed_by_default() {
    let pool = mem_pool();
    assert!(vocabulary::upsert(&pool, "u1", "n8n", 1.0, "auto"));
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM vocabulary", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn vocabulary_upsert_respects_scoped_learning_disabled() {
    let pool = mem_pool();
    let inserted = legacy_learning::with_legacy_write_scope(false, async {
        vocabulary::upsert(&pool, "u1", "n8n", 1.0, "auto")
    })
    .await;
    assert!(!inserted);
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM vocabulary", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
