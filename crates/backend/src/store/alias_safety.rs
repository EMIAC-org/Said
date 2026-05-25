use rusqlite::{OptionalExtension, params};

use super::{DbPool, now_ms};

#[derive(Debug, Clone, PartialEq)]
pub struct AliasSafetyJudgment {
    pub source_norm: String,
    pub verdict: String,
    pub confidence: f64,
    pub provider: String,
    pub model: String,
    pub reason: String,
    pub created_at: i64,
    pub last_used_at: i64,
}

pub fn get(pool: &DbPool, user_id: &str, source_norm: &str) -> Option<AliasSafetyJudgment> {
    let source_norm = source_norm.trim();
    if source_norm.is_empty() {
        return None;
    }
    let conn = pool.get().ok()?;
    let found = conn
        .query_row(
            "SELECT source_norm, verdict, confidence, provider, model, reason, created_at, last_used_at
               FROM alias_safety_judgments
              WHERE user_id = ?1 AND source_norm = ?2",
            params![user_id, source_norm],
            |row| {
                Ok(AliasSafetyJudgment {
                    source_norm: row.get(0)?,
                    verdict: row.get(1)?,
                    confidence: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    reason: row.get(5)?,
                    created_at: row.get(6)?,
                    last_used_at: row.get(7)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten();
    if found.is_some() {
        let _ = conn.execute(
            "UPDATE alias_safety_judgments
                SET last_used_at = ?3
              WHERE user_id = ?1 AND source_norm = ?2",
            params![user_id, source_norm, now_ms()],
        );
    }
    found
}

pub fn upsert(
    pool: &DbPool,
    user_id: &str,
    source_norm: &str,
    verdict: &str,
    confidence: f64,
    provider: &str,
    model: &str,
    reason: &str,
) -> bool {
    let source_norm = source_norm.trim();
    if source_norm.is_empty() || verdict.trim().is_empty() {
        return false;
    }
    let Ok(conn) = pool.get() else {
        return false;
    };
    let now = now_ms();
    conn.execute(
        "INSERT INTO alias_safety_judgments
             (user_id, source_norm, verdict, confidence, provider, model, reason, created_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(user_id, source_norm) DO UPDATE SET
             verdict = excluded.verdict,
             confidence = excluded.confidence,
             provider = excluded.provider,
             model = excluded.model,
             reason = excluded.reason,
             last_used_at = excluded.last_used_at",
        params![
            user_id,
            source_norm,
            verdict.trim(),
            confidence.clamp(0.0, 1.0),
            provider.trim(),
            model.trim(),
            reason.trim(),
            now,
        ],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use r2d2_sqlite::SqliteConnectionManager;

    use super::*;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE alias_safety_judgments (
                user_id TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
                source_norm TEXT NOT NULL,
                verdict TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.0,
                provider TEXT NOT NULL DEFAULT 'local',
                model TEXT NOT NULL DEFAULT '',
                reason TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                last_used_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, source_norm)
             );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn cache_round_trip_updates_existing_verdict() {
        let pool = mem_pool();
        assert!(upsert(
            &pool,
            "u1",
            "kaisa",
            "common_block",
            0.99,
            "local",
            "",
            "common word",
        ));
        let first = get(&pool, "u1", "kaisa").unwrap();
        assert_eq!(first.verdict, "common_block");
        assert_eq!(first.provider, "local");

        assert!(upsert(
            &pool,
            "u1",
            "kaisa",
            "ambiguous_block",
            0.5,
            "groq",
            "llama-3.1-8b-instant",
            "unclear",
        ));
        let second = get(&pool, "u1", "kaisa").unwrap();
        assert_eq!(second.verdict, "ambiguous_block");
        assert_eq!(second.provider, "groq");
        assert_eq!(second.model, "llama-3.1-8b-instant");
    }
}
