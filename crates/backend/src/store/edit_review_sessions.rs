use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::store::{DbPool, now_ms};

#[derive(Debug, Clone, Serialize)]
pub struct EditReviewSession {
    pub id: String,
    pub recording_id: String,
    pub ai_output: String,
    pub user_kept: String,
    pub review_candidates: Vec<serde_json::Value>,
    pub detected_changes: Vec<serde_json::Value>,
    pub created_at_ms: i64,
}

pub fn insert(
    pool: &DbPool,
    user_id: &str,
    recording_id: &str,
    ai_output: &str,
    user_kept: &str,
    review_candidates: &[serde_json::Value],
    detected_changes: &[serde_json::Value],
) -> Option<String> {
    let candidates_json = serde_json::to_string(review_candidates).ok()?;
    let changes_json = serde_json::to_string(detected_changes).ok()?;
    let id = uuid::Uuid::new_v4().to_string();
    let conn = pool.get().ok()?;
    conn.execute(
        "INSERT OR IGNORE INTO edit_review_sessions
             (id, user_id, recording_id, ai_output, user_kept,
              review_candidates_json, detected_changes_json, created_at_ms, status)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,0)",
        params![
            id,
            user_id,
            recording_id,
            ai_output,
            user_kept,
            candidates_json,
            changes_json,
            now_ms(),
        ],
    )
    .ok()?;
    conn.query_row(
        "SELECT id FROM edit_review_sessions
          WHERE user_id = ?1 AND recording_id = ?2 AND status = 0",
        params![user_id, recording_id],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn next_pending(pool: &DbPool, user_id: &str) -> Option<EditReviewSession> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT id, recording_id, ai_output, user_kept,
                review_candidates_json, detected_changes_json, created_at_ms
           FROM edit_review_sessions
          WHERE user_id = ?1 AND status = 0
          ORDER BY created_at_ms ASC
          LIMIT 1",
        params![user_id],
        |row| {
            let review_candidates_json: String = row.get(4)?;
            let detected_changes_json: String = row.get(5)?;
            Ok(EditReviewSession {
                id: row.get(0)?,
                recording_id: row.get(1)?,
                ai_output: row.get(2)?,
                user_kept: row.get(3)?,
                review_candidates: serde_json::from_str(&review_candidates_json)
                    .unwrap_or_default(),
                detected_changes: serde_json::from_str(&detected_changes_json).unwrap_or_default(),
                created_at_ms: row.get(6)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn resolve(pool: &DbPool, user_id: &str, id: &str, status: i64) -> bool {
    if !matches!(status, 1 | 2) {
        return false;
    }
    let conn = match pool.get() {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    conn.execute(
        "UPDATE edit_review_sessions
            SET status = ?1, resolved_at_ms = ?2
          WHERE id = ?3 AND user_id = ?4 AND status = 0",
        params![status, now_ms(), id, user_id],
    )
    .map(|updated| updated > 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    fn pool() -> DbPool {
        let pool = Pool::builder()
            .max_size(1)
            .build(SqliteConnectionManager::memory())
            .unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE local_user (id TEXT PRIMARY KEY);
                 CREATE TABLE recordings (id TEXT PRIMARY KEY);
                 CREATE TABLE edit_review_sessions (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL REFERENCES local_user(id),
                    recording_id TEXT NOT NULL REFERENCES recordings(id),
                    ai_output TEXT NOT NULL,
                    user_kept TEXT NOT NULL,
                    review_candidates_json TEXT NOT NULL,
                    detected_changes_json TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    resolved_at_ms INTEGER,
                    status INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE UNIQUE INDEX idx_edit_review_recording_pending
                    ON edit_review_sessions (user_id, recording_id) WHERE status = 0;
                 INSERT INTO local_user(id) VALUES ('u1');
                 INSERT INTO recordings(id) VALUES ('r1'), ('r2');",
            )
            .unwrap();
        }
        pool
    }

    #[test]
    fn queue_is_fifo_and_insert_is_idempotent_per_recording() {
        let pool = pool();
        let first = insert(
            &pool,
            "u1",
            "r1",
            "before",
            "after",
            &[serde_json::json!({"corrected": "after"})],
            &[serde_json::json!({"reason": "stt_error"})],
        )
        .unwrap();
        let duplicate = insert(&pool, "u1", "r1", "before", "after", &[], &[]).unwrap();
        assert_eq!(duplicate, first);

        let second = insert(&pool, "u1", "r2", "x", "y", &[], &[]).unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE edit_review_sessions SET created_at_ms = 1 WHERE id = ?1",
                params![first],
            )
            .unwrap();
            conn.execute(
                "UPDATE edit_review_sessions SET created_at_ms = 2 WHERE id = ?1",
                params![second],
            )
            .unwrap();
        }

        assert_eq!(next_pending(&pool, "u1").unwrap().id, first);
        assert!(resolve(&pool, "u1", &first, 1));
        assert_eq!(next_pending(&pool, "u1").unwrap().id, second);
        assert!(resolve(&pool, "u1", &second, 2));
        assert!(next_pending(&pool, "u1").is_none());
    }
}
