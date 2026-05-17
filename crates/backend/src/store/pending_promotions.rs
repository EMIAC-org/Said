//! Pending-promotions queue — implements the k-event promotion gate.
//!
//! Why this exists: WisperFlow's documented bloat problem ("trim the
//! dictionary manually") is the textbook outcome of single-event promotion.
//! Their docs literally tell users that hundreds of dictionary entries
//! degrade accuracy.  We avoid that by requiring **k ≥ 2 confirming sightings**
//! with phonetic agreement before a term reaches the live `vocabulary` /
//! `stt_replacements` tables.
//!
//! Flow:
//!   1. Classifier fires STT_ERROR / POLISH_ERROR + a candidate term.
//!   2. We call `record_sighting()` here.  It:
//!        • inserts a new row at sighting_count = 1, OR
//!        • bumps an existing row's sighting_count + last_seen.
//!      Then it returns a `PromotionDecision`:
//!        • `Pending` — sighting_count < k; do NOT promote yet.
//!        • `Promote` — sighting_count ≥ k; the route promotes.
//!   3. After successful promotion, the route deletes the pending row.
//!
//! Phonetic-key agreement: a second sighting with a *different* phonetic
//! key (e.g. user typed "Aiden" once, then "Eden" — different sounds)
//! resets the count rather than promoting.  Otherwise typos in the
//! correction itself (the user mistyping their own correction) would
//! masquerade as confirmations.

use rusqlite::params;
use tracing::info;

use super::{DbPool, now_ms};
use crate::llm::phonetics;

/// Default k.  Lower = faster learning + more false positives.  Higher =
/// slower learning + fewer false positives.  Raised from 2 to 3 after user
/// reports of repeated typos getting promoted (two identical typos shouldn't
/// be enough to learn a term).
pub const DEFAULT_K: i64 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum PromotionDecision {
    /// Not enough confirming sightings yet.  Caller should NOT promote.
    Pending { sighting_count: i64 },
    /// Confirming sightings reached k; caller should promote and then
    /// call `delete()` to clear the pending row.
    Promote { sighting_count: i64 },
}

/// Record a new sighting of `(correct_form, transcript_form)` in
/// `output_language`.  Returns whether this sighting tips the term over
/// the k threshold (Promote) or not (Pending).
///
/// Phonetic key matching: if a row already exists with a *different*
/// phonetic key for the same correct_form, we treat it as a different
/// term entirely — overwrite the row at count = 1.
pub fn record_sighting(
    pool: &DbPool,
    user_id: &str,
    correct_form: &str,
    transcript_form: &str,
    output_language: &str,
    k: i64,
) -> Option<PromotionDecision> {
    let conn = pool.get().ok()?;
    let correct = correct_form.trim();
    if correct.is_empty() {
        return None;
    }
    let now = now_ms();
    let phon_key = phonetics::phonetic_key(correct);

    const STALE_MS: i64 = 14 * 24 * 60 * 60 * 1000;

    let existing: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT sighting_count, phonetic_key, last_seen
               FROM pending_promotions
              WHERE user_id = ?1 AND correct_form = ?2 AND output_language = ?3",
            params![user_id, correct, output_language],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();

    let (new_count, was_reset) = match existing {
        None => {
            conn.execute(
                "INSERT INTO pending_promotions
                   (user_id, correct_form, transcript_form, phonetic_key,
                    output_language, sighting_count, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                params![
                    user_id,
                    correct,
                    transcript_form.trim(),
                    phon_key,
                    output_language,
                    now
                ],
            )
            .ok()?;
            (1_i64, false)
        }
        Some((count, existing_key, last_seen)) if existing_key == phon_key => {
            let base = if (now - last_seen) > STALE_MS { 0 } else { count };
            let next = base + 1;
            conn.execute(
                "UPDATE pending_promotions
                    SET sighting_count = ?4, last_seen = ?5, transcript_form = ?6
                  WHERE user_id = ?1 AND correct_form = ?2 AND output_language = ?3",
                params![
                    user_id,
                    correct,
                    output_language,
                    next,
                    now,
                    transcript_form.trim()
                ],
            )
            .ok()?;
            (next, false)
        }
        Some((_count, _other_key, _last_seen)) => {
            // Phonetic key drift — treat as a fresh term, reset the count.
            conn.execute(
                "UPDATE pending_promotions
                    SET sighting_count = 1, first_seen = ?5, last_seen = ?5,
                        transcript_form = ?6, phonetic_key = ?7
                  WHERE user_id = ?1 AND correct_form = ?2 AND output_language = ?3",
                params![
                    user_id,
                    correct,
                    output_language,
                    /*placeholder*/ 0,
                    now,
                    transcript_form.trim(),
                    phon_key
                ],
            )
            .ok()?;
            (1_i64, true)
        }
    };

    let decision = if new_count >= k {
        PromotionDecision::Promote {
            sighting_count: new_count,
        }
    } else {
        PromotionDecision::Pending {
            sighting_count: new_count,
        }
    };
    info!(
        "[pending-promo] sighting term={correct:?} lang={output_language:?} count={new_count} reset={was_reset} k={k} → {decision:?}",
    );
    Some(decision)
}

/// Drop the pending row after a successful promotion.  Idempotent.
pub fn delete(pool: &DbPool, user_id: &str, correct_form: &str, output_language: &str) {
    let Ok(conn) = pool.get() else {
        return;
    };
    let _ = conn.execute(
        "DELETE FROM pending_promotions
          WHERE user_id = ?1 AND correct_form = ?2 AND output_language = ?3",
        params![user_id, correct_form.trim(), output_language],
    );
}

/// Drop EVERY pending row for a `correct_form`, across all languages. Called
/// from the vocabulary delete path: when a user removes a term explicitly,
/// any queued promotion sightings for it should die too — otherwise a
/// future K-event tick could resurrect the deleted term silently.
pub fn delete_all_for_term(pool: &DbPool, user_id: &str, correct_form: &str) -> usize {
    let Ok(conn) = pool.get() else {
        return 0;
    };
    conn.execute(
        "DELETE FROM pending_promotions
          WHERE user_id = ?1 AND correct_form = ?2",
        params![user_id, correct_form.trim()],
    )
    .unwrap_or(0)
}

/// Aging: drop pending rows older than `max_age_ms` (e.g. 30 days).  Called
/// periodically by the route or a background tick.
pub fn prune_stale(pool: &DbPool, user_id: &str, max_age_ms: i64) -> usize {
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let cutoff = now_ms() - max_age_ms;
    conn.execute(
        "DELETE FROM pending_promotions
          WHERE user_id = ?1 AND last_seen < ?2",
        params![user_id, cutoff],
    )
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE pending_promotions (
                 user_id          TEXT NOT NULL REFERENCES local_user(id),
                 correct_form     TEXT NOT NULL,
                 transcript_form  TEXT NOT NULL,
                 phonetic_key     TEXT NOT NULL,
                 output_language  TEXT NOT NULL,
                 sighting_count   INTEGER NOT NULL DEFAULT 1,
                 first_seen       INTEGER NOT NULL,
                 last_seen        INTEGER NOT NULL,
                 UNIQUE(user_id, correct_form, output_language)
             );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn first_sighting_is_pending() {
        let p = mem_pool();
        let d = record_sighting(&p, "u1", "n8n", "written", "english", 2).unwrap();
        assert_eq!(d, PromotionDecision::Pending { sighting_count: 1 });
    }

    #[test]
    fn second_sighting_promotes_at_k_2() {
        let p = mem_pool();
        record_sighting(&p, "u1", "n8n", "written", "english", 2).unwrap();
        let d = record_sighting(&p, "u1", "n8n", "written", "english", 2).unwrap();
        assert_eq!(d, PromotionDecision::Promote { sighting_count: 2 });
    }

    #[test]
    fn third_sighting_at_k_3_still_pending() {
        let p = mem_pool();
        record_sighting(&p, "u1", "n8n", "written", "english", 3).unwrap();
        record_sighting(&p, "u1", "n8n", "written", "english", 3).unwrap();
        let d = record_sighting(&p, "u1", "n8n", "written", "english", 3).unwrap();
        assert_eq!(d, PromotionDecision::Promote { sighting_count: 3 });
    }

    #[test]
    fn different_language_buckets_separately() {
        let p = mem_pool();
        record_sighting(&p, "u1", "n8n", "written", "english", 2).unwrap();
        let d = record_sighting(&p, "u1", "n8n", "written", "hinglish", 2).unwrap();
        // The hinglish row is a fresh sighting — count 1 — even though
        // english is at 1 already.
        assert_eq!(d, PromotionDecision::Pending { sighting_count: 1 });
    }

    #[test]
    fn delete_clears_the_row() {
        let p = mem_pool();
        record_sighting(&p, "u1", "n8n", "written", "english", 2).unwrap();
        delete(&p, "u1", "n8n", "english");
        let d = record_sighting(&p, "u1", "n8n", "written", "english", 2).unwrap();
        assert_eq!(d, PromotionDecision::Pending { sighting_count: 1 });
    }

    #[test]
    fn delete_all_for_term_wipes_every_language() {
        // Regression: when the user deletes a vocab term, queued promotion
        // sightings for that correct_form must die across ALL languages —
        // otherwise a later K-event in any locale could resurrect the
        // deleted term silently.
        let p = mem_pool();
        record_sighting(&p, "u1", "MACOBS", "main corps", "english", 2).unwrap();
        record_sighting(&p, "u1", "MACOBS", "main corps", "hinglish", 2).unwrap();
        record_sighting(&p, "u1", "OTHER", "uthhrr", "english", 2).unwrap();

        let n = delete_all_for_term(&p, "u1", "MACOBS");
        assert_eq!(n, 2, "both MACOBS rows (english + hinglish) removed");

        // Re-sighting MACOBS in either language starts from count=1.
        let d = record_sighting(&p, "u1", "MACOBS", "main corps", "english", 2).unwrap();
        assert_eq!(d, PromotionDecision::Pending { sighting_count: 1 });
        // Unrelated term is untouched.
        let d2 = record_sighting(&p, "u1", "OTHER", "uthhrr", "english", 2).unwrap();
        assert_eq!(
            d2,
            PromotionDecision::Promote { sighting_count: 2 },
            "unrelated pending row must survive"
        );
    }

    #[test]
    fn prune_stale_drops_old_rows() {
        let p = mem_pool();
        record_sighting(&p, "u1", "ancient", "old", "english", 2).unwrap();
        // Manually backdate the row past the cutoff.
        p.get()
            .unwrap()
            .execute(
                "UPDATE pending_promotions SET last_seen = 0 WHERE correct_form = 'ancient'",
                params![],
            )
            .unwrap();
        let pruned = prune_stale(&p, "u1", 1000);
        assert_eq!(pruned, 1);
    }
}
