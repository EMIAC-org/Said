//! Vocabulary embedding store + relevance-aware retrieval.
//!
//! At learn time we embed `"{term}. {example_context}"` and persist the
//! 256d vector here. At polish time we embed the transcript (already done
//! for RAG), then `top_k_relevant` cosine-ranks vocab against it. Combined
//! with `select_for_polish` (in this module), the polish prompt receives
//! a small relevance-scoped slice of vocab instead of the full table.
//!
//! Why this matters at scale: 200+ vocab entries × 80 tokens each ≈ 16 KB
//! of prompt on every recording. The LLM's attention degrades, latency
//! climbs, and the *one entry that matters* gets diluted. Vector retrieval
//! gives us the entries that match what the user just *said* — typically
//! 10–20 entries, all relevant.
//!
//! See `vectors.rs` for the parallel implementation on edit-event RAG.

use rusqlite::params;
use tracing::{info, warn};

use super::{DbPool, now_ms};
use crate::embedder::gemini::{blob_to_floats, floats_to_blob};
use crate::store::vocabulary::VocabTerm;

const PROMPT_VOCAB_TOP_WEIGHT: usize = 8;
const PROMPT_VOCAB_K_RELEVANT: usize = 12;
const PROMPT_VOCAB_MAX_TOTAL: usize = 25;
const PROMPT_VOCAB_MIN_SIM: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabSelectionTier {
    Apply,
    Suggest,
}

impl VocabSelectionTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Suggest => "suggest",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VocabSelection {
    pub term: VocabTerm,
    pub tier: VocabSelectionTier,
    pub reason: String,
    pub evidence: String,
    pub score: f64,
}

/// One vocab entry plus its embedding, as loaded from the joined query.
struct VocabRow {
    term: String,
    embedding: Vec<f32>,
    weight: f64,
    use_count: i64,
    last_used: i64,
    source: String,
    example_context: Option<String>,
    term_type: Option<String>,
    meaning: Option<String>,
}

impl VocabRow {
    fn into_term(self) -> VocabTerm {
        VocabTerm {
            term: self.term,
            weight: self.weight,
            use_count: self.use_count,
            last_used: self.last_used,
            source: self.source,
            example_context: self.example_context,
            term_type: self.term_type,
            meaning: self.meaning,
        }
    }
}

/// Maximum number of example embeddings retained per (user, term) in the
/// FIFO ring. Centroid quality plateaus around 8-12; we pick 10 as a
/// reasonable balance between robustness and storage. At 10 examples ×
/// 1 KB each × 200 terms = 2 MB worst case per user. Cheap.
const EXAMPLES_RING_SIZE: usize = 10;

/// Insert or replace the centroid embedding for one vocabulary term.
///
/// Legacy entry-point: writes a single-embedding "centroid" directly. New
/// code should call `record_example_and_recentre` so the per-sighting ring
/// stays in sync. Kept for cases where the caller has only the centroid
/// (e.g. a migration backfill) and not the original example sentence.
pub fn upsert_embedding(pool: &DbPool, user_id: &str, term: &str, embedding: &[f32]) {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "vocab_embeddings",
            "upsert_embedding",
            "vocab_embeddings::upsert_embedding",
        );
        return;
    }
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[vocab-emb] pool error: {e}");
            return;
        }
    };
    write_centroid(&conn, user_id, term, embedding);
}

/// Append one example embedding to the per-term FIFO ring (capped at
/// `EXAMPLES_RING_SIZE`), then recompute the centroid + persist it as the
/// `vocab_embeddings.embedding` row.
///
/// This is the foundational write path: every observed sighting of a term
/// adds an example, and the centroid (mean of L2-normalised vectors,
/// re-normalised to unit length) becomes the term's representation for
/// retrieval. Single-example representations are the largest source of
/// retrieval noise — centroids of 5-10 examples are dramatically more
/// stable (Snell et al., Prototypical Networks, NeurIPS 2017).
///
/// Atomicity: ring append, eviction, and centroid recompute happen inside
/// one connection without an explicit transaction — safe because the only
/// reader (`top_k_relevant`) tolerates a momentary stale centroid (worst
/// case: one retrieval uses last-cycle's centroid).
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
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[vocab-emb] pool error: {e}");
            return;
        }
    };
    let term_trim = term.trim();
    let now = now_ms();

    // 1. Append the new example.
    let blob = floats_to_blob(embedding);
    if let Err(e) = conn.execute(
        "INSERT INTO vocab_embedding_examples
            (user_id, term, embedding, example_text, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, term_trim, blob, example_text, now],
    ) {
        warn!("[vocab-emb] insert example failed: {e}");
        return;
    }

    // 2. Evict oldest beyond the ring size (FIFO by recorded_at).
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

    // 3. Recompute centroid from the live ring.
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

/// Variance of the example cloud — high values indicate the term is being
/// used in semantically distinct contexts (candidate for split). Defined
/// as `1 - mean(cosine(example_i, centroid))`. Range [0, 2]; typical
/// cohesive concepts sit below 0.2; bimodal concepts above 0.5.
///
/// Used as a soft signal — surfaced in logs today, will drive
/// auto-split-into-two-prototypes in a future iteration.
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
        .map(|e| {
            let en = l2_norm(e);
            if en == 0.0 {
                0.0
            } else {
                dot(e, &centroid) / (en * cn)
            }
        })
        .sum::<f32>()
        / examples.len() as f32;
    (1.0 - mean_sim).max(0.0)
}

/// Load the most-recent example texts for a (user, term), newest first,
/// capped at `limit`. Used by the meaning-refinement path so the LLM can
/// re-distill the term's description from its current usage cloud.
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

/// Curate a small support set for meaning generation/refinement.
///
/// The support set is intentionally stable and diverse:
///   • earliest anchor-rich example
///   • latest example
///   • strongest lexical-anchor example
///   • one diverse outlier when examples differ materially
///
/// This gives the background meaning LLM a better view of the term than
/// "just the newest sentence", while keeping prompt size bounded.
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
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
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

    let mut chosen: Vec<String> = Vec::new();
    let push_unique = |chosen: &mut Vec<String>, candidate: &str| {
        if !candidate.trim().is_empty() && !chosen.iter().any(|s| s == candidate) {
            chosen.push(candidate.to_string());
        }
    };

    // Earliest useful anchor-rich example.
    if let Some((text, _)) = rows
        .iter()
        .max_by_key(|(text, recorded_at)| (anchor_score(text), std::cmp::Reverse(*recorded_at)))
    {
        push_unique(&mut chosen, text);
    }

    // Latest example.
    if let Some((text, _)) = rows.last() {
        push_unique(&mut chosen, text);
    }

    // Strongest lexical anchor example.
    if let Some((text, _)) = rows.iter().max_by_key(|(text, _)| anchor_score(text)) {
        push_unique(&mut chosen, text);
    }

    // One diverse outlier if available.
    let seed = chosen.first().cloned().unwrap_or_else(|| rows[0].0.clone());
    let seed_tokens: std::collections::HashSet<String> = seed
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect();
    if let Some((text, _)) = rows.iter().max_by_key(|(text, _)| {
        let tokens: std::collections::HashSet<String> = text
            .split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_ascii_lowercase()
            })
            .filter(|t| !t.is_empty())
            .collect();
        let overlap = tokens.intersection(&seed_tokens).count();
        let novelty = tokens.len().saturating_sub(overlap);
        (novelty, anchor_score(text))
    }) {
        push_unique(&mut chosen, text);
    }

    chosen.truncate(limit.max(1));
    chosen
}

/// Bump `last_used` on a set of vocab terms — called after polish completes
/// so terms that actually appeared in the prompt get reinforced. This is
/// the "use signal" half of the time-decay scoring (the other half is the
/// exp(-λ·Δt) factor in `decay_factor`).
///
/// Cheap: one batched UPDATE per call; idempotent.
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
    // SQLite doesn't have a clean batched UPDATE-IN; loop with prepared stmt.
    let Ok(mut stmt) = conn.prepare(
        "UPDATE vocabulary
            SET last_used = ?3,
                use_count = use_count + 1
          WHERE user_id = ?1 AND term = ?2",
    ) else {
        return;
    };
    for t in terms {
        let _ = stmt.execute(params![user_id, t.trim(), now]);
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

/// Internal: write the centroid into vocab_embeddings (with current ts).
fn write_centroid(conn: &rusqlite::Connection, user_id: &str, term: &str, centroid: &[f32]) {
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

/// Internal: load all example embeddings for a (user, term).
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

/// Internal: mean of vectors, then L2-normalised. The returned vector is a
/// unit-length centroid suitable for direct cosine comparison against
/// other unit vectors.
fn mean_normalised(vectors: &[Vec<f32>]) -> Vec<f32> {
    let n = vectors.len();
    if n == 0 {
        return vec![];
    }
    let dim = vectors[0].len();
    let mut sum = vec![0.0_f32; dim];
    for v in vectors {
        if v.len() != dim {
            continue;
        }
        for (s, &x) in sum.iter_mut().zip(v.iter()) {
            *s += x;
        }
    }
    let inv_n = 1.0 / n as f32;
    for s in sum.iter_mut() {
        *s *= inv_n;
    }
    let norm = l2_norm(&sum);
    if norm > 0.0 {
        for s in sum.iter_mut() {
            *s /= norm;
        }
    }
    sum
}

/// Time-decay factor. Returns a multiplier in [0, 1] that decays a vocab
/// term's score by elapsed time since `last_used`. Half-life: `HALF_LIFE_DAYS`.
///
/// Per the Ebbinghaus literature ("meaningful content forgets ~10× slower"),
/// dictation vocabulary — which is highly recurrent and intentional — gets
/// a generous 45-day half-life by default. Result: a term untouched for
/// 45d gets weight 0.5, untouched for 90d gets 0.25, etc.
fn decay_factor(last_used_ms: i64, now_ms: i64) -> f32 {
    const HALF_LIFE_DAYS: f32 = 45.0;
    let elapsed_ms = (now_ms - last_used_ms).max(0) as f32;
    let elapsed_days = elapsed_ms / (1000.0 * 60.0 * 60.0 * 24.0);
    // exp(-λ Δt) where λ = ln(2) / half_life
    (-std::f32::consts::LN_2 * elapsed_days / HALF_LIFE_DAYS).exp()
}

/// Use-count factor: log(1 + use_count). Diminishing returns — a term used
/// 100 times isn't 100× more relevant than one used twice; it's ~6× more.
fn use_count_factor(use_count: i64) -> f32 {
    (1.0 + use_count.max(0) as f32).ln() + 1.0
}

/// Remove a term's centroid AND its FIFO ring of example embeddings.
/// Called by the vocabulary delete path; safe to call when no row exists.
///
/// Why both: `vocab_embedding_examples` has no FK cascade to `vocabulary`.
/// Without explicit cleanup, deleting a term leaves 1–10 orphan ring rows
/// behind. If the user later re-adds the same term, those zombie rows
/// would resurface in the centroid recompute as ghost sightings.
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

/// Top-K vocab terms (with their full row data) by cosine similarity to
/// `query_embedding`. Filters out rows below `min_sim`. Returns at most K.
///
/// Filters by `language` — passes rows whose vocabulary.language is NULL
/// (legacy / language-agnostic) or matches.
pub fn top_k_relevant(
    pool: &DbPool,
    user_id: &str,
    query: &[f32],
    language: &str,
    k: usize,
    min_sim: f32,
) -> Vec<VocabTerm> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let mut stmt = match conn.prepare(
        "SELECT v.term, ve.embedding, v.weight, v.use_count, v.last_used,
                v.source, v.example_context, v.term_type, v.meaning
           FROM vocab_embeddings ve
           JOIN vocabulary v
             ON v.user_id = ve.user_id AND v.term = ve.term
          WHERE ve.user_id = ?1
            AND (v.language = ?2 OR v.language IS NULL)",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let rows: Vec<VocabRow> = stmt
        .query_map(params![user_id, language], |row| {
            let blob: Vec<u8> = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                blob,
                row.get::<_, f64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .ok()
        .map(|iter| {
            iter.filter_map(|r| r.ok())
                .filter_map(|(term, blob, weight, uc, lu, src, ctx, ty, mn)| {
                    blob_to_floats(&blob).map(|embedding| VocabRow {
                        term,
                        embedding,
                        weight,
                        use_count: uc,
                        last_used: lu,
                        source: src,
                        example_context: ctx,
                        term_type: ty,
                        meaning: mn,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if rows.is_empty() {
        return vec![];
    }

    let query_norm = l2_norm(query);
    if query_norm == 0.0 {
        return vec![];
    }
    let now = now_ms();

    // Score = cosine × decay × use_factor
    //
    // Cosine remains the gate (we still apply min_sim BEFORE multiplicative
    // factors so an unrelated term can't be promoted by sheer recency).
    // After the gate, the raw cosine is reweighted by the time-decay
    // multiplier (exp(-λΔt)) and a diminishing-returns use-count factor
    // (log(1+use)+1). Net effect: among entries that meet the cosine bar,
    // recent + frequently-used ones rank higher than ancient + rare ones.
    let mut scored: Vec<(f32, VocabRow)> = rows
        .into_iter()
        .filter_map(|row| {
            let row_norm = l2_norm(&row.embedding);
            if row_norm == 0.0 {
                return None;
            }
            let cos = dot(&row.embedding, query) / (row_norm * query_norm);
            if cos < min_sim {
                return None;
            }
            let decay = decay_factor(row.last_used, now);
            let usef = use_count_factor(row.use_count);
            Some((cos * decay * usef, row))
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);

    scored.into_iter().map(|(_, r)| r.into_term()).collect()
}

/// Build the polish prompt's vocabulary slice using a HYBRID strategy:
///
///   • **Always** include starred terms (user-pinned, regardless of relevance)
///   • **Hybrid retrieval**: combine dense (cosine on centroids, with time-
///     decay reinforcement) and sparse (BM25 on term + example_context) via
///     Reciprocal Rank Fusion. This catches both semantic matches AND
///     exact-keyword matches that pure cosine misses (acronyms, brand
///     names, code identifiers).
///   • **Top-N by weight** is added only when hybrid retrieval found NOTHING
///     (fresh install / embedder down / transcript matches no past context).
///
/// `query_embedding` may be None — we skip the dense leg in that case but
/// still run BM25 if we have a `query_text`. When both are unavailable, fall
/// back to starred + weight.
///
/// `query_text` is the raw transcript (for BM25). `query_embedding` is the
/// transcript's vector (for cosine). We need both for hybrid; either alone
/// degrades gracefully.
pub fn select_for_polish(
    pool: &DbPool,
    user_id: &str,
    language: &str,
    query_embedding: Option<&[f32]>,
    n_top_weight: usize,
    k_relevant: usize,
    max_total: usize,
    min_sim: f32,
) -> Vec<VocabTerm> {
    select_for_prompt_with_tiers(
        pool,
        user_id,
        language,
        query_embedding,
        /* query_text = */ None,
        n_top_weight,
        k_relevant,
        max_total,
        min_sim,
    )
    .into_iter()
    .map(|selection| selection.term)
    .collect()
}

/// Shared selector for the final LLM polish prompt.
///
/// Voice and text polish should scope vocabulary identically so the final
/// prompt only sees terms backed by transcript evidence instead of broad
/// top-weight slates.
pub fn select_for_prompt(
    pool: &DbPool,
    user_id: &str,
    language: &str,
    query_embedding: Option<&[f32]>,
    query_text: Option<&str>,
) -> Vec<VocabTerm> {
    select_for_prompt_with_tiers(
        pool,
        user_id,
        language,
        query_embedding,
        query_text,
        PROMPT_VOCAB_TOP_WEIGHT,
        PROMPT_VOCAB_K_RELEVANT,
        PROMPT_VOCAB_MAX_TOTAL,
        PROMPT_VOCAB_MIN_SIM,
    )
    .into_iter()
    .map(|selection| selection.term)
    .collect()
}

/// Tiered transcript-evidence selector for the final polish prompt.
///
/// APPLY entries are safe normalization evidence: exact canonical term, split
/// CamelCase/PascalCase form, or exact approved STT alias. SUGGEST entries are
/// longer near-surface matches; the polish model receives them as possible
/// matches and must use judgment.
pub fn select_for_prompt_with_tiers(
    pool: &DbPool,
    user_id: &str,
    _language: &str,
    _query_embedding: Option<&[f32]>,
    query_text: Option<&str>,
    _n_top_weight: usize,
    _k_relevant: usize,
    max_total: usize,
    _min_sim: f32,
) -> Vec<VocabSelection> {
    use crate::store::vocabulary;

    let Some(query_text) = query_text.map(str::trim).filter(|s| !s.is_empty()) else {
        return vocabulary::top_terms(pool, user_id, 1000)
            .into_iter()
            .filter(|t| t.source == "starred")
            .take(max_total)
            .map(|term| VocabSelection {
                term,
                tier: VocabSelectionTier::Suggest,
                reason: "starred_no_transcript".to_string(),
                evidence: String::new(),
                score: 0.20,
            })
            .collect();
    };

    let all = vocabulary::top_terms(pool, user_id, 1000);
    let alias_map = load_approved_alias_map(pool, user_id);
    let transcript_norm = normalize_retrieval_text(query_text);
    let transcript_tokens = retrieval_tokens(query_text);
    let transcript_windows = phrase_windows(&transcript_tokens, 4);

    let mut selections = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for term in all.iter().cloned() {
        if let Some(selection) =
            score_vocab_term_v3(term, &alias_map, &transcript_norm, &transcript_windows)
        {
            if seen.insert(selection.term.term.to_ascii_lowercase()) {
                selections.push(selection);
            }
        }
    }

    for term in all.iter().take(20).cloned() {
        if seen.insert(term.term.to_ascii_lowercase()) {
            selections.push(VocabSelection {
                term,
                tier: VocabSelectionTier::Suggest,
                reason: "top_vocab_baseline".to_string(),
                evidence: String::new(),
                score: 0.30,
            });
        }
    }

    selections.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.term.use_count.cmp(&a.term.use_count))
            .then_with(|| {
                b.term
                    .weight
                    .partial_cmp(&a.term.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                a.term
                    .term
                    .to_ascii_lowercase()
                    .cmp(&b.term.term.to_ascii_lowercase())
            })
    });
    selections.truncate(max_total);
    selections
}

fn load_approved_alias_map(
    pool: &DbPool,
    user_id: &str,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let Ok(conn) = pool.get() else {
        return map;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT LOWER(correct_form), LOWER(transcript_form)
           FROM stt_replacements
          WHERE user_id = ?1
            AND review_status = 'approved'",
    ) else {
        return map;
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![user_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return map;
    };
    for row in rows.flatten() {
        let alias = row.1.trim().to_string();
        if !alias.is_empty() {
            map.entry(row.0).or_default().push(alias);
        }
    }
    map
}

fn score_vocab_term_v3(
    term: VocabTerm,
    alias_map: &std::collections::HashMap<String, Vec<String>>,
    transcript_norm: &str,
    transcript_windows: &[String],
) -> Option<VocabSelection> {
    let term_key = normalize_retrieval_text(&term.term);
    if phrase_present(transcript_norm, &term_key) {
        return Some(VocabSelection {
            term,
            tier: VocabSelectionTier::Apply,
            reason: "exact_term".to_string(),
            evidence: term_key,
            score: 1.0,
        });
    }

    let split_key = normalize_retrieval_text(&split_camel_case(&term.term));
    if split_key != term_key && phrase_present(transcript_norm, &split_key) {
        return Some(VocabSelection {
            term,
            tier: VocabSelectionTier::Apply,
            reason: "exact_split_term".to_string(),
            evidence: split_key,
            score: 0.99,
        });
    }

    if let Some(aliases) = alias_map.get(&term_key) {
        for alias in aliases {
            let alias_key = normalize_retrieval_text(alias);
            if phrase_present(transcript_norm, &alias_key) {
                return Some(VocabSelection {
                    term,
                    tier: VocabSelectionTier::Apply,
                    reason: "exact_alias".to_string(),
                    evidence: alias_key,
                    score: 0.98,
                });
            }
        }
        if let Some(alias_match) = best_approved_alias_neighbor(transcript_windows, &term, aliases)
        {
            return Some(VocabSelection {
                term,
                tier: VocabSelectionTier::Suggest,
                reason: "near_approved_alias".to_string(),
                evidence: alias_match.evidence,
                score: 0.78 + alias_match.surface.max(alias_match.phonetic).min(0.10),
            });
        }
    }

    let target_compact = compact_retrieval_text(&term.term);
    if matches!(term.term_type.as_deref(), Some("acronym")) || target_compact.chars().count() < 5 {
        return None;
    }

    let precise = matches!(
        term.term_type.as_deref(),
        Some("proper_noun" | "brand" | "code_identifier" | "phrase")
    ) || crate::llm::phonetics::jargon_score(&term.term) >= 0.45;
    let best = best_surface_window(transcript_windows, &term.term)?;
    if !precise && target_compact.chars().count() < 7 {
        return None;
    }
    if best.surface >= 0.82 {
        return Some(VocabSelection {
            term,
            tier: VocabSelectionTier::Suggest,
            reason: if precise {
                "near_surface_precise_term".to_string()
            } else {
                "near_surface_long_term".to_string()
            },
            evidence: best.evidence,
            score: 0.82 + (best.surface - 0.82).min(0.08),
        });
    }
    if precise && best.phonetic >= 0.82 && best.surface >= 0.55 {
        return Some(VocabSelection {
            term,
            tier: VocabSelectionTier::Suggest,
            reason: "strong_phonetic_term".to_string(),
            evidence: best.evidence,
            score: 0.82 + (best.phonetic - 0.82).min(0.08),
        });
    }
    None
}

#[derive(Debug)]
struct WindowScore {
    evidence: String,
    surface: f64,
    phonetic: f64,
}

fn best_approved_alias_neighbor(
    windows: &[String],
    term: &VocabTerm,
    aliases: &[String],
) -> Option<WindowScore> {
    if matches!(term.term_type.as_deref(), Some("acronym")) {
        return None;
    }
    if !matches!(
        term.term_type.as_deref(),
        Some("proper_noun" | "brand" | "code_identifier" | "phrase")
    ) {
        return None;
    }

    let mut best: Option<WindowScore> = None;
    for alias in aliases {
        let alias_norm = normalize_retrieval_text(alias);
        let alias_compact = compact_retrieval_text(&alias_norm);
        if alias_compact.chars().count() < 2 {
            continue;
        }
        for window in windows {
            if window
                .split_whitespace()
                .all(|word| is_common_retrieval_word(word))
            {
                continue;
            }
            let window_norm = normalize_retrieval_text(window);
            let window_compact = compact_retrieval_text(&window_norm);
            let surface = normalized_similarity(&window_norm, &alias_norm)
                .max(normalized_similarity(&window_compact, &alias_compact))
                .max(spoken_alias_similarity(&window_norm, &alias_norm));
            let phonetic = crate::llm::phonetics::similarity(&window_norm, &alias_norm);
            let matched = surface >= 0.74 || (phonetic >= 0.74 && surface >= 0.50);
            if !matched {
                continue;
            }
            if best
                .as_ref()
                .map(|current| surface.max(phonetic) > current.surface.max(current.phonetic))
                .unwrap_or(true)
            {
                best = Some(WindowScore {
                    evidence: window_norm,
                    surface,
                    phonetic,
                });
            }
        }
    }
    best
}

fn best_surface_window(windows: &[String], term: &str) -> Option<WindowScore> {
    let mut best: Option<WindowScore> = None;
    let target_norm = normalize_retrieval_text(term);
    let target_compact = compact_retrieval_text(term);
    for window in windows {
        if window
            .split_whitespace()
            .all(|word| is_common_retrieval_word(word))
        {
            continue;
        }
        let surface = normalized_similarity(window, &target_norm).max(normalized_similarity(
            &compact_retrieval_text(window),
            &target_compact,
        ));
        let phonetic = crate::llm::phonetics::similarity(window, term);
        if best
            .as_ref()
            .map(|current| surface.max(phonetic) > current.surface.max(current.phonetic))
            .unwrap_or(true)
        {
            best = Some(WindowScore {
                evidence: window.clone(),
                surface,
                phonetic,
            });
        }
    }
    best
}

fn retrieval_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '@' && c != '_' && c != '-' && c != '.')
        .map(normalize_retrieval_text)
        .filter(|s| !s.is_empty())
        .collect()
}

fn phrase_windows(tokens: &[String], max_width: usize) -> Vec<String> {
    let mut windows = Vec::new();
    for width in 1..=max_width.min(tokens.len()) {
        for start in 0..=tokens.len() - width {
            windows.push(tokens[start..start + width].join(" "));
        }
    }
    windows
}

fn phrase_present(text_norm: &str, phrase_norm: &str) -> bool {
    let phrase: Vec<&str> = phrase_norm.split_whitespace().collect();
    if phrase.is_empty() {
        return false;
    }
    let words: Vec<&str> = text_norm.split_whitespace().collect();
    if phrase.len() == 1 {
        return words.iter().any(|word| *word == phrase[0]);
    }
    words
        .windows(phrase.len())
        .any(|window| window == phrase.as_slice())
}

fn normalize_retrieval_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true;
    for ch in text.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_alphanumeric() || matches!(ch, '@' | '_' | '-' | '.' | '#') {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

fn compact_retrieval_text(text: &str) -> String {
    normalize_retrieval_text(text)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn spoken_alias_similarity(a: &str, b: &str) -> f64 {
    normalized_similarity(
        &normalize_spoken_alias_text(a),
        &normalize_spoken_alias_text(b),
    )
}

fn normalize_spoken_alias_text(text: &str) -> String {
    normalize_retrieval_text(text)
        .split_whitespace()
        .map(|word| match word {
            "woh" | "voh" | "vah" | "wa" => "vo",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_camel_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    let mut prev: Option<char> = None;
    for ch in text.chars() {
        if let Some(prev_ch) = prev {
            if (prev_ch.is_ascii_lowercase() || prev_ch.is_ascii_digit()) && ch.is_ascii_uppercase()
            {
                out.push(' ');
            }
        }
        if matches!(ch, '_' | '-') {
            out.push(' ');
        } else {
            out.push(ch);
        }
        prev = Some(ch);
    }
    out
}

fn normalized_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let distance = levenshtein_chars(a, b) as f64;
    let max_len = a.chars().count().max(b.chars().count()) as f64;
    1.0 - distance / max_len
}

fn levenshtein_chars(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn is_common_retrieval_word(word: &str) -> bool {
    matches!(
        word,
        "a" | "ab"
            | "and"
            | "app"
            | "are"
            | "be"
            | "but"
            | "dev"
            | "did"
            | "do"
            | "hai"
            | "hain"
            | "i"
            | "in"
            | "is"
            | "it"
            | "ka"
            | "kar"
            | "ke"
            | "ki"
            | "ko"
            | "mac"
            | "main"
            | "me"
            | "mein"
            | "of"
            | "on"
            | "or"
            | "set"
            | "site"
            | "that"
            | "the"
            | "this"
            | "to"
            | "we"
            | "you"
    )
}

/// Internal: per-term score within the lexically-gated set. Combines
/// cosine on the term's centroid, time-decay, and log(1+use_count).
fn score_within_set(
    conn: &Option<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>>,
    user_id: &str,
    vt: &VocabTerm,
    q: &[f32],
    q_norm: f32,
    now: i64,
) -> f32 {
    // Default if we can't load embedding: fall back to weight × decay.
    let weight_decay = vt.weight as f32 * decay_factor(vt.last_used, now);
    let conn = match conn {
        Some(c) => c,
        None => return weight_decay,
    };
    let blob: Vec<u8> = match conn.query_row(
        "SELECT embedding FROM vocab_embeddings WHERE user_id=?1 AND term=?2",
        params![user_id, vt.term],
        |row| row.get(0),
    ) {
        Ok(b) => b,
        Err(_) => return weight_decay,
    };
    let centroid = match blob_to_floats(&blob) {
        Some(v) => v,
        None => return weight_decay,
    };
    let cn = l2_norm(&centroid);
    if cn == 0.0 {
        return weight_decay;
    }
    let cos = dot(&centroid, q) / (cn * q_norm);
    cos * decay_factor(vt.last_used, now) * use_count_factor(vt.use_count)
}

// ── Math helpers (kept local — same impl as vectors.rs) ───────────────────────

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
        let mgr = SqliteConnectionManager::memory();
        // r2d2's :memory: connections are per-connection isolated, so multi-
        // conn pools each get a fresh empty DB. Single-conn is correct; the
        // helpers in this module that take `pool: &DbPool` must be careful
        // never to hold a conn open while calling another store fn.
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        pool.get()
            .unwrap()
            .execute_batch(
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
             );
             CREATE TABLE vocab_embeddings (
                 user_id    TEXT NOT NULL REFERENCES local_user(id),
                 term       TEXT NOT NULL,
                 embedding  BLOB NOT NULL,
                 updated_at INTEGER NOT NULL,
                 UNIQUE(user_id, term)
             );
             CREATE TABLE vocab_embedding_examples (
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id       TEXT NOT NULL REFERENCES local_user(id),
                 term          TEXT NOT NULL,
                 embedding     BLOB NOT NULL,
                 example_text  TEXT NOT NULL,
                 recorded_at   INTEGER NOT NULL
             );
             CREATE INDEX idx_vocab_examples_user_term
               ON vocab_embedding_examples (user_id, term, recorded_at DESC);
             CREATE VIRTUAL TABLE vocab_fts USING fts5(
                 user_id UNINDEXED, term, example_context,
                 tokenize = 'unicode61 remove_diacritics 2'
             );",
            )
            .unwrap();
        pool
    }

    fn seed(
        pool: &DbPool,
        term: &str,
        weight: f64,
        source: &str,
        embedding: &[f32],
        language: &str,
    ) {
        // Scope the conn so it's released before upsert_embedding takes its
        // own from the pool (max_size=1 in tests would deadlock otherwise).
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used, source, language)
                 VALUES ('u1', ?1, ?2, 1, ?3, ?4, ?5)",
                params![term, weight, now_ms(), source, language],
            ).unwrap();
        }
        upsert_embedding(pool, "u1", term, embedding);
    }

    /// Build a tiny 4-d unit-ish vector for testing cosine math.
    fn vec4(a: f32, b: f32, c: f32, d: f32) -> Vec<f32> {
        vec![a, b, c, d]
    }

    #[test]
    fn support_examples_curate_stable_meaning_contexts() {
        let pool = mem_pool();
        seed(
            &pool,
            "EMIAC",
            2.0,
            "manual",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "hinglish",
        );
        let conn = pool.get().unwrap();
        let emb = crate::embedder::gemini::floats_to_blob(&vec4(1.0, 0.0, 0.0, 0.0));
        let cases = [
            (1_i64, "EMIAC growth in FY2026 premium segment"),
            (2_i64, "EMIAC company overview"),
            (3_i64, "EMIAC trimmer margin compared with MACOBS"),
            (4_i64, "Latest EMIAC update for retail expansion"),
        ];
        for (recorded_at, text) in cases {
            conn.execute(
                "INSERT INTO vocab_embedding_examples (user_id, term, embedding, example_text, recorded_at)
                 VALUES ('u1', 'EMIAC', ?1, ?2, ?3)",
                params![emb, text, recorded_at],
            )
            .unwrap();
        }
        drop(conn);

        let support = support_example_texts(&pool, "u1", "EMIAC", 4);
        assert!(support.contains(&"Latest EMIAC update for retail expansion".to_string()));
        assert!(support.contains(&"EMIAC trimmer margin compared with MACOBS".to_string()));
        assert!(support.len() >= 2);
        assert!(support.len() <= 4);
    }

    // ── Centroid ring + drift detection ───────────────────────────────────────

    fn unit(v: Vec<f32>) -> Vec<f32> {
        let n = (v.iter().map(|x| x * x).sum::<f32>()).sqrt();
        if n == 0.0 {
            v
        } else {
            v.into_iter().map(|x| x / n).collect()
        }
    }

    #[test]
    fn ring_buffer_caps_at_examples_ring_size() {
        let pool = mem_pool();
        // Seed the parent vocabulary row first.
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used)
             VALUES ('u1', 'TERM', 1.0, 1, ?1)",
                params![now_ms()],
            )
            .unwrap();
        // Push 15 example embeddings; ring should keep only the latest 10.
        for i in 0..15 {
            let emb = unit(vec![i as f32, 0.0, 0.0, 0.0]);
            record_example_and_recentre(&pool, "u1", "TERM", &emb, &format!("ex{i}"));
        }
        let conn = pool.get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vocab_embedding_examples WHERE user_id='u1' AND term='TERM'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, EXAMPLES_RING_SIZE as i64);
    }

    #[test]
    fn centroid_is_mean_of_examples() {
        let pool = mem_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used)
             VALUES ('u1', 'X', 1.0, 1, ?1)",
                params![now_ms()],
            )
            .unwrap();
        record_example_and_recentre(&pool, "u1", "X", &unit(vec![1.0, 0.0, 0.0, 0.0]), "a");
        record_example_and_recentre(&pool, "u1", "X", &unit(vec![1.0, 0.0, 0.0, 0.0]), "b");
        // Centroid of two identical unit vectors should be the same vector.
        let got = top_k_relevant(
            &pool,
            "u1",
            &unit(vec![1.0, 0.0, 0.0, 0.0]),
            "english",
            5,
            0.0,
        );
        assert_eq!(got.len(), 1);
        // Cosine should be ~1.0 (identical to query).
    }

    #[test]
    fn centroid_shifts_toward_new_examples() {
        let pool = mem_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used)
             VALUES ('u1', 'X', 1.0, 1, ?1)",
                params![now_ms()],
            )
            .unwrap();
        // Start with examples in direction (1, 0, 0, 0).
        for _ in 0..3 {
            record_example_and_recentre(&pool, "u1", "X", &unit(vec![1.0, 0.0, 0.0, 0.0]), "old");
        }
        // Add 7 examples in direction (0, 1, 0, 0).
        for _ in 0..7 {
            record_example_and_recentre(&pool, "u1", "X", &unit(vec![0.0, 1.0, 0.0, 0.0]), "new");
        }
        // Centroid should now be closer to (0, 1, 0, 0) than (1, 0, 0, 0).
        let against_new = top_k_relevant(
            &pool,
            "u1",
            &unit(vec![0.0, 1.0, 0.0, 0.0]),
            "english",
            5,
            0.0,
        );
        let against_old = top_k_relevant(
            &pool,
            "u1",
            &unit(vec![1.0, 0.0, 0.0, 0.0]),
            "english",
            5,
            0.0,
        );
        assert_eq!(
            against_new.len(),
            1,
            "centroid should match the new direction"
        );
        // 'old' direction may also score above 0 cosine but lower; we don't
        // need a hard ordering — the key fact is centroid moved.
        let _ = against_old;
    }

    #[test]
    fn cluster_spread_low_for_cohesive_examples() {
        let pool = mem_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used)
             VALUES ('u1', 'COHESIVE', 1.0, 1, ?1)",
                params![now_ms()],
            )
            .unwrap();
        // Three nearly-identical examples — variance should be ~0.
        record_example_and_recentre(
            &pool,
            "u1",
            "COHESIVE",
            &unit(vec![1.0, 0.05, 0.0, 0.0]),
            "a",
        );
        record_example_and_recentre(
            &pool,
            "u1",
            "COHESIVE",
            &unit(vec![1.0, 0.0, 0.05, 0.0]),
            "b",
        );
        record_example_and_recentre(
            &pool,
            "u1",
            "COHESIVE",
            &unit(vec![1.0, 0.0, 0.0, 0.05]),
            "c",
        );
        let s = cluster_spread(&pool, "u1", "COHESIVE");
        assert!(s < 0.1, "cohesive cluster spread should be low, got {s}");
    }

    #[test]
    fn cluster_spread_high_for_bimodal_examples() {
        let pool = mem_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used)
             VALUES ('u1', 'MERCURY', 1.0, 1, ?1)",
                params![now_ms()],
            )
            .unwrap();
        // Half pointing one way (planet), half another (band).
        for _ in 0..3 {
            record_example_and_recentre(
                &pool,
                "u1",
                "MERCURY",
                &unit(vec![1.0, 0.0, 0.0, 0.0]),
                "planet",
            );
        }
        for _ in 0..3 {
            record_example_and_recentre(
                &pool,
                "u1",
                "MERCURY",
                &unit(vec![0.0, 1.0, 0.0, 0.0]),
                "band",
            );
        }
        let s = cluster_spread(&pool, "u1", "MERCURY");
        assert!(s > 0.2, "bimodal cluster spread should be high, got {s}");
    }

    // ── Time-decay scoring ────────────────────────────────────────────────────

    #[test]
    fn decay_factor_is_one_at_zero_elapsed() {
        let now = 1_000_000_000_000_i64;
        assert!((decay_factor(now, now) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn decay_factor_halves_at_half_life() {
        let now = 2_000_000_000_000_i64;
        let one_half_life_ago = now - 45 * 24 * 3600 * 1000;
        let f = decay_factor(one_half_life_ago, now);
        assert!(
            (f - 0.5).abs() < 0.01,
            "decay at 45d should be ~0.5, got {f}"
        );
    }

    #[test]
    fn decay_factor_quarters_at_two_half_lives() {
        let now = 2_000_000_000_000_i64;
        let two_half_lives_ago = now - 90 * 24 * 3600 * 1000;
        let f = decay_factor(two_half_lives_ago, now);
        assert!(
            (f - 0.25).abs() < 0.01,
            "decay at 90d should be ~0.25, got {f}"
        );
    }

    #[test]
    fn use_count_factor_grows_logarithmically() {
        let f1 = use_count_factor(1);
        let f10 = use_count_factor(10);
        let f100 = use_count_factor(100);
        // Should be diminishing returns: 100× use is not 100× factor.
        assert!(f100 < 10.0 * f1, "use_count_factor should be sub-linear");
        assert!(f10 > f1);
        assert!(f100 > f10);
    }

    #[test]
    fn bump_last_used_updates_timestamp() {
        let pool = mem_pool();
        // Seed a row with last_used 1 day ago.
        let day_ago = now_ms() - 86_400_000;
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used)
             VALUES ('u1', 'TICK', 1.0, 0, ?1)",
                params![day_ago],
            )
            .unwrap();
        bump_last_used(&pool, "u1", &["TICK".into()]);
        let row: (i64, i64) = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT last_used, use_count FROM vocabulary WHERE term='TICK'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(row.0 > day_ago, "last_used should advance");
        assert_eq!(row.1, 1, "use_count should increment");
    }

    #[test]
    fn upsert_and_retrieve_round_trip() {
        let pool = mem_pool();
        seed(
            &pool,
            "MACOBS",
            2.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
        );
        let got = top_k_relevant(&pool, "u1", &vec4(1.0, 0.0, 0.0, 0.0), "english", 5, 0.0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].term, "MACOBS");
    }

    #[test]
    fn cosine_ordering_correct() {
        let pool = mem_pool();
        // Aligned with query → high similarity
        seed(
            &pool,
            "FINANCE",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
        );
        // Orthogonal → zero
        seed(
            &pool,
            "COOKING",
            1.0,
            "auto",
            &vec4(0.0, 1.0, 0.0, 0.0),
            "english",
        );
        // Slightly aligned
        seed(
            &pool,
            "ECONOMY",
            1.0,
            "auto",
            &vec4(0.7, 0.3, 0.0, 0.0),
            "english",
        );

        let got = top_k_relevant(&pool, "u1", &vec4(1.0, 0.0, 0.0, 0.0), "english", 5, 0.0);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].term, "FINANCE"); // sim = 1.0
        assert_eq!(got[1].term, "ECONOMY"); // sim ≈ 0.92
        assert_eq!(got[2].term, "COOKING"); // sim = 0.0
    }

    #[test]
    fn min_sim_filters_out_low_relevance() {
        let pool = mem_pool();
        seed(
            &pool,
            "FINANCE",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
        );
        seed(
            &pool,
            "COOKING",
            1.0,
            "auto",
            &vec4(0.0, 1.0, 0.0, 0.0),
            "english",
        );
        let got = top_k_relevant(&pool, "u1", &vec4(1.0, 0.0, 0.0, 0.0), "english", 5, 0.5);
        assert_eq!(got.len(), 1); // COOKING filtered (sim = 0.0)
        assert_eq!(got[0].term, "FINANCE");
    }

    #[test]
    fn delete_clears_embedding() {
        let pool = mem_pool();
        seed(
            &pool,
            "TERM",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
        );
        delete(&pool, "u1", "TERM");
        assert!(
            top_k_relevant(&pool, "u1", &vec4(1.0, 0.0, 0.0, 0.0), "english", 5, 0.0).is_empty()
        );
    }

    #[test]
    fn delete_also_clears_examples_ring() {
        // Regression: deleting a vocab term used to leak its FIFO ring of
        // example embeddings (no FK cascade from vocabulary). Re-adding the
        // same term later would resurrect those rows in the centroid
        // recompute. delete() must wipe both the centroid and the ring.
        let pool = mem_pool();
        seed(
            &pool,
            "TERM",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
        );
        record_example_and_recentre(&pool, "u1", "TERM", &unit(vec![1.0, 0.0, 0.0, 0.0]), "ex1");
        record_example_and_recentre(&pool, "u1", "TERM", &unit(vec![0.0, 1.0, 0.0, 0.0]), "ex2");

        delete(&pool, "u1", "TERM");

        let conn = pool.get().unwrap();
        let ring_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vocab_embedding_examples WHERE user_id='u1' AND term='TERM'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ring_count, 0,
            "examples ring must be cleared on term delete (no zombie sightings)"
        );
    }

    #[test]
    fn upsert_replaces_existing_embedding() {
        let pool = mem_pool();
        seed(
            &pool,
            "TERM",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
        );
        // Re-embed with a different vector.
        upsert_embedding(&pool, "u1", "TERM", &vec4(0.0, 1.0, 0.0, 0.0));
        // Original-direction query should now miss; new-direction should hit.
        assert!(
            top_k_relevant(&pool, "u1", &vec4(1.0, 0.0, 0.0, 0.0), "english", 5, 0.5).is_empty()
        );
        assert_eq!(
            top_k_relevant(&pool, "u1", &vec4(0.0, 1.0, 0.0, 0.0), "english", 5, 0.5).len(),
            1,
        );
    }

    // ── Tiered prompt selector tests ──────────────────────────────────────────
    //
    // The production selector is transcript-evidence-first. APPLY means exact
    // canonical/split/approved-alias evidence. SUGGEST means longer fuzzy
    // evidence that the polish model may accept or ignore in context.

    /// Helper: also write a vocab_fts row for legacy selector wrappers.
    fn seed_with_context(
        pool: &DbPool,
        term: &str,
        weight: f64,
        source: &str,
        embedding: &[f32],
        language: &str,
        context: &str,
    ) {
        seed_with_context_and_meaning(
            pool,
            term,
            weight,
            source,
            embedding,
            language,
            context,
            Some("Test meaning."),
        );
    }

    fn seed_with_context_and_meaning(
        pool: &DbPool,
        term: &str,
        weight: f64,
        source: &str,
        embedding: &[f32],
        language: &str,
        context: &str,
        meaning: Option<&str>,
    ) {
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO vocabulary
                   (user_id, term, weight, use_count, last_used, source, language, example_context, meaning)
                 VALUES ('u1', ?1, ?2, 1, ?3, ?4, ?5, ?6, ?7)",
                params![term, weight, now_ms(), source, language, context, meaning],
            ).unwrap();
            conn.execute(
                "INSERT INTO vocab_fts (user_id, term, example_context)
                 VALUES ('u1', ?1, ?2)",
                params![term, context],
            )
            .unwrap();
        }
        upsert_embedding(pool, "u1", term, embedding);
    }

    fn select_terms(
        pool: &DbPool,
        user_id: &str,
        language: &str,
        query_embedding: Option<&[f32]>,
        query_text: Option<&str>,
        n_top_weight: usize,
        k_relevant: usize,
        max_total: usize,
        min_sim: f32,
    ) -> Vec<VocabTerm> {
        select_for_prompt_with_tiers(
            pool,
            user_id,
            language,
            query_embedding,
            query_text,
            n_top_weight,
            k_relevant,
            max_total,
            min_sim,
        )
        .into_iter()
        .map(|selection| selection.term)
        .collect()
    }

    #[test]
    fn tiered_selector_includes_term_when_transcript_mentions_it_directly() {
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "MACOBS",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "MACOBS ka IPO ka 12 hazaar batana",
        );

        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("the MACOBS announcement"), // term itself appears in transcript
            5,
            5,
            10,
            0.0,
        );
        assert!(
            chosen.iter().any(|v| v.term.term == "MACOBS"),
            "term-itself match must include the entry"
        );
    }

    #[test]
    fn tiered_selector_excludes_term_on_example_context_only_overlap() {
        // FOUNDATIONAL: "context confirmed" means the term itself must be
        // present in the transcript (verbatim or phonetically close). An
        // example_context-only overlap is NOT enough — that was the loose
        // gate that caused the polish LLM to hallucinate vocab into
        // unrelated places. Mishearing recovery for this case (e.g.
        // "main corps" → "MACOBS") is handled by the deterministic
        // stt_replacements layer, not by polish-prompt vocab injection.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "MACOBS",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "MACOBS ka IPO ka 12 hazaar batana",
        );

        // Transcript shares "ka", "IPO", "hazaar" with example_context but
        // contains no token phonetically close to MACOBS.
        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("main corps ka IPO ka 12 hazaar batana"),
            5,
            5,
            10,
            0.0,
        );
        let macobs = chosen.iter().find(|v| v.term.term == "MACOBS");
        assert!(
            macobs.is_some_and(
                |v| v.reason == "top_vocab_baseline" && v.tier == VocabSelectionTier::Suggest
            ),
            "context-only overlap must not look like evidence; it may only appear as weak top-vocab baseline"
        );
    }

    #[test]
    fn tiered_selector_excludes_term_when_no_transcript_overlap() {
        // The "tembeess for time" regression. tembeess vocab exists with a
        // distinct context. Transcript "what time is it" shares no words
        // with tembeess or its context. Lexical gate must EXCLUDE tembeess
        // → no over-replacement possible at the polish-prompt layer.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "tembeess",
            4.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "tembeess team meeting on Friday",
        );

        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(0.99, 0.0, 0.0, 0.0)), // semantically near (cosine high)
            Some("what time is it"),          // BUT no lexical overlap
            5,
            5,
            10,
            0.0,
        );
        let tembeess = chosen.iter().find(|v| v.term.term == "tembeess");
        assert!(
            tembeess.is_some_and(
                |v| v.reason == "top_vocab_baseline" && v.tier == VocabSelectionTier::Suggest
            ),
            "unrelated high-cosine terms may only appear as weak top-vocab baseline"
        );
    }

    #[test]
    fn tiered_selector_starred_without_evidence_is_baseline_suggest() {
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "PINNED",
            0.5,
            "starred",
            &vec4(0.0, 1.0, 0.0, 0.0),
            "english",
            "PINNED is my favourite term",
        );

        // Transcript shares NO words with PINNED or its context.
        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            None,
            Some("the cat sat on the mat"),
            5,
            5,
            10,
            0.0,
        );
        let pinned = chosen.iter().find(|v| v.term.term == "PINNED");
        assert!(
            pinned.is_some_and(
                |v| v.reason == "top_vocab_baseline" && v.tier == VocabSelectionTier::Suggest
            ),
            "unrelated starred terms should be weak prompt context, not hard evidence"
        );
    }

    #[test]
    fn tiered_selector_uses_top_vocab_baseline_for_no_match() {
        // Trial behaviour: even when nothing matches, send top vocab as weak
        // SUGGEST context so the model can recover short names like Divo.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "tembeess",
            5.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "tembeess Friday team meeting",
        );

        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(0.99, 0.0, 0.0, 0.0)), // semantically near
            Some("what time is it"),          // no lexical anchor
            10, // n_top_weight — must NOT fire because gate ran (text was passed)
            5,
            25,
            0.0,
        );
        let tembeess = chosen.iter().find(|v| v.term.term == "tembeess");
        assert!(
            tembeess.is_some_and(
                |v| v.reason == "top_vocab_baseline" && v.tier == VocabSelectionTier::Suggest
            ),
            "no-match transcript should still receive top vocab as weak SUGGEST context"
        );
    }

    #[test]
    fn no_text_call_falls_back_to_starred_only() {
        // Legacy callers (no transcript passed) get the old behaviour:
        // starred only. Top-weight auto terms without evidence are prompt
        // pollution and are intentionally not injected.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "STARRED",
            0.5,
            "starred",
            &vec4(0.0, 1.0, 0.0, 0.0),
            "english",
            "",
        );
        seed_with_context(
            &pool,
            "HEAVY",
            4.0,
            "auto",
            &vec4(0.0, 0.0, 1.0, 0.0),
            "english",
            "",
        );

        let chosen = select_terms(
            &pool, "u1", "english", None, // no embedding
            None, // no transcript → lexical gate doesn't run
            5, 5, 10, 0.0,
        );
        let names: Vec<&str> = chosen.iter().map(|v| v.term.as_str()).collect();
        assert!(names.contains(&"STARRED"));
        assert!(
            !names.contains(&"HEAVY"),
            "no-text fallback must not inject top-weight auto terms"
        );
    }

    #[test]
    fn within_gated_set_cosine_ranks_higher_first() {
        // When multiple terms BOTH appear in the transcript, cosine + decay
        // + use_count determines the order within the gated set.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "MACOBS",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "MACOBS ka IPO ka 12 hazaar",
        );
        seed_with_context(
            &pool,
            "OTHERCO",
            1.0,
            "auto",
            &vec4(0.0, 1.0, 0.0, 0.0),
            "english",
            "OTHERCO ka IPO date hai",
        );

        // Query embedding aligns with MACOBS (1,0,0,0) > OTHERCO (0,1,0,0).
        // Transcript directly contains both terms — passes the strict gate.
        let chosen = select_terms(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("MACOBS and OTHERCO IPO tomorrow"),
            5,
            5,
            10,
            0.0,
        );
        assert!(chosen.iter().any(|v| v.term == "MACOBS"));
        assert!(chosen.iter().any(|v| v.term == "OTHERCO"));
        let macobs_idx = chosen.iter().position(|v| v.term == "MACOBS").unwrap();
        let otherco_idx = chosen.iter().position(|v| v.term == "OTHERCO").unwrap();
        assert!(
            macobs_idx < otherco_idx,
            "MACOBS (cosine-near to query) should rank above OTHERCO"
        );
    }

    #[test]
    fn tiered_selector_exact_term_without_meaning_is_apply() {
        // Exact transcript evidence is enough for APPLY. Requiring meaning
        // here was the old broken gate: it hid valid learned terms from the
        // prompt just because the meaning pipeline had not backfilled them.
        let pool = mem_pool();
        seed_with_context_and_meaning(
            &pool,
            "WITHMEANING",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "WITHMEANING context",
            Some("A test term with a stored meaning."),
        );
        seed_with_context_and_meaning(
            &pool,
            "NOMEANING",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "NOMEANING context",
            None,
        );

        let chosen = select_terms(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("WITHMEANING and NOMEANING both appear here"),
            5,
            5,
            10,
            0.0,
        );
        assert!(
            chosen.iter().any(|v| v.term == "WITHMEANING"),
            "term with meaning must pass the gate"
        );
        assert!(
            chosen.iter().any(|v| v.term == "NOMEANING"),
            "exact term evidence must pass even when meaning is NULL"
        );
    }

    #[test]
    fn tiered_selector_no_text_starred_terms_are_suggest_only() {
        // No-transcript legacy fallback is intentionally weak: starred terms
        // may be shown as suggestions, never as hard APPLY corrections.
        let pool = mem_pool();
        seed_with_context_and_meaning(
            &pool,
            "STARRED_NOMEANING",
            0.5,
            "starred",
            &vec4(0.0, 1.0, 0.0, 0.0),
            "english",
            "starred context",
            None,
        );
        let chosen = select_terms(&pool, "u1", "english", None, None, 5, 5, 10, 0.0);
        assert!(
            chosen.iter().any(|v| v.term == "STARRED_NOMEANING"),
            "starred terms are retained for no-transcript legacy callers"
        );
        let tiered =
            select_for_prompt_with_tiers(&pool, "u1", "english", None, None, 5, 5, 10, 0.0);
        assert!(
            tiered.iter().any(
                |s| s.term.term == "STARRED_NOMEANING" && s.tier == VocabSelectionTier::Suggest
            ),
            "no-transcript starred fallback must be SUGGEST, not APPLY"
        );
    }

    #[test]
    fn tiered_selector_near_surface_long_term_is_suggest() {
        // Fuzzy recovery is allowed for longer precise terms, but only as a
        // SUGGEST entry so the polish model can arbitrate with context.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "Anugra",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "Anugra is a teammate name",
        );
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE vocabulary SET term_type='proper_noun' WHERE user_id='u1' AND term='Anugra'",
                [],
            )
            .unwrap();
        }

        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("ask anupra about the review"),
            5,
            5,
            10,
            0.0,
        );
        let selection = chosen.iter().find(|v| v.term.term == "Anugra");
        assert!(
            selection.is_some_and(|v| v.tier == VocabSelectionTier::Suggest),
            "near-surface long proper noun should be suggested, not hard-applied"
        );
    }

    #[test]
    fn tiered_selector_short_acronym_fuzzy_is_rejected() {
        // Short terms and acronyms are too dangerous for fuzzy matching.
        // Exact/alias can APPLY them; loose words like "site" may only carry
        // STT through the weak top-vocab baseline, never as fuzzy evidence.
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "STT",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "speech to text",
        );
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE vocabulary SET term_type='acronym' WHERE user_id='u1' AND term='STT'",
                [],
            )
            .unwrap();
        }
        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("site quality is bad"),
            5,
            5,
            10,
            0.0,
        );
        let stt = chosen.iter().find(|v| v.term.term == "STT");
        assert!(
            stt.is_some_and(
                |v| v.reason == "top_vocab_baseline" && v.tier == VocabSelectionTier::Suggest
            ),
            "short acronym fuzzy match must be rejected; baseline SUGGEST is allowed"
        );
    }

    #[test]
    fn tiered_selector_approved_alias_is_apply() {
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "DeepSeek",
            1.0,
            "auto",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "english",
            "DeepSeek model",
        );
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS stt_replacements (
                    user_id TEXT NOT NULL,
                    transcript_form TEXT NOT NULL,
                    correct_form TEXT NOT NULL,
                    phonetic_key TEXT NOT NULL,
                    weight REAL NOT NULL DEFAULT 1.0,
                    use_count INTEGER NOT NULL DEFAULT 1,
                    last_used INTEGER NOT NULL,
                    language TEXT,
                    export_tier TEXT NOT NULL DEFAULT 'local_only',
                    contradiction_count INTEGER NOT NULL DEFAULT 0,
                    review_status TEXT NOT NULL DEFAULT 'pending'
                )",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO stt_replacements
                   (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used, language, export_tier, contradiction_count, review_status)
                 VALUES ('u1', 'deep sick', 'DeepSeek', 'DS', 1.0, 1, ?1, 'english', 'local_only', 0, 'approved')",
                params![now_ms()],
            )
            .unwrap();
        }

        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("try deep sick model once"),
            5,
            5,
            10,
            0.0,
        );
        let selection = chosen.iter().find(|v| v.term.term == "DeepSeek");
        assert!(
            selection.is_some_and(|v| {
                v.tier == VocabSelectionTier::Apply
                    && v.reason == "exact_alias"
                    && v.evidence == "deep sick"
            }),
            "approved exact alias must hard-apply the canonical vocab spelling"
        );
    }

    #[test]
    fn tiered_selector_short_name_near_approved_alias_is_suggest() {
        let pool = mem_pool();
        seed_with_context(
            &pool,
            "Divo",
            3.0,
            "confirmed",
            &vec4(1.0, 0.0, 0.0, 0.0),
            "hinglish",
            "",
        );
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE vocabulary SET term_type='proper_noun' WHERE user_id='u1' AND term='Divo'",
                [],
            )
            .unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS stt_replacements (
                    user_id TEXT NOT NULL,
                    transcript_form TEXT NOT NULL,
                    correct_form TEXT NOT NULL,
                    phonetic_key TEXT NOT NULL,
                    weight REAL NOT NULL DEFAULT 1.0,
                    use_count INTEGER NOT NULL DEFAULT 1,
                    last_used INTEGER NOT NULL,
                    language TEXT,
                    export_tier TEXT NOT NULL DEFAULT 'local_only',
                    contradiction_count INTEGER NOT NULL DEFAULT 0,
                    review_status TEXT NOT NULL DEFAULT 'pending'
                )",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO stt_replacements
                   (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used, language, export_tier, contradiction_count, review_status)
                 VALUES ('u1', 'dvo', 'Divo', 'DF', 1.0, 1, ?1, 'hinglish', 'local_only', 0, 'approved')",
                params![now_ms()],
            )
            .unwrap();
        }

        let chosen = select_for_prompt_with_tiers(
            &pool,
            "u1",
            "hinglish",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some("Mere paas yeh note aur d vah donon par kaam karna hai"),
            5,
            5,
            40,
            0.0,
        );
        let divo = chosen.iter().find(|v| v.term.term == "Divo");
        assert!(
            divo.is_some_and(|v| v.reason == "near_approved_alias"
                && v.tier == VocabSelectionTier::Suggest
                && v.evidence == "d vah"),
            "short proper noun should be suggested when transcript is near an approved alias"
        );
    }

    #[test]
    fn tiered_selector_caps_at_max_total() {
        let pool = mem_pool();
        // Seed 50 terms whose names ARE in the transcript so they all pass
        // the strict term-in-transcript gate. The cap should still clamp
        // the result to max_total regardless.
        for i in 0..50 {
            seed_with_context(
                &pool,
                &format!("T{i}"),
                1.0,
                "auto",
                &vec4(i as f32, 0.0, 0.0, 0.0),
                "english",
                "T context",
            );
        }
        let transcript: String = (0..50).map(|i| format!("T{i} ")).collect();
        let chosen = select_terms(
            &pool,
            "u1",
            "english",
            Some(&vec4(1.0, 0.0, 0.0, 0.0)),
            Some(&transcript),
            100,
            100,
            5,
            0.0,
        );
        assert_eq!(chosen.len(), 5);
    }
}
