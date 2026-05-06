//! Preference vector store — insert + cosine-similarity KNN.
//!
//! Embeddings are stored in `preference_vectors` as 768×f32 little-endian BLOBs.
//! KNN is computed entirely in Rust (dot-product after L2-normalisation).
//! At personal scale (< 1 000 vectors) this is effectively instant.

use rusqlite::params;
use sha2::{Digest, Sha256};
use tracing::warn;

use super::DbPool;
use crate::{
    embedder::gemini::{blob_to_floats, floats_to_blob},
    llm::prompt::RagExample,
};

/// A row fetched from `preference_vectors` + its linked edit event.
struct VectorRow {
    embedding: Vec<f32>,
    ai_output: String,
    user_kept: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningKind {
    Stt,
    Polish,
    Rewrite,
    Style,
}

impl LearningKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::Polish => "polish",
            Self::Rewrite => "rewrite",
            Self::Style => "style",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorQuality {
    Normal,
    LowInfo,
}

impl VectorQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::LowInfo => "low_info",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LearningMeta {
    pub learning_kind: LearningKind,
    pub text_fingerprint: String,
    pub vector_quality: VectorQuality,
    pub should_embed: bool,
}

/// Insert (or replace) a preference vector for an edit event.
pub fn upsert_vector(pool: &DbPool, user_id: &str, edit_event_id: &str, embedding: &[f32]) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[vectors] pool error: {e}");
            return;
        }
    };

    let blob = floats_to_blob(embedding);
    let _ = conn.execute(
        "INSERT OR REPLACE INTO preference_vectors (user_id, edit_event_id, embedding)
         VALUES (?1, ?2, ?3)",
        params![user_id, edit_event_id, blob],
    );
}

/// Retrieve the top-`k` most similar past edit examples for `query_embedding`.
///
/// Returns at most `k` examples where cosine similarity exceeds `min_sim`.
pub fn retrieve_similar(
    pool: &DbPool,
    user_id: &str,
    query: &[f32],
    k: usize,
    min_sim: f32,
) -> Vec<RagExample> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    // Pull all vectors for this user along with their edit text
    let mut stmt = match conn.prepare(
        "SELECT pv.embedding, ee.ai_output, ee.user_kept
         FROM preference_vectors pv
         JOIN edit_events ee ON ee.id = pv.edit_event_id
         WHERE pv.user_id = ?1",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let rows: Vec<VectorRow> = stmt
        .query_map(params![user_id], |row| {
            let blob: Vec<u8> = row.get(0)?;
            let ai_output: String = row.get(1)?;
            let user_kept: String = row.get(2)?;
            Ok((blob, ai_output, user_kept))
        })
        .ok()
        .map(|iter| {
            iter.filter_map(|r| r.ok())
                .filter_map(|(blob, ai, kept)| {
                    blob_to_floats(&blob).map(|embedding| VectorRow {
                        embedding,
                        ai_output: ai,
                        user_kept: kept,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if rows.is_empty() {
        return vec![];
    }

    // Compute cosine similarity for each row, keep top-k above threshold
    let query_norm = l2_norm(query);
    if query_norm == 0.0 {
        return vec![];
    }

    let mut scored: Vec<(f32, &VectorRow)> = rows
        .iter()
        .filter_map(|row| {
            let row_norm = l2_norm(&row.embedding);
            if row_norm == 0.0 {
                return None;
            }
            let sim = dot(&row.embedding, query) / (row_norm * query_norm);
            if sim >= min_sim {
                Some((sim, row))
            } else {
                None
            }
        })
        .collect();

    // Sort descending by similarity
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);

    scored
        .into_iter()
        .map(|(_, row)| RagExample {
            ai_output: row.ai_output.clone(),
            user_kept: row.user_kept.clone(),
        })
        .collect()
}

/// Insert a new edit_event row. Returns the generated UUID.
pub fn insert_edit_event(
    pool: &DbPool,
    user_id: &str,
    recording_id: Option<&str>,
    transcript: &str,
    ai_output: &str,
    user_kept: &str,
    target_app: Option<&str>,
) -> Option<String> {
    let conn = pool.get().ok()?;
    let now_ms = super::now_ms();
    let meta = derive_learning_meta(transcript, ai_output, user_kept);

    if let Some(existing_id) = conn
        .query_row(
            "SELECT id FROM edit_events
              WHERE user_id = ?1
                AND text_fingerprint = ?2
                AND timestamp_ms >= ?3
              ORDER BY timestamp_ms DESC
              LIMIT 1",
            params![user_id, meta.text_fingerprint, now_ms - 86_400_000i64],
            |row| row.get::<_, String>(0),
        )
        .ok()
    {
        return Some(existing_id);
    }

    let id = uuid::Uuid::new_v4().to_string();

    conn.execute(
        "INSERT INTO edit_events
         (id, user_id, recording_id, timestamp_ms, transcript, ai_output, user_kept, target_app,
          learning_kind, text_fingerprint, vector_quality)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            user_id,
            recording_id,
            now_ms,
            transcript,
            ai_output,
            user_kept,
            target_app,
            meta.learning_kind.as_str(),
            meta.text_fingerprint,
            meta.vector_quality.as_str(),
        ],
    )
    .ok()?;

    Some(id)
}

pub fn should_embed_event(pool: &DbPool, edit_event_id: &str) -> bool {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    conn.query_row(
        "SELECT vector_quality FROM edit_events WHERE id = ?1",
        params![edit_event_id],
        |row| row.get::<_, String>(0),
    )
    .map(|quality| quality != VectorQuality::LowInfo.as_str())
    .unwrap_or(false)
}

pub fn derive_learning_meta(transcript: &str, ai_output: &str, user_kept: &str) -> LearningMeta {
    let norm_transcript = normalize_text(transcript);
    let norm_ai = normalize_text(ai_output);
    let norm_kept = normalize_text(user_kept);

    let learning_kind =
        if !norm_transcript.is_empty() && norm_kept == norm_transcript && norm_ai != norm_kept {
            LearningKind::Polish
        } else if norm_ai.is_empty() || norm_kept.is_empty() {
            LearningKind::Rewrite
        } else {
            let ai_tokens = token_set(&norm_ai);
            let kept_tokens = token_set(&norm_kept);
            let overlap = ai_tokens.intersection(&kept_tokens).count();
            let max_len = ai_tokens.len().max(kept_tokens.len()).max(1);
            let overlap_ratio = overlap as f64 / max_len as f64;
            if overlap_ratio < 0.35 {
                LearningKind::Rewrite
            } else if looks_jargon_shift(&norm_ai, &norm_kept) {
                LearningKind::Stt
            } else {
                LearningKind::Style
            }
        };

    let low_info = norm_ai == norm_kept
        || token_set(&norm_ai) == token_set(&norm_kept)
        || (!norm_ai.is_empty()
            && !norm_kept.is_empty()
            && token_set(&norm_ai)
                .symmetric_difference(&token_set(&norm_kept))
                .count()
                <= 1
            && (norm_ai.len() as i64 - norm_kept.len() as i64).abs() <= 2);

    let fingerprint_src = format!(
        "{}\n{}\n{}\n{}",
        learning_kind.as_str(),
        norm_transcript,
        norm_ai,
        norm_kept
    );
    let fingerprint = hex::encode(Sha256::digest(fingerprint_src.as_bytes()));

    LearningMeta {
        learning_kind,
        text_fingerprint: fingerprint,
        vector_quality: if low_info {
            VectorQuality::LowInfo
        } else {
            VectorQuality::Normal
        },
        should_embed: !low_info,
    }
}

// ── Math helpers ──────────────────────────────────────────────────────────────

fn normalize_text(text: &str) -> String {
    text.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn token_set(text: &str) -> std::collections::HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn looks_jargon_shift(ai_output: &str, user_kept: &str) -> bool {
    let ai = token_set(ai_output);
    let kept = token_set(user_kept);
    kept.iter()
        .any(|tok| crate::llm::phonetics::jargon_score(tok) >= 0.4 && !ai.contains(tok))
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
    use crate::store::DbPool;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE edit_events (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                recording_id TEXT,
                timestamp_ms INTEGER NOT NULL,
                transcript TEXT NOT NULL,
                ai_output TEXT NOT NULL,
                user_kept TEXT NOT NULL,
                target_app TEXT,
                learning_kind TEXT,
                text_fingerprint TEXT,
                vector_quality TEXT NOT NULL DEFAULT 'normal'
            );
            CREATE TABLE preference_vectors (
                user_id TEXT NOT NULL,
                edit_event_id TEXT NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY (user_id, edit_event_id)
            );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn low_info_events_are_deduped_and_skip_embedding() {
        let pool = mem_pool();
        let first = insert_edit_event(
            &pool,
            "u1",
            None,
            "Zara batana RAG",
            "Zara batana RAG",
            "Zara batana rag",
            None,
        )
        .unwrap();
        assert!(!should_embed_event(&pool, &first));

        let second = insert_edit_event(
            &pool,
            "u1",
            None,
            "Zara batana RAG",
            "Zara batana RAG",
            "Zara batana rag",
            None,
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn jargon_shift_is_classified_as_stt_learning() {
        let meta = derive_learning_meta(
            "zara batana return ka automation",
            "Zara batana return ka automation",
            "Zara batana n8n ka automation",
        );
        assert_eq!(meta.learning_kind, LearningKind::Stt);
        assert!(meta.should_embed);
        assert_eq!(meta.vector_quality, VectorQuality::Normal);
    }
}
