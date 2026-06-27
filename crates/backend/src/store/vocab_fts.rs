//! BM25 keyword search over vocabulary terms — half of the hybrid retrieval.
//!
//! Vocabulary is exact-match-heavy (acronyms, brand names, code identifiers,
//! proper nouns). Pure dense retrieval (cosine on Gemini embeddings) misses
//! these because embeddings collapse rare tokens onto common neighbours.
//! BM25 catches them by keyword. We fuse the two ranks via RRF in the
//! polish-time selector — documented 15-30% recall improvement on
//! exact-match-critical corpora (Weaviate / OpenSearch hybrid blogs).
//!
//! Uses SQLite's built-in FTS5 (no new dependency). The virtual table is
//! "contentless" — we manage inserts/updates/deletes from Rust because
//! external-content FTS5 + ON-CONFLICT upserts in `vocabulary` is fragile.
//! The cost is one extra UPDATE per vocab write; the benefit is no trigger
//! cascade weirdness and total control over what gets indexed.

use rusqlite::params;
use tracing::warn;

use super::DbPool;

/// Insert or update the FTS row for a (user, term). Idempotent — safe to
/// call after every `vocabulary::upsert`. Replaces any prior FTS row for
/// the same (user_id, term) so example_context updates are reflected.
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
    let ctx = example_context.unwrap_or("").trim();

    // FTS5 contentless tables don't support ON CONFLICT; do delete + insert.
    let _ = conn.execute(
        "DELETE FROM vocab_fts WHERE user_id = ?1 AND term = ?2",
        params![user_id, term_trim],
    );
    if let Err(e) = conn.execute(
        "INSERT INTO vocab_fts (user_id, term, example_context)
         VALUES (?1, ?2, ?3)",
        params![user_id, term_trim, ctx],
    ) {
        warn!("[vocab-fts] insert failed: {e}");
    }
}

/// Delete the FTS row when a vocab term is removed. Cascade-safe.
pub fn delete(pool: &DbPool, user_id: &str, term: &str) {
    let Ok(conn) = pool.get() else {
        return;
    };
    let _ = conn.execute(
        "DELETE FROM vocab_fts WHERE user_id = ?1 AND term = ?2",
        params![user_id, term.trim()],
    );
}

/// BM25 search over (term, example_context). Returns the top-K matching
/// terms, ordered by BM25 ascending (lower score = better match in SQLite's
/// FTS5 BM25 implementation; the sort is handled by the prepared query).
///
/// `query` is treated as a free-text search — caller should not pre-quote
/// or pre-escape (we do safe quoting here). Empty queries return empty.
pub fn search(pool: &DbPool, user_id: &str, query: &str, k: usize) -> Vec<String> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let q = query.trim();
    if q.is_empty() {
        return vec![];
    }
    // FTS5 MATCH needs a token-style query. We escape by wrapping in double
    // quotes (FTS5 phrase syntax) and stripping any inner double-quotes —
    // this turns the entire query into one phrase, which is correct for
    // short user transcripts (we want any token in the transcript that
    // appears in a vocab term/context to score).
    //
    // Better approach: tokenise the transcript ourselves and OR the tokens.
    // That gives BM25 something to score per token. SQLite FTS5 supports
    // `OR` as a top-level connective.
    let tokens: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| s.len() >= 2)
        .map(|s| s.replace('"', ""))
        .map(|s| format!("\"{s}\""))
        .collect();
    if tokens.is_empty() {
        return vec![];
    }
    let match_clause = tokens.join(" OR ");

    let sql = "SELECT term FROM vocab_fts
                WHERE user_id = ?1 AND vocab_fts MATCH ?2
                ORDER BY bm25(vocab_fts)
                LIMIT ?3";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return vec![];
    };
    stmt.query_map(params![user_id, match_clause, k as i64], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .map(|iter| iter.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Rebuild FTS rows from the current vocabulary table. Safe to call at
/// startup: we delete+insert each term row so older users gain BM25 support
/// even if their vocabulary predates the FTS migration or missed a write.
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
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    drop(stmt);
    drop(conn);

    for (user_id, term, context) in &rows {
        upsert(pool, user_id, term, Some(context.as_str()));
    }
    rows.len()
}

/// Reciprocal Rank Fusion — combine multiple ranked lists into one score
/// per item, where items appearing high in any input list rank higher.
///
/// Score(item) = Σ over lists  1 / (k + rank_in_list_i)
///
/// `k=60` is the canonical default from the original RRF paper (Cormack
/// et al. 2009). It dampens the contribution of items appearing only deep
/// in one list. Returns items ordered by fused score descending.
///
/// Used by `vocab_embeddings::select_for_polish` to combine BM25 hits and
/// dense (cosine) hits into a single hybrid ranking.
pub fn rrf_fuse(rankings: &[&[String]], k: f32) -> Vec<String> {
    use std::collections::HashMap;
    let mut score: HashMap<&String, f32> = HashMap::new();
    for list in rankings {
        for (rank, item) in list.iter().enumerate() {
            let r = (rank + 1) as f32; // 1-indexed
            *score.entry(item).or_insert(0.0) += 1.0 / (k + r);
        }
    }
    let mut sorted: Vec<(&&String, f32)> = score.iter().map(|(k, v)| (k, *v)).collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.into_iter().map(|(k, _)| (*k).clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        pool.get()
            .unwrap()
            .execute_batch(
                "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE VIRTUAL TABLE vocab_fts USING fts5(
                 user_id UNINDEXED, term, example_context,
                 tokenize = 'unicode61 remove_diacritics 2'
             );",
            )
            .unwrap();
        pool
    }

    #[test]
    fn upsert_and_exact_term_search() {
        let pool = mem_pool();
        upsert(
            &pool,
            "u1",
            "MACOBS",
            Some("MACOBS ka IPO ka 12 hazaar batana"),
        );
        let hits = search(&pool, "u1", "MACOBS update", 5);
        assert!(hits.contains(&"MACOBS".to_string()));
    }

    #[test]
    fn search_finds_term_via_context_match() {
        let pool = mem_pool();
        upsert(&pool, "u1", "MACOBS", Some("MACOBS ka IPO ka 12 hazaar"));
        // Query mentions IPO (in the example context, not the term).
        let hits = search(&pool, "u1", "what is the IPO date?", 5);
        assert!(
            hits.contains(&"MACOBS".to_string()),
            "BM25 should find MACOBS via example_context match"
        );
    }

    #[test]
    fn search_user_scoping() {
        let pool = mem_pool();
        upsert(&pool, "u1", "FROM_U1", Some("u1's secret"));
        upsert(&pool, "u2", "FROM_U2", Some("u2's secret"));
        // Need to add u2 to local_user — extend the in-memory schema.
        let _ = pool
            .get()
            .unwrap()
            .execute("INSERT INTO local_user VALUES ('u2')", []);
        let u1_hits = search(&pool, "u1", "secret", 5);
        let u2_hits = search(&pool, "u2", "secret", 5);
        assert!(u1_hits.contains(&"FROM_U1".to_string()));
        assert!(
            !u1_hits.contains(&"FROM_U2".to_string()),
            "no cross-user leak"
        );
        assert!(u2_hits.contains(&"FROM_U2".to_string()));
    }

    #[test]
    fn search_returns_empty_for_empty_query() {
        let pool = mem_pool();
        upsert(&pool, "u1", "MACOBS", Some("..."));
        assert!(search(&pool, "u1", "", 5).is_empty());
        assert!(search(&pool, "u1", "   ", 5).is_empty());
    }

    #[test]
    fn delete_removes_from_index() {
        let pool = mem_pool();
        upsert(&pool, "u1", "TEMP", Some("placeholder"));
        assert!(!search(&pool, "u1", "placeholder", 5).is_empty());
        delete(&pool, "u1", "TEMP");
        assert!(search(&pool, "u1", "placeholder", 5).is_empty());
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let pool = mem_pool();
        upsert(&pool, "u1", "MACOBS", Some("first context with widgets"));
        upsert(&pool, "u1", "MACOBS", Some("second context with rockets"));
        // Old context shouldn't match anymore.
        assert!(search(&pool, "u1", "widgets", 5).is_empty());
        // New context should.
        assert!(!search(&pool, "u1", "rockets", 5).is_empty());
    }

    // ── RRF fusion tests ─────────────────────────────────────────────────────

    #[test]
    fn rrf_fuse_combines_two_lists() {
        let dense: Vec<String> = vec!["A".into(), "B".into(), "C".into()];
        let sparse: Vec<String> = vec!["B".into(), "D".into(), "A".into()];
        let fused = rrf_fuse(&[&dense, &sparse], 60.0);
        // B appears at rank 2 + 1 = best combined → top
        // A appears at rank 1 + 3
        // Both B (1/62 + 1/61) and A (1/61 + 1/63) — B should win since
        // it's at rank 1 in sparse and rank 2 in dense.
        assert_eq!(fused[0], "B");
    }

    #[test]
    fn rrf_fuse_handles_disjoint_lists() {
        let a: Vec<String> = vec!["X".into(), "Y".into()];
        let b: Vec<String> = vec!["Z".into()];
        let fused = rrf_fuse(&[&a, &b], 60.0);
        assert_eq!(fused.len(), 3);
        // X is rank 1 in list a → score 1/61
        // Z is rank 1 in list b → score 1/61
        // They tie; order is impl-defined but both must precede Y (rank 2 in a → 1/62)
        let last = fused.last().unwrap();
        assert_eq!(last, "Y");
    }

    #[test]
    fn rrf_fuse_empty_lists() {
        let empty: Vec<String> = vec![];
        let fused = rrf_fuse(&[&empty, &empty], 60.0);
        assert!(fused.is_empty());
    }
}
