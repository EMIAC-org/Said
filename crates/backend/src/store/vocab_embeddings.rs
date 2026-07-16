//! Vocabulary embedding storage.
//!
//! This module intentionally does not choose prompt vocabulary. It stores the
//! per-term example ring and centroid used by the meaning-first retriever in
//! `llm::vocab_retrieval`.

use rusqlite::params;
use tracing::{info, warn};

use super::{DbPool, now_ms};
use crate::embedder::gemini::{blob_to_floats, floats_to_blob};

const EXAMPLES_RING_SIZE: usize = 10;

/// Insert or replace the centroid embedding for one vocabulary term.
///
/// Kept for backfills and tests that already have a centroid. New learning
/// writes should prefer `record_example_and_recentre`.
pub fn upsert_embedding(pool: &DbPool, user_id: &str, term: &str, embedding: &[f32]) {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "vocab_embeddings",
            "upsert_embedding",
            "vocab_embeddings::upsert_embedding",
        );
        return;
    }
    let Ok(conn) = pool.get() else {
        warn!("[vocab-emb] pool error while upserting centroid");
        return;
    };
    write_centroid(&conn, user_id, term.trim(), embedding);
}

/// Append one observed example and recompute the term centroid from the live
/// FIFO ring. The hot dictation path reads this centroid only from cache; this
/// write path is background/learning maintenance.
pub fn record_example_and_recentre(
    pool: &DbPool,
    user_id: &str,
    term: &str,
    embedding: &[f32],
    example_text: &str,
) {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "vocab_embeddings",
            "record_example_and_recentre",
            "vocab_embeddings::record_example_and_recentre",
        );
        return;
    }
    let Ok(conn) = pool.get() else {
        warn!("[vocab-emb] pool error while recording example");
        return;
    };
    let term_trim = term.trim();
    if term_trim.is_empty() || embedding.is_empty() || example_text.trim().is_empty() {
        return;
    }

    let now = now_ms();
    let blob = floats_to_blob(embedding);
    if let Err(e) = conn.execute(
        "INSERT INTO vocab_embedding_examples
            (user_id, term, embedding, example_text, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, term_trim, blob, example_text.trim(), now],
    ) {
        warn!("[vocab-emb] insert example failed: {e}");
        return;
    }

    let _ = conn.execute(
        "DELETE FROM vocab_embedding_examples
          WHERE id IN (
            SELECT id FROM vocab_embedding_examples
             WHERE user_id = ?1 AND term = ?2
             ORDER BY recorded_at DESC
             LIMIT -1 OFFSET ?3
          )",
        params![user_id, term_trim, EXAMPLES_RING_SIZE as i64],
    );

    let examples = load_example_embeddings(&conn, user_id, term_trim);
    if examples.is_empty() {
        return;
    }
    let centroid = mean_normalised(&examples);
    write_centroid(&conn, user_id, term_trim, &centroid);
    info!(
        "[vocab-emb] centroid({term_trim:?}) recomputed from {} example(s)",
        examples.len(),
    );
}

/// Variance of a term's example cloud. High values indicate the term may be
/// overloaded across distinct meanings and should be reviewed/split later.
pub fn cluster_spread(pool: &DbPool, user_id: &str, term: &str) -> f32 {
    let Ok(conn) = pool.get() else {
        return 0.0;
    };
    let examples = load_example_embeddings(&conn, user_id, term.trim());
    if examples.len() < 2 {
        return 0.0;
    }
    let centroid = mean_normalised(&examples);
    let cn = l2_norm(&centroid);
    if cn == 0.0 {
        return 0.0;
    }
    let mean_sim: f32 = examples
        .iter()
        .map(|example| {
            let en = l2_norm(example);
            if en == 0.0 {
                0.0
            } else {
                dot(example, &centroid) / (en * cn)
            }
        })
        .sum::<f32>()
        / examples.len() as f32;
    (1.0 - mean_sim).max(0.0)
}

/// Load recent example texts for a term, newest first.
pub fn recent_example_texts(pool: &DbPool, user_id: &str, term: &str, limit: usize) -> Vec<String> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT example_text FROM vocab_embedding_examples
          WHERE user_id = ?1 AND term = ?2
          ORDER BY recorded_at DESC
          LIMIT ?3",
    ) else {
        return vec![];
    };
    stmt.query_map(params![user_id, term.trim(), limit as i64], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Curate a small, stable support set for meaning generation.
pub fn support_example_texts(
    pool: &DbPool,
    user_id: &str,
    term: &str,
    limit: usize,
) -> Vec<String> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT example_text, recorded_at FROM vocab_embedding_examples
          WHERE user_id = ?1 AND term = ?2
          ORDER BY recorded_at ASC",
    ) else {
        return vec![];
    };
    let rows: Vec<(String, i64)> = stmt
        .query_map(params![user_id, term.trim()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .ok()
        .map(|iter| iter.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    if rows.is_empty() {
        return vec![];
    }
    if rows.len() == 1 || limit <= 1 {
        return vec![rows[0].0.clone()];
    }

    let anchor_score = |text: &str| -> usize {
        text.split_whitespace()
            .filter(|tok| {
                let trimmed = tok.trim_matches(|c: char| !c.is_alphanumeric());
                trimmed.chars().any(|c| c.is_ascii_digit()) || trimmed.chars().count() >= 4
            })
            .count()
    };

    let mut chosen = Vec::new();
    let push_unique = |chosen: &mut Vec<String>, candidate: &str| {
        if !candidate.trim().is_empty() && !chosen.iter().any(|s| s == candidate) {
            chosen.push(candidate.to_string());
        }
    };

    if let Some((text, _)) = rows
        .iter()
        .max_by_key(|(text, recorded_at)| (anchor_score(text), std::cmp::Reverse(*recorded_at)))
    {
        push_unique(&mut chosen, text);
    }
    if let Some((text, _)) = rows.last() {
        push_unique(&mut chosen, text);
    }
    if let Some((text, _)) = rows.iter().max_by_key(|(text, _)| anchor_score(text)) {
        push_unique(&mut chosen, text);
    }

    let seed = chosen.first().cloned().unwrap_or_else(|| rows[0].0.clone());
    let seed_tokens: std::collections::HashSet<String> = tokenize_for_diversity(&seed);
    if let Some((text, _)) = rows.iter().max_by_key(|(text, _)| {
        let tokens = tokenize_for_diversity(text);
        let overlap = tokens.intersection(&seed_tokens).count();
        let novelty = tokens.len().saturating_sub(overlap);
        (novelty, anchor_score(text))
    }) {
        push_unique(&mut chosen, text);
    }

    chosen.truncate(limit.max(1));
    chosen
}

/// Bump positive use signals for cards that were actually sent to the prompt.
pub fn bump_last_used(pool: &DbPool, user_id: &str, terms: &[String]) {
    if terms.is_empty() {
        return;
    }
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "vocab_embeddings",
            "bump_last_used",
            "vocab_embeddings::bump_last_used",
        );
        return;
    }
    let Ok(conn) = pool.get() else {
        return;
    };
    let now = now_ms();
    let Ok(mut stmt) = conn.prepare(
        "UPDATE vocabulary
            SET last_used = ?3,
                use_count = use_count + 1
          WHERE user_id = ?1 AND term = ?2",
    ) else {
        return;
    };
    for term in terms {
        let _ = stmt.execute(params![user_id, term.trim(), now]);
    }
}

pub fn has_centroid(pool: &DbPool, user_id: &str, term: &str) -> bool {
    let Ok(conn) = pool.get() else {
        return false;
    };
    conn.query_row(
        "SELECT 1 FROM vocab_embeddings WHERE user_id = ?1 AND term = ?2 LIMIT 1",
        params![user_id, term.trim()],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn has_example_ring(pool: &DbPool, user_id: &str, term: &str) -> bool {
    let Ok(conn) = pool.get() else {
        return false;
    };
    conn.query_row(
        "SELECT 1 FROM vocab_embedding_examples
          WHERE user_id = ?1 AND term = ?2
          LIMIT 1",
        params![user_id, term.trim()],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn rebuild_centroid_from_examples(pool: &DbPool, user_id: &str, term: &str) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "vocab_embeddings",
            "rebuild_centroid_from_examples",
            "vocab_embeddings::rebuild_centroid_from_examples",
        );
        return false;
    }
    let Ok(conn) = pool.get() else {
        return false;
    };
    let examples = load_example_embeddings(&conn, user_id, term.trim());
    if examples.is_empty() {
        return false;
    }
    let centroid = mean_normalised(&examples);
    write_centroid(&conn, user_id, term.trim(), &centroid);
    true
}

/// Remove a term's centroid and example ring.
pub fn delete(pool: &DbPool, user_id: &str, term: &str) {
    let Ok(conn) = pool.get() else {
        return;
    };
    let term_trim = term.trim();
    let _ = conn.execute(
        "DELETE FROM vocab_embeddings WHERE user_id = ?1 AND term = ?2",
        params![user_id, term_trim],
    );
    let _ = conn.execute(
        "DELETE FROM vocab_embedding_examples WHERE user_id = ?1 AND term = ?2",
        params![user_id, term_trim],
    );
}

fn write_centroid(conn: &rusqlite::Connection, user_id: &str, term: &str, centroid: &[f32]) {
    if term.is_empty() || centroid.is_empty() {
        return;
    }
    let blob = floats_to_blob(centroid);
    let now = now_ms();
    let _ = conn.execute(
        "INSERT INTO vocab_embeddings (user_id, term, embedding, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, term) DO UPDATE SET
            embedding  = excluded.embedding,
            updated_at = excluded.updated_at",
        params![user_id, term, blob, now],
    );
}

fn load_example_embeddings(
    conn: &rusqlite::Connection,
    user_id: &str,
    term: &str,
) -> Vec<Vec<f32>> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT embedding FROM vocab_embedding_examples
          WHERE user_id = ?1 AND term = ?2
          ORDER BY recorded_at DESC",
    ) else {
        return vec![];
    };
    stmt.query_map(params![user_id, term], |row| row.get::<_, Vec<u8>>(0))
        .ok()
        .map(|iter| {
            iter.filter_map(|r| r.ok())
                .filter_map(|blob| blob_to_floats(&blob))
                .collect()
        })
        .unwrap_or_default()
}

fn mean_normalised(vectors: &[Vec<f32>]) -> Vec<f32> {
    let Some(first) = vectors.first() else {
        return vec![];
    };
    let dim = first.len();
    if dim == 0 {
        return vec![];
    }
    let mut sum = vec![0.0_f32; dim];
    let mut kept = 0usize;
    for vector in vectors {
        if vector.len() != dim {
            continue;
        }
        kept += 1;
        for (slot, value) in sum.iter_mut().zip(vector) {
            *slot += *value;
        }
    }
    if kept == 0 {
        return vec![];
    }
    let inv = 1.0 / kept as f32;
    for value in &mut sum {
        *value *= inv;
    }
    let norm = l2_norm(&sum);
    if norm > 0.0 {
        for value in &mut sum {
            *value /= norm;
        }
    }
    sum
}

fn tokenize_for_diversity(text: &str) -> std::collections::HashSet<String> {
    text.split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[inline]
fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
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
                     use_count INTEGER NOT NULL DEFAULT 1,
                     last_used INTEGER NOT NULL,
                     UNIQUE(user_id, term)
                 );
                 CREATE TABLE vocab_embeddings (
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     embedding BLOB NOT NULL,
                     updated_at INTEGER NOT NULL,
                     UNIQUE(user_id, term)
                 );
                 CREATE TABLE vocab_embedding_examples (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     embedding BLOB NOT NULL,
                     example_text TEXT NOT NULL,
                     recorded_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
        pool
    }

    #[test]
    fn record_example_rebuilds_centroid_and_keeps_examples() {
        let pool = mem_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary(user_id, term, last_used) VALUES ('u1', 'MACOBS', ?1)",
                params![now_ms()],
            )
            .unwrap();

        record_example_and_recentre(&pool, "u1", "MACOBS", &[1.0, 0.0], "MACOBS onboarding");
        record_example_and_recentre(&pool, "u1", "MACOBS", &[0.0, 1.0], "Macobs rollout");

        assert!(has_centroid(&pool, "u1", "MACOBS"));
        assert!(has_example_ring(&pool, "u1", "MACOBS"));
        assert_eq!(recent_example_texts(&pool, "u1", "MACOBS", 5).len(), 2);
        assert!(cluster_spread(&pool, "u1", "MACOBS") > 0.0);
    }

    #[test]
    fn support_examples_are_bounded_and_nonempty() {
        let pool = mem_pool();
        for idx in 0..5 {
            record_example_and_recentre(
                &pool,
                "u1",
                "n8n",
                &[1.0, idx as f32],
                &format!("n8n workflow example {idx}"),
            );
        }
        let support = support_example_texts(&pool, "u1", "n8n", 3);
        assert!(!support.is_empty());
        assert!(support.len() <= 3);
    }
}
