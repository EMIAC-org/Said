use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};

use super::{DbPool, now_ms};

pub fn normalize_email(email: &str) -> Option<String> {
    let cleaned = email
        .trim()
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | ')' | ']' | '}'))
        .trim_start_matches(|c: char| matches!(c, '(' | '[' | '{'))
        .to_string();
    let (local, domain) = cleaned.split_once('@')?;
    let local = local.trim_matches('.');
    let domain = domain.trim_matches('.').to_ascii_lowercase();
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || local.chars().any(char::is_whitespace)
        || domain.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(format!("{local}@{domain}"))
}

pub fn compact_email_norm(email: &str) -> Option<String> {
    normalize_email(email).map(|value| {
        value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    })
}

pub fn upsert(pool: &DbPool, user_id: &str, email: &str, source_hint: Option<&str>) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "email_memories",
            "upsert",
            "email_memory::upsert",
        );
        return false;
    }
    let Some(email) = normalize_email(email) else {
        return false;
    };
    let Some(email_norm) = compact_email_norm(&email) else {
        return false;
    };
    let source_norm = source_hint
        .map(crate::llm::alias_safety::normalize_source)
        .filter(|value| !value.is_empty());
    let Ok(conn) = pool.get() else {
        return false;
    };
    let existed = conn
        .query_row(
            "SELECT 1 FROM email_memories WHERE user_id = ?1 AND email_norm = ?2",
            params![user_id, email_norm],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    let now = now_ms();
    let written = conn.execute(
        "INSERT INTO email_memories
             (user_id, email, email_norm, source_hint, source_norm, positive_count, first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
         ON CONFLICT(user_id, email_norm) DO UPDATE SET
             email = excluded.email,
             source_hint = COALESCE(excluded.source_hint, email_memories.source_hint),
             source_norm = COALESCE(excluded.source_norm, email_memories.source_norm),
             positive_count = positive_count + 1,
             last_seen = excluded.last_seen",
        params![user_id, email, email_norm, source_hint, source_norm, now],
    );
    written.is_ok() && !existed
}

pub fn upsert_many_from_text(
    pool: &DbPool,
    user_id: &str,
    text: &str,
    source_hint: Option<&str>,
) -> Vec<String> {
    let mut learned = Vec::new();
    let mut seen = HashSet::new();
    for email in crate::llm::format_recover::extract_emails(text) {
        let Some(canonical) = normalize_email(&email) else {
            continue;
        };
        let Some(norm) = compact_email_norm(&canonical) else {
            continue;
        };
        if !seen.insert(norm) {
            continue;
        }
        if upsert(pool, user_id, &canonical, source_hint) {
            learned.push(canonical);
        }
    }
    learned
}

pub fn load_candidates(pool: &DbPool, user_id: &str) -> Vec<String> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(email) = conn
        .query_row(
            "SELECT email FROM local_user WHERE id = ?1",
            params![user_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|email| normalize_email(&email))
        .filter(|email| email != "local@voicepolish.app")
    {
        if let Some(norm) = compact_email_norm(&email) {
            if seen.insert(norm) {
                out.push(email);
            }
        }
    }

    let Ok(mut stmt) = conn.prepare(
        "SELECT email
           FROM email_memories
          WHERE user_id = ?1
          ORDER BY positive_count DESC, last_seen DESC
          LIMIT 50",
    ) else {
        return out;
    };
    let rows = stmt
        .query_map(params![user_id], |row| row.get::<_, String>(0))
        .ok();
    if let Some(rows) = rows {
        for row in rows.flatten() {
            if let Some(email) = normalize_email(&row) {
                if let Some(norm) = compact_email_norm(&email) {
                    if seen.insert(norm) {
                        out.push(email);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        crate::legacy_learning::enable_debug_legacy_writes_for_tests();
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder().max_size(1).build(manager).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY, email TEXT);
             INSERT INTO local_user(id, email) VALUES ('u1', 'vabhi.verma2678@gmail.com');
             CREATE TABLE email_memories (
                 user_id TEXT NOT NULL,
                 email TEXT NOT NULL,
                 email_norm TEXT NOT NULL,
                 source_hint TEXT,
                 source_norm TEXT,
                 positive_count INTEGER NOT NULL DEFAULT 1,
                 first_seen INTEGER NOT NULL,
                 last_seen INTEGER NOT NULL,
                 PRIMARY KEY (user_id, email_norm)
             );",
        )
        .unwrap();
        drop(conn);
        pool
    }

    #[test]
    fn loads_user_email_and_learned_emails_without_duplicates() {
        let pool = mem_pool();
        assert!(upsert(
            &pool,
            "u1",
            "support@airnote.app",
            Some("support at airnote dot app")
        ));
        assert!(!upsert(
            &pool,
            "u1",
            "support@airnote.app",
            Some("support at airnote dot app")
        ));
        let candidates = load_candidates(&pool, "u1");
        assert!(candidates.contains(&"vabhi.verma2678@gmail.com".to_string()));
        assert!(candidates.contains(&"support@airnote.app".to_string()));
    }
}
