//! Post-STT replacement store.
//!
//! When STT keeps emitting a wrong form for a term, biasing alone isn't enough
//! — we also rewrite the transcript before it reaches the LLM polish step.
//!
//! Each row stores `(transcript_form → correct_form)` plus a phonetic key on
//! the transcript_form.  Application is two-pass:
//!   1. Exact (case-insensitive) whole-word match on `transcript_form` → swap.
//!   2. (Optional) fuzzy match: for each transcript token, compute its phonetic
//!      key and look up by `phonetic_key`; require similarity ≥ threshold.
//!
//! The phonetic pass catches small STT variations ("aiden" / "aidan" / "ate-n")
//! that the exact pass would miss without exploding the table size.

use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{DbPool, now_ms};
use crate::llm::phonetics;

#[derive(Debug, Clone, Serialize)]
pub struct SttReplacement {
    pub transcript_form: String,
    pub correct_form: String,
    pub phonetic_key: String,
    pub weight: f64,
    pub use_count: i64,
    pub last_used: i64,
    pub language: Option<String>,
    pub export_tier: ExportTier,
    pub contradiction_count: i64,
    pub review_status: ReviewStatus,
    pub review_reason: Option<String>,
    pub last_reviewed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExportTier {
    LocalOnly,
    ExportKeytermSupport,
    ExportReplaceReady,
    Blocked,
}

impl ExportTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local_only",
            Self::ExportKeytermSupport => "export_keyterm_support",
            Self::ExportReplaceReady => "export_replace_ready",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "export_keyterm_support" => Self::ExportKeytermSupport,
            "export_replace_ready" => Self::ExportReplaceReady,
            "blocked" => Self::Blocked,
            _ => Self::LocalOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Blocked,
    Skipped,
}

impl ReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "approved" => Self::Approved,
            "blocked" => Self::Blocked,
            "skipped" => Self::Skipped,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    Phonetic,
    Tier2Policy,
    Tier2EditPolicy,
    Tier2ClusterFuzzy,
    Tier2Deterministic,
    Tier2Onnx,
    Tier2Context,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Phonetic => "phonetic",
            Self::Tier2Policy => "tier2_policy",
            Self::Tier2EditPolicy => "tier2_edit_policy",
            Self::Tier2ClusterFuzzy => "tier2_cluster_fuzzy",
            Self::Tier2Deterministic => "tier2_deterministic",
            Self::Tier2Onnx => "tier2_onnx",
            Self::Tier2Context => "tier2_context",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMatch {
    pub transcript_form: String,
    pub correct_form: String,
    pub kind: MatchKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoringTrace {
    pub token: String,
    pub candidate: String,
    pub second_candidate: Option<String>,
    pub kind: MatchKind,
    pub score: f64,
    pub second_score: f64,
    pub margin: f64,
    pub deterministic_score: f64,
    pub onnx_score: Option<f64>,
    pub policy_score: Option<f64>,
    pub policy_boost: f64,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyResult {
    pub text: String,
    pub matches: Vec<AppliedMatch>,
    pub traces: Vec<ScoringTrace>,
}

/// Upsert a (transcript_form → correct_form) replacement rule.
///
/// Same-pair repeats bump weight + use_count; conflicting pairs (same
/// transcript_form, different correct_form) coexist as separate rows so we
/// can see the distribution and pick the highest-weight one at apply time.
///
/// Each row in `stt_replacements` is one **alias** for the canonical
/// `correct_form`. A canonical can have many aliases — different shapes STT
/// has been observed producing for the same intended utterance. Aliases are
/// learned from BOTH the polish span (what the LLM thought STT said) AND
/// the raw transcript span (what STT actually emitted) — see
/// `upsert_aliases` in this module for the foundational caller.
///
/// Apply (`apply()` below) does longest-match phrase replacement, so a
/// multi-word alias will match a multi-word transcript span and a
/// single-word alias matches a single token. No language-specific stopword
/// list, no fragmenting heuristic — just learn the actual STT output shape
/// and match against it directly.
pub fn upsert(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
    bump: f64,
) -> bool {
    upsert_inner(pool, user_id, transcript_form, correct_form, bump, None)
}

pub fn upsert_with_language(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
    bump: f64,
    language: &str,
) -> bool {
    upsert_inner(
        pool,
        user_id,
        transcript_form,
        correct_form,
        bump,
        Some(language),
    )
}

fn upsert_inner(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
    bump: f64,
    language: Option<&str>,
) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "upsert",
            "stt_replacements::upsert_inner",
        );
        return false;
    }
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let from = transcript_form.trim().to_ascii_lowercase();
    let to = correct_form.trim().to_string();
    if from.is_empty() || to.is_empty() {
        return false;
    }
    // Don't learn no-op rules (alias == canonical, case-insensitive ASCII).
    if from.is_ascii() && to.is_ascii() && from == to.to_ascii_lowercase() {
        return false;
    }
    if crate::llm::alias_safety::is_common_alias_source(&from) {
        tracing::info!("[stt-replace] rejected common alias source: {from:?} → {to:?}");
        return false;
    }
    if !is_plausible_alias(&from, &to) {
        tracing::info!("[stt-replace] rejected implausible alias: {from:?} → {to:?}");
        return false;
    }
    let key = phonetics::phonetic_key(&from);
    let now = now_ms();
    let rows = conn
        .execute(
            "INSERT INTO stt_replacements
            (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used, language,
             export_tier, contradiction_count, review_status)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, 'local_only', 0, 'pending')
         ON CONFLICT(user_id, transcript_form, correct_form) DO UPDATE SET
            weight    = MIN(5.0, weight + ?5),
            use_count = use_count + 1,
            last_used = excluded.last_used,
            language  = COALESCE(excluded.language, stt_replacements.language),
            review_status = CASE
                              WHEN stt_replacements.review_status = 'blocked' THEN 'blocked'
                              ELSE 'pending'
                            END",
            params![user_id, from, to, key, bump, now, language],
        )
        .unwrap_or(0);

    info!("[stt-repl] upsert {from:?} → {to:?} (key={key}, bump={bump}, rows={rows})");
    if rows > 0 {
        drop(conn);
        enforce_alias_cap(pool, user_id, correct_form);
    }
    rows > 0
}

pub fn is_plausible_alias(transcript_form: &str, correct_form: &str) -> bool {
    let from = transcript_form.trim().to_ascii_lowercase();
    let to = correct_form.trim().to_ascii_lowercase();
    if from.is_empty() || to.is_empty() {
        return false;
    }
    if from.is_ascii() && to.is_ascii() && from == to {
        return false;
    }

    let to_compact = compact_ascii(&to);
    if to_compact.is_empty() {
        return true;
    }

    // Code identifiers with digits (n8n, k8s, s3) cannot be phonetically
    // matched — STT produces completely unrelated words for these. Allow them.
    let target_has_digit = to.chars().any(|c| c.is_ascii_digit());
    if target_has_digit {
        return true;
    }

    // Gibberish source words (not in any dictionary) are genuine STT
    // distortions — Deepgram made up a nonsense word. Allow these
    // regardless of phonetic similarity. The real gate for "mac" → "EMIAC"
    // is `is_in_dictionary("mac")` in the caller, not phonetic similarity.
    if from.is_ascii()
        && !from.contains(' ')
        && !crate::tier2::is_in_dictionary(&from)
        && !crate::llm::alias_safety::is_common_alias_source(&from)
    {
        return true;
    }

    let ascii_tokens = ascii_alias_tokens(&from);
    if from.is_ascii() && to.is_ascii() {
        if ascii_tokens.len() <= 1 && !from.contains(' ') && !to.contains(' ') {
            let phon_sim = phonetics::similarity(&from, &to);
            let first_match = from.chars().next().map(|f| f.to_ascii_lowercase())
                == to.chars().next().map(|t| t.to_ascii_lowercase());
            return phon_sim >= 0.45 || (first_match && phon_sim >= 0.35);
        }
        let (best_sim, first_match) = alias_phrase_score(&ascii_tokens, &to_compact);
        return best_sim >= 0.40 || (first_match && best_sim >= 0.30);
    }

    // Mixed-script aliases are common because Deepgram can emit Devanagari
    // around an English proper noun ("मैं Corps" → "MACOBS"). Keep those
    // allowed; the unrelated-alias pollution we are blocking here is the
    // pure ASCII multi-word case ("urban aura" → "Macobs").
    if !from.is_ascii() && to.is_ascii() {
        return true;
    }

    true
}

fn ascii_alias_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn compact_ascii(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn alias_phrase_score(tokens: &[String], correct_compact: &str) -> (f64, bool) {
    let content_tokens = tokens
        .iter()
        .filter(|token| !is_alias_glue_word(token))
        .cloned()
        .collect::<Vec<_>>();
    let compared = if content_tokens.is_empty() {
        tokens.to_vec()
    } else {
        content_tokens
    };
    let compact_all = tokens.join("");
    let compact_content = compared.join("");
    let best_sim = std::iter::once(compact_all.as_str())
        .chain(std::iter::once(compact_content.as_str()))
        .chain(compared.iter().map(String::as_str))
        .filter(|token| !token.is_empty())
        .map(|token| phonetics::similarity(token, correct_compact))
        .fold(0.0_f64, f64::max);
    let first_match = compared.iter().any(|token| {
        token.chars().next().map(|f| f.to_ascii_lowercase())
            == correct_compact
                .chars()
                .next()
                .map(|t| t.to_ascii_lowercase())
    });
    (best_sim, first_match)
}

fn is_alias_glue_word(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "aur"
            | "hai"
            | "hain"
            | "he"
            | "hi"
            | "ka"
            | "ke"
            | "ki"
            | "ko"
            | "main"
            | "mein"
            | "me"
            | "the"
            | "to"
            | "with"
    )
}

const MAX_ALIASES_PER_TERM: usize = 15;

fn enforce_alias_cap(pool: &DbPool, user_id: &str, correct_form: &str) {
    let Ok(conn) = pool.get() else { return };
    let canon = correct_form.trim();
    if canon.is_empty() {
        return;
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM stt_replacements \
             WHERE user_id = ?1 AND LOWER(correct_form) = LOWER(?2)",
            params![user_id, canon],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if count as usize <= MAX_ALIASES_PER_TERM {
        return;
    }
    let evicted = conn
        .execute(
            "DELETE FROM stt_replacements \
             WHERE user_id = ?1 AND LOWER(correct_form) = LOWER(?2) \
               AND rowid NOT IN ( \
                   SELECT rowid FROM stt_replacements \
                   WHERE user_id = ?1 AND LOWER(correct_form) = LOWER(?2) \
                   ORDER BY use_count DESC, weight DESC, last_used DESC \
                   LIMIT ?3 \
               )",
            params![user_id, canon, MAX_ALIASES_PER_TERM as i64],
        )
        .unwrap_or(0);
    if evicted > 0 {
        info!("[stt-repl] evicted {evicted} alias(es) for {canon:?} (cap={MAX_ALIASES_PER_TERM})");
    }
}

/// Upsert all known aliases for a canonical correct_form in one shot.
///
/// This is the foundational learning entrypoint. Given the (transcript_window,
/// polish_window) pair from a learning event, it stores BOTH spans as aliases
/// for the canonical. Why both:
///
///   • `polish_window` — what the LLM polish step output for the misheard
///     region. Future runs that go through identical polish behaviour will
///     match this exact form.
///
///   • `transcript_window` — what Deepgram (or any STT) actually emitted.
///     Future runs in which polish behaves differently — or in which we
///     bypass polish entirely (debug, alt models) — will match this.
///
/// Both spans are stored at full weight; the longest-match phrase apply
/// handles overlap correctly (the longer alias wins per starting position).
///
/// `transcript_window` may be empty (positional alignment failed in the
/// diff stage) — we just skip it in that case.
pub fn upsert_aliases(
    pool: &DbPool,
    user_id: &str,
    transcript_window: &str,
    polish_window: &str,
    correct_form: &str,
    bump: f64,
) -> usize {
    upsert_aliases_for_language(
        pool,
        user_id,
        transcript_window,
        polish_window,
        correct_form,
        bump,
        "",
    )
}

pub fn upsert_aliases_for_language(
    pool: &DbPool,
    user_id: &str,
    transcript_window: &str,
    polish_window: &str,
    correct_form: &str,
    bump: f64,
    language: &str,
) -> usize {
    let mut written = 0;
    let lang = if language.trim().is_empty() {
        None
    } else {
        Some(language)
    };
    if !polish_window.trim().is_empty()
        && upsert_inner(pool, user_id, polish_window, correct_form, bump, lang)
    {
        written += 1;
    }
    if !transcript_window.trim().is_empty()
        && transcript_window.trim() != polish_window.trim()
        && upsert_inner(pool, user_id, transcript_window, correct_form, bump, lang)
    {
        written += 1;
    }
    written
}

/// Decrement weight on revert; delete when ≤ 0.
pub fn demote(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
    penalty: f64,
) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "demote",
            "stt_replacements::demote",
        );
        return false;
    }
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let from = transcript_form.trim().to_ascii_lowercase();
    let to = correct_form.trim();
    if from.is_empty() || to.is_empty() {
        return false;
    }
    let updated = conn
        .execute(
            "UPDATE stt_replacements SET weight = weight - ?3
           WHERE user_id = ?1 AND transcript_form = ?2 AND correct_form = ?4",
            params![user_id, from, penalty, to],
        )
        .unwrap_or(0);
    if updated == 0 {
        return false;
    }

    // When weight drops to zero, block the alias permanently instead of
    // deleting it.  A blocked row survives in the DB so apply_exact_safe
    // (review_status != Approved) skips it and the classifier's
    // `is_safe_distortion` check sees it too — preventing re-learning.
    let _ = conn.execute(
        "UPDATE stt_replacements
            SET review_status = 'blocked',
                review_reason = 'demoted by user correction',
                weight = 0.0
          WHERE user_id = ?1
            AND transcript_form = ?2
            AND correct_form = ?3
            AND weight <= 0.0",
        params![user_id, from, to],
    );
    info!("[stt-repl] demote {from:?} → {to:?} penalty={penalty}");
    true
}

/// Drop every alias whose `correct_form` matches the given canonical (case-
/// insensitive). Called from the vocabulary delete path so a removed vocab
/// term can no longer fire as a pre-polish substitution. Without this, the
/// raw STT layer would keep rewriting "main corps" → "MACOBS" even after
/// the user explicitly deleted MACOBS from their vocabulary.
///
/// Returns the number of rows removed (for logging / regression assertions).
pub fn delete_by_correct_form(pool: &DbPool, user_id: &str, correct_form: &str) -> usize {
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let canon = correct_form.trim();
    if canon.is_empty() {
        return 0;
    }
    let n = conn
        .execute(
            "DELETE FROM stt_replacements
          WHERE user_id = ?1
            AND lower(correct_form) = lower(?2)",
            params![user_id, canon],
        )
        .unwrap_or(0);
    if n > 0 {
        info!("[stt-repl] cleared {n} alias(es) pointing at {canon:?}");
    }
    n
}

/// Delete aliases where the transcript_form matches `from` and correct_form
/// matches `to`. Used when the user reverts a wrong replacement — e.g. system
/// typed "Macobs" but user wanted "access", so we delete any alias that maps
/// "access" → "Macobs".
pub fn delete_alias_pair(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
) -> usize {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "delete_alias_pair",
            "stt_replacements::delete_alias_pair",
        );
        return 0;
    }
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let from = transcript_form.trim();
    let to = correct_form.trim();
    if from.is_empty() || to.is_empty() {
        return 0;
    }
    let n = conn
        .execute(
            "DELETE FROM stt_replacements
              WHERE user_id = ?1
                AND lower(transcript_form) = lower(?2)
                AND lower(correct_form) = lower(?3)",
            params![user_id, from, to],
        )
        .unwrap_or(0);
    if n > 0 {
        info!(
            "[stt-repl] deleted {n} alias(es) for pair {:?} → {:?}",
            from, to
        );
    }
    n
}

/// Load all replacements for a user.  Always small (tens of rows); we apply
/// them in a single linear pass over the transcript.
pub fn load_all(pool: &DbPool, user_id: &str) -> Vec<SttReplacement> {
    load_for_language(pool, user_id, "")
}

pub fn load_for_language(pool: &DbPool, user_id: &str, language: &str) -> Vec<SttReplacement> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let sql = if language.trim().is_empty() {
        "SELECT transcript_form, correct_form, phonetic_key, weight, use_count, last_used,
                language, export_tier, contradiction_count, review_status, review_reason, last_reviewed_at
           FROM stt_replacements
          WHERE user_id = ?1
          ORDER BY weight DESC"
    } else {
        "SELECT transcript_form, correct_form, phonetic_key, weight, use_count, last_used,
                language, export_tier, contradiction_count, review_status, review_reason, last_reviewed_at
           FROM stt_replacements
          WHERE user_id = ?1
            AND (language = ?2 OR language IS NULL)
          ORDER BY weight DESC"
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    if language.trim().is_empty() {
        stmt.query_map(params![user_id], |row| {
            Ok(SttReplacement {
                transcript_form: row.get(0)?,
                correct_form: row.get(1)?,
                phonetic_key: row.get(2)?,
                weight: row.get(3)?,
                use_count: row.get(4)?,
                last_used: row.get(5)?,
                language: row.get(6).ok(),
                export_tier: ExportTier::parse(&row.get::<_, String>(7)?),
                contradiction_count: row.get(8)?,
                review_status: ReviewStatus::parse(&row.get::<_, String>(9)?),
                review_reason: row.get(10).ok(),
                last_reviewed_at: row.get(11).ok(),
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
        .unwrap_or_default()
    } else {
        stmt.query_map(params![user_id, language], |row| {
            Ok(SttReplacement {
                transcript_form: row.get(0)?,
                correct_form: row.get(1)?,
                phonetic_key: row.get(2)?,
                weight: row.get(3)?,
                use_count: row.get(4)?,
                last_used: row.get(5)?,
                language: row.get(6).ok(),
                export_tier: ExportTier::parse(&row.get::<_, String>(7)?),
                contradiction_count: row.get(8)?,
                review_status: ReviewStatus::parse(&row.get::<_, String>(9)?),
                review_reason: row.get(10).ok(),
                last_reviewed_at: row.get(11).ok(),
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
        .unwrap_or_default()
    }
}

pub fn get_for_language(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
    language: &str,
) -> Option<SttReplacement> {
    let conn = pool.get().ok()?;
    let from = transcript_form.trim().to_ascii_lowercase();
    let to = correct_form.trim();
    if from.is_empty() || to.is_empty() {
        return None;
    }
    conn.query_row(
        "SELECT transcript_form, correct_form, phonetic_key, weight, use_count, last_used,
                language, export_tier, contradiction_count, review_status, review_reason, last_reviewed_at
           FROM stt_replacements
          WHERE user_id = ?1
            AND transcript_form = ?2
            AND correct_form = ?3
            AND (language = ?4 OR (?4 = '' AND language IS NULL) OR (?4 != '' AND language IS NULL))
          ORDER BY CASE WHEN language = ?4 THEN 0 ELSE 1 END
          LIMIT 1",
        params![user_id, from, to, language],
        |row| {
            Ok(SttReplacement {
                transcript_form: row.get(0)?,
                correct_form: row.get(1)?,
                phonetic_key: row.get(2)?,
                weight: row.get(3)?,
                use_count: row.get(4)?,
                last_used: row.get(5)?,
                language: row.get(6).ok(),
                export_tier: ExportTier::parse(&row.get::<_, String>(7)?),
                contradiction_count: row.get(8)?,
                review_status: ReviewStatus::parse(&row.get::<_, String>(9)?),
                review_reason: row.get(10).ok(),
                last_reviewed_at: row.get(11).ok(),
            })
        },
    )
    .ok()
}

pub fn update_export_metadata(
    pool: &DbPool,
    user_id: &str,
    transcript_form: &str,
    correct_form: &str,
    export_tier: ExportTier,
    review_status: ReviewStatus,
    review_reason: Option<&str>,
    language: &str,
) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "update_export_metadata",
            "stt_replacements::update_export_metadata",
        );
        return false;
    }
    let Ok(conn) = pool.get() else {
        return false;
    };
    let now = now_ms();
    conn.execute(
        "UPDATE stt_replacements
            SET export_tier = ?5,
                review_status = ?6,
                review_reason = ?7,
                last_reviewed_at = ?8
          WHERE user_id = ?1
            AND transcript_form = ?2
            AND correct_form = ?3
            AND (language = ?4 OR (?4 = '' AND language IS NULL) OR (?4 != '' AND language IS NULL))",
        params![
            user_id,
            transcript_form.trim().to_ascii_lowercase(),
            correct_form.trim(),
            language,
            export_tier.as_str(),
            review_status.as_str(),
            review_reason,
            now
        ],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Approve all pending aliases for a term so they fire immediately in
/// `apply_exact_safe()`.  Called after the alias passes all safety gates
/// during the classify flow — no need to wait for the 15-min background
/// review.
pub fn approve_aliases_for_term(pool: &DbPool, user_id: &str, correct_form: &str) -> usize {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "approve_aliases_for_term",
            "stt_replacements::approve_aliases_for_term",
        );
        return 0;
    }
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let now = now_ms();
    conn.execute(
        "UPDATE stt_replacements
            SET review_status = 'approved',
                last_reviewed_at = ?3
          WHERE user_id = ?1
            AND lower(correct_form) = lower(?2)
            AND review_status = 'pending'",
        params![user_id, correct_form.trim(), now],
    )
    .unwrap_or(0)
}

pub fn note_negative_signal(
    pool: &DbPool,
    user_id: &str,
    correct_form: &str,
    penalty: i64,
) -> usize {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "note_negative_signal",
            "stt_replacements::note_negative_signal",
        );
        return 0;
    }
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let now = now_ms();
    conn.execute(
        "UPDATE stt_replacements
            SET contradiction_count = contradiction_count + ?3,
                last_contradicted_at = ?4,
                export_tier = CASE
                    WHEN contradiction_count + ?3 >= 4 THEN 'blocked'
                    WHEN contradiction_count + ?3 >= 2 THEN 'local_only'
                    WHEN export_tier = 'export_replace_ready' THEN 'export_keyterm_support'
                    ELSE export_tier
                END,
                review_status = CASE
                    WHEN review_status = 'blocked' THEN 'blocked'
                    ELSE 'pending'
                END
          WHERE user_id = ?1
            AND lower(correct_form) = lower(?2)",
        params![user_id, correct_form.trim(), penalty, now],
    )
    .unwrap_or(0)
}

pub fn review_candidates(pool: &DbPool, user_id: &str, limit: usize) -> Vec<SttReplacement> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let mut stmt = match conn.prepare(
        "SELECT transcript_form, correct_form, phonetic_key, weight, use_count, last_used,
                language, export_tier, contradiction_count, review_status, review_reason, last_reviewed_at
           FROM stt_replacements
          WHERE user_id = ?1
            AND (
                review_status = 'pending'
                OR contradiction_count > 0
                OR (last_reviewed_at IS NULL AND export_tier != 'local_only')
            )
          ORDER BY contradiction_count DESC, use_count DESC, weight DESC, last_used DESC
          LIMIT ?2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id, limit as i64], |row| {
        Ok(SttReplacement {
            transcript_form: row.get(0)?,
            correct_form: row.get(1)?,
            phonetic_key: row.get(2)?,
            weight: row.get(3)?,
            use_count: row.get(4)?,
            last_used: row.get(5)?,
            language: row.get(6).ok(),
            export_tier: ExportTier::parse(&row.get::<_, String>(7)?),
            contradiction_count: row.get(8)?,
            review_status: ReviewStatus::parse(&row.get::<_, String>(9)?),
            review_reason: row.get(10).ok(),
            last_reviewed_at: row.get(11).ok(),
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

pub fn note_negative_signals_for_edit(
    pool: &DbPool,
    user_id: &str,
    ai_output: &str,
    user_kept: &str,
) -> usize {
    let ai_lower = ai_output.to_ascii_lowercase();
    let kept_lower = user_kept.to_ascii_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut updated = 0;
    for rule in load_all(pool, user_id) {
        let canonical = rule.correct_form.to_ascii_lowercase();
        if canonical.is_empty() || !ai_lower.contains(&canonical) || kept_lower.contains(&canonical)
        {
            continue;
        }
        if seen.insert(canonical.clone()) {
            updated += note_negative_signal(pool, user_id, &canonical, 1);
        }
    }
    updated
}

/// Apply replacements to a transcript. Pure function; safe to unit-test.
///
/// Algorithm — **longest-match phrase replacement** with phonetic fallback:
///
///   1. Tokenise the transcript into (word, original_chunk) pairs preserving
///      whitespace + punctuation around each word.
///
///   2. Sort rules by `transcript_form` token-count DESC (longest first), so
///      a 3-word alias gets a chance to match before a 1-word alias on the
///      same starting position.
///
///   3. Walk left-to-right. At each cursor position:
///        a. Try the longest rule whose tokens equal the next N tokens
///           (case-insensitive on the WORD CORES — punctuation around them
///           is preserved verbatim from the input). If hit, emit the
///           canonical, advance N tokens, continue.
///        b. Otherwise try a single-token phonetic match (key-based,
///           similarity ≥ 0.85). If hit, emit the canonical for that one
///           token, advance 1, continue.
///        c. Otherwise emit the current chunk verbatim, advance 1.
///
/// This handles BOTH single-word rules ("corps" → "MACOBS") and multi-word
/// rules ("Main corps" → "MACOBS") uniformly — no token-only / phrase-only
/// fork, and no language-specific stopword tricks. Whatever STT span we
/// learned, we match the same span shape.
pub fn apply(transcript: &str, rules: &[SttReplacement]) -> String {
    apply_with_matches(transcript, rules).text
}

pub fn apply_with_matches(transcript: &str, rules: &[SttReplacement]) -> ApplyResult {
    apply_inner(transcript, rules, true)
}

/// Apply ONLY exact whole-word matches, skipping phonetic fallback, and
/// filtering out aliases whose transcript_form is a common Hindi/English
/// word. Safe for pre-LLM substitution where false positives are costly.
pub fn apply_exact_safe(transcript: &str, rules: &[SttReplacement]) -> ApplyResult {
    if rules.is_empty() {
        return ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
    }
    let safe: Vec<SttReplacement> = rules
        .iter()
        .filter(|r| {
            if r.transcript_form.trim().len() < 2 {
                return false;
            }
            if crate::llm::alias_safety::is_common_alias_source(&r.transcript_form)
                || crate::tier2::is_in_dictionary(&r.transcript_form)
            {
                return false;
            }
            if r.review_status != ReviewStatus::Approved {
                return false;
            }
            if !is_plausible_alias(&r.transcript_form, &r.correct_form) {
                return false;
            }
            true
        })
        .cloned()
        .collect();
    apply_inner(transcript, &safe, false)
}

fn apply_inner(transcript: &str, rules: &[SttReplacement], allow_phonetic: bool) -> ApplyResult {
    if rules.is_empty() {
        return ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
    }

    // Pre-compute (word_count, lowercased_words) for each rule so the inner
    // loop is cheap. Sort by word_count DESC so longest-match wins.
    let mut indexed: Vec<(Vec<String>, &SttReplacement)> = rules
        .iter()
        .map(|r| {
            let words: Vec<String> = r
                .transcript_form
                .split_whitespace()
                .map(|w| w.to_ascii_lowercase())
                .collect();
            (words, r)
        })
        .filter(|(w, _)| !w.is_empty())
        .collect();
    indexed.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    // Tokenise the transcript while preserving each token's original chunk
    // (the word + any trailing whitespace and punctuation glued to it).
    let chunks: Vec<&str> = split_chunks(transcript);
    let cores: Vec<String> = chunks
        .iter()
        .map(|c| word_core(c).to_ascii_lowercase())
        .collect();

    let mut out = String::with_capacity(transcript.len());
    let mut matches = Vec::new();
    let mut i = 0;
    while i < chunks.len() {
        // Empty cores (pure-whitespace / pure-punct chunks) can't match anything.
        if cores[i].is_empty() {
            out.push_str(chunks[i]);
            i += 1;
            continue;
        }

        // Try multi-token rules at this position (longest first).
        let mut matched = None;
        for (rule_words, rule) in &indexed {
            let n = rule_words.len();
            if i + n > chunks.len() {
                continue;
            }
            // Compare rule words to the cores starting at i.
            let mut ok = true;
            let mut consumed = 0;
            let mut k = i;
            while consumed < n && k < chunks.len() {
                if cores[k].is_empty() {
                    // Skip pure-punct chunks inside the window — they don't
                    // count against the rule's word index.
                    k += 1;
                    continue;
                }
                if cores[k] != rule_words[consumed] {
                    ok = false;
                    break;
                }
                consumed += 1;
                k += 1;
            }
            if ok && consumed == n {
                matched = Some((rule, k));
                break;
            }
        }

        if let Some((rule, end)) = matched {
            // Replace the matched span with the canonical, preserving the
            // leading punctuation of the first chunk and the trailing
            // whitespace/punct of the last consumed chunk.
            let first = chunks[i];
            let last = chunks[end - 1];
            let (lead, _) = split_punct(first);
            let (_, trail) = split_punct_trailing(last);
            out.push_str(lead);
            out.push_str(&rule.correct_form);
            out.push_str(trail);
            matches.push(AppliedMatch {
                transcript_form: rule.transcript_form.clone(),
                correct_form: rule.correct_form.clone(),
                kind: MatchKind::Exact,
            });
            i = end;
            continue;
        }

        if allow_phonetic {
            // Fall back to single-token phonetic match against transcript_form.
            let chunk = chunks[i];
            let core = &cores[i];
            let key = phonetics::phonetic_key(core);
            let mut phonetic_hit = None;
            if !key.is_empty() {
                for (_, rule) in &indexed {
                    if rule.phonetic_key == key {
                        let sim = phonetics::similarity(core, &rule.transcript_form);
                        if sim >= 0.85 {
                            phonetic_hit = Some(rule);
                            break;
                        }
                    }
                }
            }
            // Second phonetic fallback: match against the CORRECT_FORM.
            // Deepgram distorts the same word differently each time
            // (e.g. "Emiac" → "MEAH" first time, "MEX" second time).
            // We store a rule for (MEAH→Emiac) but next time Deepgram says "MEX"
            // which doesn't match "MEAH" phonetically. However "MEX" DOES
            // phonetically resemble "Emiac" (the correct_form). So we try
            // matching the transcript token against correct_form too.
            //
            // Guards to prevent false positives (e.g. "MACOBS" matching "Emiac"):
            //   • High threshold (0.75) — only very close phonetic matches
            //   • Token must be short relative to correct_form (≤ 1.5x length)
            //     to avoid matching full words against unrelated short rules
            //   • Token must not be longer than correct_form + 2 chars
            if phonetic_hit.is_none() && !key.is_empty() && core.len() >= 3 {
                let mut best_sim = 0.0_f64;
                let mut best_rule = None;
                for (_, rule) in &indexed {
                    let correct_lower = rule.correct_form.to_ascii_lowercase();
                    if *core == correct_lower {
                        continue;
                    }
                    let len_ratio = core.len() as f64 / correct_lower.len().max(1) as f64;
                    if len_ratio > 1.5 || core.len() > correct_lower.len() + 2 {
                        continue;
                    }
                    let correct_sim = phonetics::similarity(core, &rule.correct_form);
                    if correct_sim >= 0.65 && correct_sim > best_sim {
                        best_sim = correct_sim;
                        best_rule = Some(rule);
                    }
                }
                if best_rule.is_some() {
                    phonetic_hit = best_rule;
                }
            }
            if let Some(rule) = phonetic_hit {
                let (lead, trail) = split_punct(chunk);
                let (_, trail2) = split_punct_trailing(trail);
                out.push_str(lead);
                out.push_str(&rule.correct_form);
                out.push_str(trail2);
                matches.push(AppliedMatch {
                    transcript_form: rule.transcript_form.clone(),
                    correct_form: rule.correct_form.clone(),
                    kind: MatchKind::Phonetic,
                });
                i += 1;
                continue;
            }
        }

        out.push_str(chunks[i]);
        i += 1;
    }

    ApplyResult {
        text: out,
        matches,
        traces: vec![],
    }
}

/// Split a transcript into chunks where each chunk is one whitespace-bounded
/// segment INCLUDING the trailing whitespace. Empty chunks are skipped.
fn split_chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Walk to next whitespace
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // Walk through the whitespace
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        out.push(&text[start..i]);
        start = i;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// The "core" word of a chunk: alphanumeric+_+- substring with leading and
/// trailing punctuation stripped. Empty if the chunk has no word characters.
fn word_core(chunk: &str) -> &str {
    let trimmed = chunk.trim();
    let leading = trimmed
        .find(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .unwrap_or(trimmed.len());
    let trailing = trimmed
        .rfind(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .map(|i| i + trimmed[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(trimmed.len());
    if leading >= trailing {
        return "";
    }
    &trimmed[leading..trailing]
}

/// Returns `(leading_punct_or_whitespace, rest)` for a chunk.
fn split_punct(chunk: &str) -> (&str, &str) {
    let split = chunk
        .find(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .unwrap_or(chunk.len());
    (&chunk[..split], &chunk[split..])
}

/// Returns `(rest, trailing_punct_and_whitespace)` for a chunk.
fn split_punct_trailing(chunk: &str) -> (&str, &str) {
    let split = chunk
        .rfind(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .map(|i| i + chunk[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    (&chunk[..split], &chunk[split..])
}

// ── Proactive distortion generation ─────────────────────────────────────

/// Pre-seed likely STT distortions as aliases when a word gets promoted to
/// vocabulary.  This creates 8–12 synthetic aliases at weight 0.6 (lower than
/// organic aliases at 1.0) so the replacement engine can catch common
/// mis-hearings on the very first encounter.
pub fn generate_proactive_distortions(
    pool: &DbPool,
    user_id: &str,
    correct_form: &str,
    observed_transcript: &str,
    language: &str,
) -> usize {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "stt_replacements",
            "generate_proactive_distortions",
            "stt_replacements::generate_proactive_distortions",
        );
        return 0;
    }
    use std::collections::HashSet;

    let base = correct_form.trim().to_ascii_lowercase();
    if base.len() < 2 {
        return 0;
    }

    let mut candidates: HashSet<String> = HashSet::new();

    // Gather from all five strategies
    for d in char_deletions(&base) {
        candidates.insert(d);
    }
    for d in vowel_substitutions(&base) {
        candidates.insert(d);
    }
    for d in adjacent_swaps(&base) {
        candidates.insert(d);
    }
    for d in consonant_softening(&base) {
        candidates.insert(d);
    }
    for d in camel_case_splits(correct_form.trim()) {
        candidates.insert(d);
    }

    // Also insert the observed transcript (lowercased) and its joined-no-spaces form
    let obs = observed_transcript.trim().to_ascii_lowercase();
    if !obs.is_empty() {
        candidates.insert(obs.clone());
        let joined: String = obs.split_whitespace().collect();
        if joined != obs {
            candidates.insert(joined);
        }
    }

    // Filter and cap
    let lang = if language.trim().is_empty() {
        None
    } else {
        Some(language)
    };

    let mut count = 0usize;
    for distortion in candidates
        .iter()
        .filter(|d| is_safe_distortion(d, correct_form.trim()))
        .take(12)
    {
        if upsert_inner(pool, user_id, distortion, correct_form.trim(), 0.6, lang) {
            count += 1;
        }
    }

    enforce_alias_cap(pool, user_id, correct_form.trim());

    info!(
        "[stt-proactive] seeded {count} distortion(s) for {:?}",
        correct_form.trim()
    );
    count
}

/// Produce all single-character deletions of `base` that are >= 3 chars.
fn char_deletions(base: &str) -> Vec<String> {
    let chars: Vec<char> = base.chars().collect();
    let mut results = Vec::new();
    for i in 0..chars.len() {
        let s: String = chars[..i].iter().chain(chars[i + 1..].iter()).collect();
        if s.chars().count() >= 3 {
            results.push(s);
        }
    }
    results
}

/// For each vowel position, substitute with other vowels (a, e, i, o, u).
fn vowel_substitutions(base: &str) -> Vec<String> {
    const VOWELS: &[char] = &['a', 'e', 'i', 'o', 'u'];
    let chars: Vec<char> = base.chars().collect();
    let mut results = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        if VOWELS.contains(&ch.to_ascii_lowercase()) {
            for &v in VOWELS {
                if v != ch.to_ascii_lowercase() {
                    let mut s: Vec<char> = chars.clone();
                    s[i] = v;
                    let out: String = s.into_iter().collect();
                    if out.chars().count() >= 3 {
                        results.push(out);
                    }
                }
            }
        }
    }
    results
}

/// Swap each pair of adjacent characters.
fn adjacent_swaps(base: &str) -> Vec<String> {
    let chars: Vec<char> = base.chars().collect();
    let mut results = Vec::new();
    for i in 0..chars.len().saturating_sub(1) {
        let mut s = chars.clone();
        s.swap(i, i + 1);
        let out: String = s.into_iter().collect();
        if out.chars().count() >= 3 {
            results.push(out);
        }
    }
    results
}

/// Apply consonant-softening substitution pairs at each position:
/// b<->p, d<->t, g<->k, v<->f, s<->z
fn consonant_softening(base: &str) -> Vec<String> {
    const PAIRS: &[(char, char)] = &[('b', 'p'), ('d', 't'), ('g', 'k'), ('v', 'f'), ('s', 'z')];
    let chars: Vec<char> = base.chars().collect();
    let mut results = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        let lower = ch.to_ascii_lowercase();
        for &(a, b) in PAIRS {
            let replacement = if lower == a {
                Some(b)
            } else if lower == b {
                Some(a)
            } else {
                None
            };
            if let Some(r) = replacement {
                let mut s = chars.clone();
                s[i] = r;
                let out: String = s.into_iter().collect();
                if out.chars().count() >= 3 {
                    results.push(out);
                }
            }
        }
    }
    results
}

/// Split at uppercase-lowercase boundaries ("AirNote" -> "air note") and
/// produce the fully-joined lowercase ("airnote").
fn camel_case_splits(term: &str) -> Vec<String> {
    let chars: Vec<char> = term.chars().collect();
    let mut results = Vec::new();

    // Find split points at uppercase-lowercase boundaries
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 && ch.is_uppercase() && !chars[i - 1].is_uppercase() {
            if !current.is_empty() {
                parts.push(current.to_ascii_lowercase());
                current = String::new();
            }
        }
        current.push(ch);
    }
    if !current.is_empty() {
        parts.push(current.to_ascii_lowercase());
    }

    // Only produce results if we actually found splits
    if parts.len() > 1 {
        // "AirNote" -> "air note"
        let spaced = parts.join(" ");
        results.push(spaced);
        // "AirNote" -> "airnote"
        let joined = parts.join("");
        results.push(joined);
    }

    results
}

/// Reject distortions that are common words, too short, equal to the
/// correct form, or degenerate (single repeated character).
fn is_safe_distortion(distortion: &str, correct_form: &str) -> bool {
    if distortion.chars().count() < 3 {
        return false;
    }
    if distortion.eq_ignore_ascii_case(correct_form) {
        return false;
    }
    if crate::llm::alias_safety::is_common_alias_source(distortion) {
        return false;
    }
    // Reject single-char repeats like "aaa"
    let mut chars_iter = distortion.chars();
    if let Some(first) = chars_iter.next() {
        if chars_iter.all(|c| c == first) {
            return false;
        }
    }
    // Full dictionary + Hindi common word check
    if crate::tier2::is_in_dictionary(distortion) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(from: &str, to: &str) -> SttReplacement {
        SttReplacement {
            transcript_form: from.to_string(),
            correct_form: to.to_string(),
            phonetic_key: phonetics::phonetic_key(from),
            weight: 1.0,
            use_count: 1,
            last_used: 0,
            language: None,
            export_tier: ExportTier::LocalOnly,
            contradiction_count: 0,
            review_status: ReviewStatus::Approved,
            review_reason: None,
            last_reviewed_at: None,
        }
    }

    #[test]
    fn exact_replacement_preserves_punctuation() {
        let rules = vec![rule("written", "n8n")];
        let out = apply("I use written, daily", &rules);
        assert_eq!(out, "I use n8n, daily");
    }

    #[test]
    fn no_rules_passthrough() {
        let out = apply("hello world", &[]);
        assert_eq!(out, "hello world");
    }

    #[test]
    fn case_insensitive_match() {
        let rules = vec![rule("aiden", "n8n")];
        let out = apply("Aiden is great", &rules);
        assert_eq!(out, "n8n is great");
    }

    #[test]
    fn does_not_match_substring() {
        let rules = vec![rule("ai", "AI")];
        // "aiden" should NOT match the rule keyed on "ai" — we operate on
        // whole tokens.
        let out = apply("aiden saw the rain", &rules);
        assert_eq!(out, "aiden saw the rain");
    }

    #[test]
    fn phonetic_fuzzy_match() {
        // rule from "aiden", transcript has slight variant "aidan"
        let rules = vec![rule("aiden", "n8n")];
        let out = apply("aidan is great", &rules);
        // Both phonetic-key to "ATN"; similarity == 1.0 since keys equal.
        assert_eq!(out, "n8n is great");
    }

    #[test]
    fn phonetic_does_not_overreach() {
        // "hello" and "aiden" have very different keys — must not swap.
        let rules = vec![rule("aiden", "n8n")];
        let out = apply("hello world", &rules);
        assert_eq!(out, "hello world");
    }

    #[test]
    fn highest_weight_wins_when_multiple_rules_match() {
        let mut a = rule("written", "n8n");
        a.weight = 0.5;
        let mut b = rule("written", "Wisp");
        b.weight = 2.0;
        let rules = vec![b, a]; // sorted by weight DESC by load_all
        let out = apply("I use written daily", &rules);
        assert_eq!(out, "I use Wisp daily");
    }

    // ── pool-backed tests ──────────────────────────────────────────────────────
    use crate::store::DbPool;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE stt_replacements (
                 user_id          TEXT NOT NULL,
                 transcript_form  TEXT NOT NULL,
                 correct_form     TEXT NOT NULL,
                 phonetic_key     TEXT NOT NULL,
                 weight           REAL NOT NULL DEFAULT 1.0,
                 use_count        INTEGER NOT NULL DEFAULT 1,
                 last_used        INTEGER NOT NULL,
                 language         TEXT,
                 export_tier      TEXT NOT NULL DEFAULT 'local_only',
                 contradiction_count INTEGER NOT NULL DEFAULT 0,
                 last_contradicted_at INTEGER,
                 review_status    TEXT NOT NULL DEFAULT 'pending',
                 review_reason    TEXT,
                 last_reviewed_at INTEGER,
                 UNIQUE(user_id, transcript_form, correct_form)
             );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn upsert_then_load_then_apply_round_trip() {
        let pool = mem_pool();
        super::upsert(&pool, "u1", "Written", "n8n", 1.0);
        let rules = super::load_all(&pool, "u1");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].transcript_form, "written"); // lowercased
        let out = super::apply("I use written for automation", &rules);
        assert_eq!(out, "I use n8n for automation");
    }

    #[test]
    fn common_hindi_alias_sources_are_never_written() {
        let pool = mem_pool();
        assert!(!super::upsert(&pool, "u1", "kaisa", "Macobs", 1.0));
        assert!(!super::upsert(&pool, "u1", "कैसा", "Macobs", 1.0));
        assert!(!super::upsert(&pool, "u1", "laga", "Macobs", 1.0));
        assert!(super::load_all(&pool, "u1").is_empty());
    }

    // ── Foundational learning + apply behavior ─────────────────────────────

    #[test]
    fn upsert_aliases_stores_both_polish_and_transcript_spans() {
        // Foundational case: raw STT and polish saw two different rare
        // distortions for the same protected term.
        // BOTH spans should land as aliases so we match either shape later.
        let pool = mem_pool();
        let n = super::upsert_aliases(
            &pool, "u1", /* transcript_window */ "mecorbs",
            /* polish_window     */ "macorbs", /* correct_form      */ "Macobs", 1.0,
        );
        assert_eq!(n, 2, "expected both spans stored");
        let rules = super::load_all(&pool, "u1");
        assert!(rules.iter().any(|r| r.transcript_form == "macorbs"));
        assert!(rules.iter().any(|r| r.transcript_form == "mecorbs"));
    }

    #[test]
    fn upsert_aliases_skips_empty_transcript_window() {
        // Diff couldn't positionally align — transcript_window is empty.
        // We still store the polish span as an alias.
        let pool = mem_pool();
        let n = super::upsert_aliases(&pool, "u1", "", "mecorbs", "Macobs", 1.0);
        assert_eq!(n, 1);
    }

    #[test]
    fn upsert_aliases_dedupes_when_polish_equals_transcript() {
        // STT and polish were identical (no polish rewrite for this region).
        // Don't double-count — store once.
        let pool = mem_pool();
        let n = super::upsert_aliases(&pool, "u1", "mecorbs", "mecorbs", "Macobs", 1.0);
        assert_eq!(n, 1);
        let rules = super::load_all(&pool, "u1");
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn upsert_aliases_rejects_unrelated_multiword_alias() {
        let pool = mem_pool();
        let n = super::upsert_aliases(&pool, "u1", "urban aura", "urban aura", "Macobs", 1.0);
        assert_eq!(n, 0);
        assert!(super::load_all(&pool, "u1").is_empty());
    }

    #[test]
    fn contradiction_signal_revokes_export_before_blocking_local_memory() {
        let pool = mem_pool();
        assert!(super::upsert_with_language(
            &pool, "u1", "emi", "EMIAC", 3.0, "hinglish"
        ));
        assert!(super::update_export_metadata(
            &pool,
            "u1",
            "emi",
            "EMIAC",
            ExportTier::ExportReplaceReady,
            ReviewStatus::Approved,
            Some("trusted"),
            "hinglish",
        ));

        assert_eq!(super::note_negative_signal(&pool, "u1", "EMIAC", 1), 1);
        let once = super::get_for_language(&pool, "u1", "emi", "EMIAC", "hinglish").unwrap();
        assert_eq!(once.contradiction_count, 1);
        assert_eq!(once.export_tier, ExportTier::ExportKeytermSupport);
        assert_eq!(once.review_status, ReviewStatus::Pending);

        assert_eq!(super::note_negative_signal(&pool, "u1", "EMIAC", 1), 1);
        let twice = super::get_for_language(&pool, "u1", "emi", "EMIAC", "hinglish").unwrap();
        assert_eq!(twice.contradiction_count, 2);
        assert_eq!(twice.export_tier, ExportTier::LocalOnly);

        assert_eq!(super::note_negative_signal(&pool, "u1", "EMIAC", 2), 1);
        let blocked = super::get_for_language(&pool, "u1", "emi", "EMIAC", "hinglish").unwrap();
        assert_eq!(blocked.contradiction_count, 4);
        assert_eq!(blocked.export_tier, ExportTier::Blocked);
    }

    #[test]
    fn delete_by_correct_form_clears_all_aliases_pointing_at_it() {
        // Regression: when a user deletes a vocab term, EVERY alias that
        // would otherwise rewrite raw STT into that canonical must die too.
        // Without this, the pre-polish layer keeps mapping learned STT
        // distortions into the canonical after the term was deleted.
        let pool = mem_pool();
        super::upsert_aliases(&pool, "u1", "mecorbs", "macorbs", "MACOBS", 1.0);
        super::upsert(&pool, "u1", "micobs", "MACOBS", 1.0);
        // Unrelated alias for a different canonical — must survive.
        super::upsert(&pool, "u1", "Written", "n8n", 1.0);
        assert_eq!(super::load_all(&pool, "u1").len(), 4);

        // Case-insensitive on the canonical so `delete("macobs")` still wipes
        // an alias stored as "MACOBS".
        let n = super::delete_by_correct_form(&pool, "u1", "macobs");
        assert_eq!(n, 3, "expected 3 MACOBS aliases removed");

        let remaining = super::load_all(&pool, "u1");
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].correct_form, "n8n",
            "unrelated alias for n8n must survive"
        );
    }

    #[test]
    fn delete_by_correct_form_is_safe_on_no_match() {
        let pool = mem_pool();
        super::upsert(&pool, "u1", "Written", "n8n", 1.0);
        let n = super::delete_by_correct_form(&pool, "u1", "NeverExisted");
        assert_eq!(n, 0);
        assert_eq!(
            super::load_all(&pool, "u1").len(),
            1,
            "unrelated rule untouched"
        );
    }

    #[test]
    fn upsert_skips_no_op_rule() {
        // Don't waste a row on alias == canonical.
        let pool = mem_pool();
        assert!(!super::upsert(&pool, "u1", "macobs", "MACOBS", 1.0));
        assert_eq!(super::load_all(&pool, "u1").len(), 0);
    }

    #[test]
    fn apply_matches_multi_word_phrase_at_any_position() {
        // The MACOBS regression: a "Main corps" rule must fire on
        // "मैं Main corps का IPO" when those two words appear together.
        let rules = vec![rule("Main corps", "MACOBS")];
        let out = super::apply("मैं Main corps का IPO", &rules);
        assert_eq!(out, "मैं MACOBS का IPO");
    }

    #[test]
    fn apply_longest_match_wins_at_overlapping_starts() {
        // Two rules could both match starting from "Main corps" — the longer
        // one ("Main corps detailed") should win when its full span is present.
        let rules = vec![
            rule("Main corps detailed", "MACOBS_DETAILED"),
            rule("Main corps", "MACOBS"),
        ];
        let out = super::apply("Please Main corps detailed today", &rules);
        assert_eq!(out, "Please MACOBS_DETAILED today");
    }

    #[test]
    fn apply_falls_back_to_shorter_when_longer_misses() {
        // "Main corps detailed" rule exists but transcript has only
        // "Main corps". Shorter "Main corps" rule must still fire.
        let rules = vec![
            rule("Main corps detailed", "MACOBS_DETAILED"),
            rule("Main corps", "MACOBS"),
        ];
        let out = super::apply("Today Main corps please", &rules);
        assert_eq!(out, "Today MACOBS please");
    }

    #[test]
    fn apply_handles_devanagari_alias() {
        // Raw STT alias contains Devanagari — must match a Devanagari span.
        let rules = vec![rule("मैं corps", "MACOBS")];
        let out = super::apply("मैं corps का password", &rules);
        assert_eq!(out, "MACOBS का password");
    }

    #[test]
    fn apply_preserves_punctuation_around_match() {
        let rules = vec![rule("Main corps", "MACOBS")];
        let out = super::apply("Hi (Main corps), today", &rules);
        // Leading "(" and trailing "," + space preserved
        assert_eq!(out, "Hi (MACOBS), today");
    }

    #[test]
    fn demote_until_blocked() {
        let pool = mem_pool();
        super::upsert(&pool, "u1", "writen", "n8n", 1.0);
        assert_eq!(super::load_all(&pool, "u1").len(), 1);
        super::demote(&pool, "u1", "writen", "n8n", 1.5);
        let remaining = super::load_all(&pool, "u1");
        assert_eq!(remaining.len(), 1, "blocked row must survive");
        assert_eq!(remaining[0].review_status, ReviewStatus::Blocked);
        assert!(remaining[0].weight <= 0.0);
    }

    // ── apply_exact_safe tests ──────────────────────────────────────────────

    #[test]
    fn exact_safe_replaces_gibberish_alias() {
        let rules = vec![rule("meac", "EMIAC")];
        let result = super::apply_exact_safe("meac mein bahut kaam hai", &rules);
        assert_eq!(result.text, "EMIAC mein bahut kaam hai");
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].kind, MatchKind::Exact);
    }

    #[test]
    fn exact_safe_skips_common_hindi_word() {
        let rules = vec![rule("main", "EMIAC")];
        let result = super::apply_exact_safe("main bahut kaam karta hoon", &rules);
        assert_eq!(
            result.text, "main bahut kaam karta hoon",
            "common Hindi word 'main' must not be replaced"
        );
        assert!(result.matches.is_empty());
    }

    #[test]
    fn exact_safe_skips_phonetic_fallback() {
        let rules = vec![rule("meac", "EMIAC")];
        // "meah" is phonetically similar to "meac" but NOT an exact match.
        // apply_exact_safe must skip it (no phonetic fallback).
        let result = super::apply_exact_safe("meah mein kaam hai", &rules);
        assert_eq!(
            result.text, "meah mein kaam hai",
            "exact_safe must not use phonetic matching"
        );
        assert!(result.matches.is_empty());
    }

    #[test]
    fn exact_safe_skips_single_char_alias() {
        let rules = vec![rule("m", "EMIAC")];
        let result = super::apply_exact_safe("m se matlab", &rules);
        assert_eq!(result.text, "m se matlab");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn exact_safe_handles_multiple_aliases() {
        let rules = vec![
            rule("meac", "EMIAC"),
            rule("mecobs", "Macobs"),
            rule("aneten", "n8n"),
        ];
        let result = super::apply_exact_safe("meac aur mecobs mein aneten use karte hain", &rules);
        assert_eq!(result.text, "EMIAC aur Macobs mein n8n use karte hain");
        assert_eq!(result.matches.len(), 3);
    }

    #[test]
    fn exact_safe_skips_blocked_review_status() {
        let mut r = rule("meac", "EMIAC");
        r.review_status = ReviewStatus::Blocked;
        let result = super::apply_exact_safe("meac mein kaam", &[r]);
        assert_eq!(result.text, "meac mein kaam");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn exact_safe_skips_implausible_multiword_alias() {
        let result =
            super::apply_exact_safe("Urban Aura ka data bhejo", &[rule("urban aura", "Macobs")]);
        assert_eq!(result.text, "Urban Aura ka data bhejo");
        assert!(result.matches.is_empty());
    }

    // ── alias cap tests ─────────────────────────────────────────────────────

    #[test]
    fn alias_cap_evicts_lowest_weight_when_over_limit() {
        let pool = mem_pool();
        for i in 0..20 {
            let form = format!("variant{i}");
            super::upsert(&pool, "u1", &form, "EMIAC", 1.0 + i as f64 * 0.1);
        }
        let rules = super::load_all(&pool, "u1");
        assert!(
            rules.len() <= super::MAX_ALIASES_PER_TERM,
            "alias count {} should be capped at {}",
            rules.len(),
            super::MAX_ALIASES_PER_TERM,
        );
    }
}
