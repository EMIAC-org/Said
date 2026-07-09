//! Full-card FTS index for vocabulary retrieval.
//!
//! `vocab_fts` indexes a compact card document, not just the canonical term.
//! Card text includes term, type, meaning, aliases, first example, and recent
//! support examples so the retriever can recall meaning-compatible candidates
//! without sending the full vocabulary to the polish model.

use rusqlite::{OptionalExtension, params};
use tracing::warn;

use super::DbPool;

/// Insert or update the FTS card for a term. The `example_context` argument is
/// kept for existing call sites; the function also reads current meaning,
/// aliases, and support examples from SQLite.
pub fn upsert(pool: &DbPool, user_id: &str, term: &str, example_context: Option<&str>) {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write("vocab_fts", "upsert", "vocab_fts::upsert");
        return;
    }
    let Ok(conn) = pool.get() else {
        return;
    };
    let term_trim = term.trim();
    if term_trim.is_empty() {
        return;
    }
    let card_text = build_card_text(&conn, user_id, term_trim, example_context);

    let _ = conn.execute(
        "DELETE FROM vocab_fts WHERE user_id = ?1 AND term = ?2",
        params![user_id, term_trim],
    );
    if let Err(e) = conn.execute(
        "INSERT INTO vocab_fts (user_id, term, card_text)
         VALUES (?1, ?2, ?3)",
        params![user_id, term_trim, card_text],
    ) {
        warn!("[vocab-fts] insert failed: {e}");
    }
}

pub fn delete(pool: &DbPool, user_id: &str, term: &str) {
    let Ok(conn) = pool.get() else {
        return;
    };
    let _ = conn.execute(
        "DELETE FROM vocab_fts WHERE user_id = ?1 AND term = ?2",
        params![user_id, term.trim()],
    );
}

/// Search the card index. Returns canonical terms ordered by BM25.
pub fn search(pool: &DbPool, user_id: &str, query: &str, k: usize) -> Vec<String> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let tokens: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && !is_query_stopword(s))
        .map(|s| s.replace('"', ""))
        .map(|s| format!("\"{s}\""))
        .take(24)
        .collect();
    if tokens.is_empty() {
        return vec![];
    }
    let match_clause = tokens.join(" OR ");
    let Ok(mut stmt) = conn.prepare(
        "SELECT term FROM vocab_fts
          WHERE user_id = ?1 AND vocab_fts MATCH ?2
          ORDER BY bm25(vocab_fts)
          LIMIT ?3",
    ) else {
        return vec![];
    };
    stmt.query_map(params![user_id, match_clause, k as i64], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .map(|iter| iter.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

pub fn backfill_from_vocabulary(pool: &DbPool) -> usize {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "vocab_fts",
            "backfill_from_vocabulary",
            "vocab_fts::backfill_from_vocabulary",
        );
        return 0;
    }
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT user_id, term, COALESCE(example_context, '')
           FROM vocabulary",
    ) else {
        return 0;
    };
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    drop(stmt);
    drop(conn);

    for (user_id, term, context) in &rows {
        upsert(pool, user_id, term, Some(context));
    }
    rows.len()
}

fn build_card_text(
    conn: &rusqlite::Connection,
    user_id: &str,
    term: &str,
    fallback_context: Option<&str>,
) -> String {
    let row = conn
        .query_row(
            "SELECT term_type, meaning, example_context
               FROM vocabulary
              WHERE user_id = ?1 AND LOWER(term) = LOWER(?2)
              LIMIT 1",
            params![user_id, term],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten();

    let (term_type, meaning, example_context) =
        row.unwrap_or((None, None, fallback_context.map(ToOwned::to_owned)));

    let aliases = load_aliases(conn, user_id, term);
    let examples = load_examples(conn, user_id, term);

    let mut parts = vec![term.to_string()];
    if let Some(term_type) = term_type.filter(|s| !s.trim().is_empty()) {
        parts.push(term_type);
    }
    if let Some(meaning) = meaning.filter(|s| !s.trim().is_empty()) {
        parts.push(meaning);
    }
    if let Some(context) = example_context
        .or_else(|| fallback_context.map(ToOwned::to_owned))
        .filter(|s| !s.trim().is_empty())
    {
        parts.push(context);
    }
    parts.extend(aliases);
    parts.extend(examples);

    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(1200)
        .collect()
}

fn load_aliases(conn: &rusqlite::Connection, user_id: &str, term: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT transcript_form
           FROM stt_replacements
          WHERE user_id = ?1
            AND LOWER(correct_form) = LOWER(?2)
            AND review_status = 'approved'
            AND export_tier <> 'blocked'
          ORDER BY use_count DESC, weight DESC, last_used DESC
          LIMIT 8",
    ) else {
        return vec![];
    };
    stmt.query_map(params![user_id, term], |row| row.get::<_, String>(0))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

fn load_examples(conn: &rusqlite::Connection, user_id: &str, term: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT example_text
           FROM vocab_embedding_examples
          WHERE user_id = ?1 AND term = ?2
          ORDER BY recorded_at DESC
          LIMIT 4",
    ) else {
        return vec![];
    };
    stmt.query_map(params![user_id, term], |row| row.get::<_, String>(0))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

fn is_query_stopword(token: &str) -> bool {
    matches!(
        token,
        "and"
            | "are"
            | "aur"
            | "hai"
            | "hain"
            | "kar"
            | "ke"
            | "kya"
            | "mein"
            | "main"
            | "the"
            | "this"
            | "that"
            | "with"
            | "you"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        crate::legacy_learning::enable_debug_legacy_writes_for_tests();
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        pool.get()
            .unwrap()
            .execute_batch(
                "CREATE TABLE local_user (id TEXT PRIMARY KEY);
                 INSERT INTO local_user(id) VALUES ('u1');
                 CREATE TABLE vocabulary (
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     weight REAL NOT NULL DEFAULT 1.0,
                     use_count INTEGER NOT NULL DEFAULT 1,
                     last_used INTEGER NOT NULL,
                     source TEXT NOT NULL DEFAULT 'auto',
                     language TEXT,
                     example_context TEXT,
                     term_type TEXT,
                     meaning TEXT,
                     UNIQUE(user_id, term)
                 );
                 CREATE TABLE stt_replacements (
                     user_id TEXT NOT NULL,
                     transcript_form TEXT NOT NULL,
                     correct_form TEXT NOT NULL,
                     phonetic_key TEXT NOT NULL DEFAULT '',
                     weight REAL NOT NULL DEFAULT 1.0,
                     use_count INTEGER NOT NULL DEFAULT 1,
                     last_used INTEGER NOT NULL DEFAULT 0,
                     language TEXT,
                     export_tier TEXT NOT NULL DEFAULT 'local_only',
                     contradiction_count INTEGER NOT NULL DEFAULT 0,
                     review_status TEXT NOT NULL DEFAULT 'approved',
                     review_reason TEXT,
                     last_reviewed_at INTEGER
                 );
                 CREATE TABLE vocab_embedding_examples (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     embedding BLOB NOT NULL,
                     example_text TEXT NOT NULL,
                     recorded_at INTEGER NOT NULL
                 );
                 CREATE VIRTUAL TABLE vocab_fts USING fts5(
                     user_id UNINDEXED,
                     term UNINDEXED,
                     card_text,
                     tokenize = 'unicode61 remove_diacritics 2'
                 );",
            )
            .unwrap();
        pool
    }

    #[test]
    fn search_finds_term_via_meaning_alias_and_example() {
        let pool = mem_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO vocabulary
                (user_id, term, weight, use_count, last_used, source, language, example_context, term_type, meaning)
             VALUES ('u1', 'MACOBS', 1.0, 1, 1, 'auto', 'hinglish', 'MACOBS onboarding flow', 'brand', 'internal onboarding product workflow')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stt_replacements
                (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used, export_tier, review_status)
             VALUES ('u1', 'main cops', 'MACOBS', 'mnkps', 1.0, 3, 1, 'export_replace_ready', 'approved')",
            [],
        )
        .unwrap();
        drop(conn);

        upsert(&pool, "u1", "MACOBS", Some("MACOBS onboarding flow"));

        assert!(search(&pool, "u1", "onboarding workflow", 5).contains(&"MACOBS".into()));
        assert!(search(&pool, "u1", "main cops", 5).contains(&"MACOBS".into()));
        assert!(search(&pool, "u1", "cosmetic shade party", 5).is_empty());
    }

    #[test]
    fn delete_removes_card() {
        let pool = mem_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary
                    (user_id, term, weight, use_count, last_used, source, meaning)
                 VALUES ('u1', 'n8n', 1.0, 1, 1, 'auto', 'workflow automation')",
                [],
            )
            .unwrap();
        upsert(&pool, "u1", "n8n", None);
        assert!(!search(&pool, "u1", "workflow automation", 5).is_empty());
        delete(&pool, "u1", "n8n");
        assert!(search(&pool, "u1", "workflow automation", 5).is_empty());
    }
}
