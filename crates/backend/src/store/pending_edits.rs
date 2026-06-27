use rusqlite::params;
use serde::Serialize;

use crate::store::{DbPool, now_ms};

#[derive(Debug, Serialize, Clone)]
pub struct PendingEdit {
    pub id: String,
    pub recording_id: Option<String>,
    pub ai_output: String,
    pub user_kept: String,
    pub timestamp_ms: i64,
}

pub fn insert(
    pool: &DbPool,
    user_id: &str,
    recording_id: Option<&str>,
    ai_output: &str,
    user_kept: &str,
) -> Option<String> {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "pending_edits",
            "insert",
            "pending_edits::insert",
        );
        return None;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let conn = pool.get().ok()?;
    conn.execute(
        "INSERT INTO pending_edits
             (id, user_id, recording_id, ai_output, user_kept, timestamp_ms, resolved)
         VALUES (?1,?2,?3,?4,?5,?6,0)",
        params![id, user_id, recording_id, ai_output, user_kept, now_ms()],
    )
    .ok()?;
    Some(id)
}

pub fn get(pool: &DbPool, id: &str) -> Option<PendingEdit> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT id, recording_id, ai_output, user_kept, timestamp_ms
           FROM pending_edits WHERE id = ?1",
        params![id],
        |row| {
            Ok(PendingEdit {
                id: row.get(0)?,
                recording_id: row.get(1)?,
                ai_output: row.get(2)?,
                user_kept: row.get(3)?,
                timestamp_ms: row.get(4)?,
            })
        },
    )
    .ok()
}

pub fn list_pending(pool: &DbPool, user_id: &str) -> Vec<PendingEdit> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut stmt = match conn.prepare(
        "SELECT id, recording_id, ai_output, user_kept, timestamp_ms
           FROM pending_edits
          WHERE user_id = ?1 AND resolved = 0
          ORDER BY timestamp_ms DESC
          LIMIT 50",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id], |row| {
        Ok(PendingEdit {
            id: row.get(0)?,
            recording_id: row.get(1)?,
            ai_output: row.get(2)?,
            user_kept: row.get(3)?,
            timestamp_ms: row.get(4)?,
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

pub fn count_pending(pool: &DbPool, user_id: &str) -> i64 {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return 0,
    };
    conn.query_row(
        "SELECT COUNT(*) FROM pending_edits WHERE user_id = ?1 AND resolved = 0",
        params![user_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// Mark a pending edit as resolved. Returns true if the row existed.
pub fn resolve(pool: &DbPool, id: &str, action: i32) -> bool {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    conn.execute(
        "UPDATE pending_edits SET resolved = ?1 WHERE id = ?2",
        params![action, id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Dismiss a pending edit (skip it) so it never pings again.
pub fn dismiss(pool: &DbPool, id: &str) -> bool {
    resolve(pool, id, 2)
}

/// Mark pending edits as notified so they don't trigger repeat notifications.
pub fn mark_notified(pool: &DbPool, ids: &[&str]) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let now = now_ms();
    for id in ids {
        let _ = conn.execute(
            "UPDATE pending_edits SET notified_at = ?1 WHERE id = ?2 AND notified_at IS NULL",
            params![now, id],
        );
    }
}

/// List pending edits that have NOT yet been notified (for first-time notification).
pub fn list_unnotified(pool: &DbPool, user_id: &str) -> Vec<PendingEdit> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut stmt = match conn.prepare(
        "SELECT id, recording_id, ai_output, user_kept, timestamp_ms
           FROM pending_edits
          WHERE user_id = ?1 AND resolved = 0 AND notified_at IS NULL
          ORDER BY timestamp_ms DESC
          LIMIT 10",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id], |row| {
        Ok(PendingEdit {
            id: row.get(0)?,
            recording_id: row.get(1)?,
            ai_output: row.get(2)?,
            user_kept: row.get(3)?,
            timestamp_ms: row.get(4)?,
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}
