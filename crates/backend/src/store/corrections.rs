//! Word-level correction rules extracted from user edits.
//!
//! These are always loaded in full at polish-time (never more than a few dozen
//! rows) and injected as hard substitution rules into the prompt.  No embeddings
//! needed — every correction applies every time.

use rusqlite::params;
use tracing::info;

use super::{DbPool, now_ms};

/// A single word substitution rule.
#[derive(Debug, Clone)]
pub struct Correction {
    pub wrong: String,
    pub right: String,
    pub count: i64,
}

/// Extract word-level diffs from an AI output / user-kept pair.
///
/// Only produces entries when the two texts have the same word count (simple
/// positional alignment).  Strips trailing punctuation so "badhiya." matches
/// "badhiya".  Returns `(wrong, correct)` pairs.
pub fn extract_diffs(ai_output: &str, user_kept: &str) -> Vec<(String, String)> {
    let ai_words: Vec<&str> = ai_output.split_whitespace().collect();
    let kept_words: Vec<&str> = user_kept.split_whitespace().collect();

    if ai_words.len() != kept_words.len() || ai_words.is_empty() {
        return vec![];
    }

    let mut diffs = Vec::new();
    for (a, k) in ai_words.iter().zip(kept_words.iter()) {
        let a_clean = a
            .trim_matches(|c: char| c.is_ascii_punctuation())
            .to_lowercase();
        let k_clean = k
            .trim_matches(|c: char| c.is_ascii_punctuation())
            .to_lowercase();
        if a_clean.is_empty() || k_clean.is_empty() {
            continue;
        }
        if a_clean != k_clean {
            diffs.push((a_clean, k_clean));
        }
    }
    diffs
}

/// Upsert word corrections for a user.  If the same `wrong_text` already exists,
/// bump the count and update `correct_text` to the latest preference.
pub fn upsert(pool: &DbPool, user_id: &str, diffs: &[(String, String)]) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let now = now_ms();
    for (wrong, correct) in diffs {
        let _ = conn.execute(
            "INSERT INTO word_corrections (user_id, wrong_text, correct_text, count, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT(user_id, wrong_text) DO UPDATE SET
                correct_text = excluded.correct_text,
                count = count + 1,
                updated_at = excluded.updated_at",
            params![user_id, wrong, correct, now],
        );
    }
}

/// Filter corrections to only those relevant to the current transcript.
///
/// A correction is relevant when its `wrong` text appears in the transcript
/// (case-insensitive whole-word match or phonetic similarity ≥ 0.70).
/// Also requires `count >= min_count` to filter out accidental one-off edits.
/// Returns at most `max_results` corrections, ordered by count descending.
pub fn filter_relevant(
    all: &[Correction],
    transcript: &str,
    min_count: i64,
    max_results: usize,
) -> Vec<Correction> {
    use crate::llm::phonetics;

    let transcript_lower = transcript.to_ascii_lowercase();
    let transcript_tokens: Vec<&str> = transcript_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    let mut relevant: Vec<Correction> = all
        .iter()
        .filter(|c| c.count >= min_count)
        .filter(|c| {
            let wrong_lower = c.wrong.to_ascii_lowercase();
            // Exact whole-word match
            if transcript_tokens.iter().any(|tok| *tok == wrong_lower) {
                return true;
            }
            // Multi-word correction: check if all words appear in sequence
            let wrong_words: Vec<&str> = wrong_lower.split_whitespace().collect();
            if wrong_words.len() > 1 {
                let found = transcript_tokens
                    .windows(wrong_words.len())
                    .any(|window| window == wrong_words.as_slice());
                if found {
                    return true;
                }
            }
            // Phonetic fallback
            transcript_tokens.iter().any(|tok| {
                phonetics::similarity(tok, &wrong_lower) >= 0.70
            })
        })
        .cloned()
        .collect();

    relevant.sort_by(|a, b| b.count.cmp(&a.count));
    relevant.truncate(max_results);
    relevant
}

/// Load every correction for a user (always small — tens of rows at most).
pub fn load_all(pool: &DbPool, user_id: &str) -> Vec<Correction> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut stmt = match conn.prepare(
        "SELECT wrong_text, correct_text, count
           FROM word_corrections
          WHERE user_id = ?1
          ORDER BY count DESC",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id], |row| {
        Ok(Correction {
            wrong: row.get(0)?,
            right: row.get(1)?,
            count: row.get(2)?,
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Backfill word_corrections from existing edit_events (runs once at startup).
pub fn backfill_from_edit_events(pool: &DbPool) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM word_corrections", [], |r| r.get(0))
        .unwrap_or(0);
    if existing > 0 {
        return; // already backfilled
    }

    let mut stmt = match conn.prepare(
        "SELECT ee.ai_output, ee.user_kept, lu.id
           FROM edit_events ee
           JOIN local_user lu ON lu.id = ee.user_id",
    ) {
        Ok(s) => s,
        Err(_) => return,
    };
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .ok()
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    drop(stmt);
    drop(conn);

    let mut total = 0usize;
    for (ai, kept, uid) in &rows {
        let diffs = extract_diffs(ai, kept);
        if !diffs.is_empty() {
            upsert(pool, uid, &diffs);
            total += diffs.len();
        }
    }
    if total > 0 {
        info!("[corrections] backfilled {total} word correction(s) from edit_events");
    }
}
