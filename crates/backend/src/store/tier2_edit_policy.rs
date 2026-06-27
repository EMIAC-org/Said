use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::{DbPool, now_ms};

pub const ACTIVATION_POSITIVES: i64 = 2;
pub const BLOCK_NEGATIVES: i64 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct Tier2EditPolicyRule {
    pub variant_form: String,
    pub variant_norm: String,
    pub correct_form: String,
    pub correct_form_norm: String,
    pub edit_type: String,
    pub positive_count: i64,
    pub negative_count: i64,
    pub status: String,
    pub left_context: Vec<String>,
    pub right_context: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Tier2EditPolicyStatus {
    pub candidate_rule_count: i64,
    pub active_rule_count: i64,
    pub blocked_rule_count: i64,
    pub explicit_positive_count: i64,
    pub negative_count: i64,
    pub fuzzy_shadow_count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedbackMarkResult {
    pub penalized: usize,
    pub marked_kept: usize,
    /// Pairs that were penalized, surfaced to the desktop for status-bar notification.
    /// Each tuple: (token_text the user actually wrote, chosen_form the system wrongly emitted).
    pub penalized_pairs: Vec<(String, String)>,
}

pub fn normalize_token(text: &str) -> String {
    crate::llm::alias_safety::compact_norm(text)
}

pub fn record_explicit_edit(
    pool: &DbPool,
    user_id: &str,
    variant_form: &str,
    correct_form: &str,
    edit_type: &str,
    left_context: &[String],
    right_context: &[String],
    source_recording_id: Option<&str>,
) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "tier2_edit_policy_rules",
            "record_explicit_edit",
            "tier2_edit_policy::record_explicit_edit",
        );
        return false;
    }
    let edit_type = normalize_edit_type(edit_type);
    let variant_form = variant_form.trim();
    let correct_form = correct_form.trim();
    let variant_norm = normalize_token(variant_form);
    let correct_form_norm = normalize_token(correct_form);
    if correct_form_norm.is_empty()
        || (edit_type == "replace" && variant_norm.is_empty())
        || (edit_type == "replace" && variant_norm == correct_form_norm)
    {
        return false;
    }
    if edit_type == "replace"
        && (crate::llm::alias_safety::is_common_alias_source(variant_form)
            || crate::llm::alias_safety::is_common_alias_source(&variant_norm)
            || crate::llm::promotion_gate::is_common_word(variant_form)
            || crate::llm::promotion_gate::is_common_word(&variant_norm))
    {
        return false;
    }

    let Ok(conn) = pool.get() else {
        return false;
    };
    let now = now_ms();
    let existing = conn
        .query_row(
            "SELECT positive_count, negative_count
               FROM tier2_edit_policy_rules
              WHERE user_id = ?1
                AND variant_norm = ?2
                AND correct_form_norm = ?3
                AND edit_type = ?4",
            params![user_id, variant_norm, correct_form_norm, edit_type],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let positive_count = existing.map(|(pos, _)| pos).unwrap_or(0) + 1;
    let negative_count = existing.map(|(_, neg)| neg).unwrap_or(0);
    let status = status_for_counts(positive_count, negative_count);
    let left_json = context_json(left_context);
    let right_json = context_json(right_context);

    conn.execute(
        "INSERT INTO tier2_edit_policy_rules
             (user_id, variant_form, variant_norm, correct_form, correct_form_norm,
              edit_type, positive_count, negative_count, status, first_seen,
              last_seen, left_context_json, right_context_json, source_recording_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13)
         ON CONFLICT(user_id, variant_norm, correct_form_norm, edit_type) DO UPDATE SET
              variant_form = excluded.variant_form,
              correct_form = excluded.correct_form,
              positive_count = excluded.positive_count,
              negative_count = excluded.negative_count,
              status = excluded.status,
              last_seen = excluded.last_seen,
              left_context_json = excluded.left_context_json,
              right_context_json = excluded.right_context_json,
              source_recording_id = excluded.source_recording_id",
        params![
            user_id,
            variant_form,
            variant_norm,
            correct_form,
            correct_form_norm,
            edit_type,
            positive_count,
            negative_count,
            status,
            now,
            left_json,
            right_json,
            source_recording_id,
        ],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

pub fn penalize_rule(pool: &DbPool, user_id: &str, variant_form: &str, correct_form: &str) -> bool {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "tier2_edit_policy_rules",
            "penalize_rule",
            "tier2_edit_policy::penalize_rule",
        );
        return false;
    }
    let variant_norm = normalize_token(variant_form);
    let correct_form_norm = normalize_token(correct_form);
    if variant_norm.is_empty() || correct_form_norm.is_empty() {
        return false;
    }
    let Ok(conn) = pool.get() else {
        return false;
    };
    let existing = conn
        .query_row(
            "SELECT positive_count, negative_count
               FROM tier2_edit_policy_rules
              WHERE user_id = ?1 AND variant_norm = ?2 AND correct_form_norm = ?3
                AND edit_type = 'replace'",
            params![user_id, variant_norm, correct_form_norm],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((positive_count, negative_count)) = existing else {
        // No existing rule — this revert came from a cluster-fuzzy or ONNX
        // correction with no stored rule. Insert a BLOCKED rule so the same
        // wrong mapping cannot fire again.
        let now = now_ms();
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO tier2_edit_policy_rules
                    (user_id, variant_form, variant_norm, correct_form, correct_form_norm,
                     edit_type, positive_count, negative_count, status, first_seen, last_seen)
                  VALUES (?1, ?2, ?3, ?4, ?5, 'replace', 0, 2, 'blocked', ?6, ?6)",
                params![
                    user_id,
                    variant_form,
                    variant_norm,
                    correct_form,
                    correct_form_norm,
                    now
                ],
            )
            .unwrap_or(0);
        return inserted > 0;
    };
    let next_negative = negative_count + 1;
    let status = status_for_counts(positive_count, next_negative);
    conn.execute(
        "UPDATE tier2_edit_policy_rules
            SET negative_count = ?4,
                status = ?5,
                last_seen = ?6
          WHERE user_id = ?1
            AND variant_norm = ?2
            AND correct_form_norm = ?3
            AND edit_type = 'replace'",
        params![
            user_id,
            variant_norm,
            correct_form_norm,
            next_negative,
            status,
            now_ms(),
        ],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

pub fn load_active_replace_rules(pool: &DbPool, user_id: &str) -> Vec<Tier2EditPolicyRule> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT variant_form, variant_norm, correct_form, correct_form_norm,
                edit_type, positive_count, negative_count, status,
                left_context_json, right_context_json
           FROM tier2_edit_policy_rules
          WHERE user_id = ?1
            AND status = 'active'
            AND edit_type = 'replace'
          ORDER BY positive_count DESC, last_seen DESC",
    ) else {
        return vec![];
    };
    stmt.query_map(params![user_id], |row| {
        Ok(Tier2EditPolicyRule {
            variant_form: row.get(0)?,
            variant_norm: row.get(1)?,
            correct_form: row.get(2)?,
            correct_form_norm: row.get(3)?,
            edit_type: row.get(4)?,
            positive_count: row.get(5)?,
            negative_count: row.get(6)?,
            status: row.get(7)?,
            left_context: parse_context(row.get::<_, Option<String>>(8)?.as_deref()),
            right_context: parse_context(row.get::<_, Option<String>>(9)?.as_deref()),
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|row| row.ok()).collect())
    .unwrap_or_default()
}

pub fn activate_all_for_term(pool: &DbPool, user_id: &str, correct_form: &str) -> usize {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "tier2_edit_policy_rules",
            "activate_all_for_term",
            "tier2_edit_policy::activate_all_for_term",
        );
        return 0;
    }
    let norm = normalize_token(correct_form);
    if norm.is_empty() {
        return 0;
    }
    let Ok(conn) = pool.get() else { return 0 };
    conn.execute(
        "UPDATE tier2_edit_policy_rules
            SET status = 'active',
                positive_count = MAX(positive_count, 2)
          WHERE user_id = ?1
            AND correct_form_norm = ?2
            AND status = 'candidate'
            AND edit_type = 'replace'
            AND positive_count >= ?3",
        rusqlite::params![user_id, norm, ACTIVATION_POSITIVES],
    )
    .unwrap_or(0)
}

pub fn mark_removed_feedback(
    pool: &DbPool,
    user_id: &str,
    recording_id: &str,
    visible_output: &str,
    user_kept: &str,
) -> FeedbackMarkResult {
    if !crate::legacy_learning::legacy_learning_writes_allowed() {
        crate::legacy_learning::skip_legacy_write(
            "tier2_edit_policy_rules",
            "mark_removed_feedback",
            "tier2_edit_policy::mark_removed_feedback",
        );
        return FeedbackMarkResult::default();
    }
    let events: Vec<(String, String, String, String)> = {
        let Ok(conn) = pool.get() else {
            return FeedbackMarkResult::default();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, token_text, chosen_form, scorer_kind
               FROM tier2_decision_events
              WHERE user_id = ?1
                AND recording_id = ?2
                AND applied = 1
                AND reward_outcome IS NULL
                AND scorer_kind IN (
                    'tier2_edit_policy',
                    'tier2_cluster_fuzzy',
                    'tier2_deterministic',
                    'tier2_onnx',
                    'tier2_context',
                    'tier2_policy',
                    'stt_alias'
                )
                AND chosen_form IS NOT NULL",
        ) else {
            return FeedbackMarkResult::default();
        };
        let Ok(rows) = stmt.query_map(params![user_id, recording_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) else {
            return FeedbackMarkResult::default();
        };
        rows.flatten().collect()
    };

    let mut result = FeedbackMarkResult::default();
    for (event_id, token_text, chosen_form, scorer_kind) in events {
        let was_visible = contains_term(visible_output, &chosen_form);
        if !was_visible {
            if let Ok(conn) = pool.get() {
                let _ = conn.execute(
                    "UPDATE tier2_decision_events
                        SET reward_outcome = ?2,
                            feedback_at = ?3
                      WHERE id = ?1",
                    params![event_id, "negative_unverified", now_ms()],
                );
            }
            continue;
        }

        let kept = contains_term(user_kept, &chosen_form);
        let outcome = if kept {
            result.marked_kept += 1;
            "kept_unrewarded"
        } else {
            let penalized = if scorer_kind == "stt_alias" {
                // Demote the STT alias that mapped token_text -> chosen_form.
                // demote() reduces the alias weight; when weight <= 0 the row
                // is deleted automatically, preventing future false positives.
                super::stt_replacements::demote(pool, user_id, &token_text, &chosen_form, 1.5)
            } else {
                let policy_penalized = super::tier2_policy::penalize_mapping(
                    pool,
                    user_id,
                    &token_text,
                    &chosen_form,
                    1.0,
                    "feedback_removed",
                );
                let rule_penalized = penalize_rule(pool, user_id, &token_text, &chosen_form);
                policy_penalized || rule_penalized
            };
            if penalized {
                result.penalized += 1;
                result
                    .penalized_pairs
                    .push((token_text.clone(), chosen_form.clone()));
                "negative"
            } else {
                "negative_unapplied"
            }
        };
        if let Ok(conn) = pool.get() {
            let _ = conn.execute(
                "UPDATE tier2_decision_events
                    SET reward_outcome = ?2,
                        feedback_at = ?3
                  WHERE id = ?1",
                params![event_id, outcome, now_ms()],
            );
        }
    }
    result
}

pub fn status(pool: &DbPool, user_id: &str) -> Tier2EditPolicyStatus {
    let Ok(conn) = pool.get() else {
        return Tier2EditPolicyStatus::default();
    };
    let mut status = conn
        .query_row(
            "SELECT
                 COALESCE(SUM(CASE WHEN status = 'candidate' THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN status = 'blocked' THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(positive_count), 0),
                 COALESCE(SUM(negative_count), 0)
               FROM tier2_edit_policy_rules
              WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok(Tier2EditPolicyStatus {
                    candidate_rule_count: row.get(0)?,
                    active_rule_count: row.get(1)?,
                    blocked_rule_count: row.get(2)?,
                    explicit_positive_count: row.get(3)?,
                    negative_count: row.get(4)?,
                    fuzzy_shadow_count: 0,
                })
            },
        )
        .unwrap_or_default();
    status.fuzzy_shadow_count = conn
        .query_row(
            "SELECT COUNT(*)
               FROM tier2_decision_events
              WHERE user_id = ?1
                AND applied = 0
                AND scorer_kind IN ('tier2_deterministic', 'tier2_onnx')",
            params![user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    status
}

fn normalize_edit_type(edit_type: &str) -> &'static str {
    match edit_type.trim() {
        "insert" => "insert",
        _ => "replace",
    }
}

fn status_for_counts(positive_count: i64, negative_count: i64) -> &'static str {
    if negative_count >= BLOCK_NEGATIVES {
        "blocked"
    } else if positive_count >= ACTIVATION_POSITIVES {
        "active"
    } else {
        "candidate"
    }
}

fn context_json(tokens: &[String]) -> Option<String> {
    let cleaned = tokens
        .iter()
        .map(|token| normalize_token(token))
        .filter(|token| !token.is_empty())
        .take(3)
        .collect::<Vec<_>>();
    if cleaned.is_empty() {
        None
    } else {
        serde_json::to_string(&cleaned).ok()
    }
}

fn parse_context(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .unwrap_or_default()
}

fn contains_term(text: &str, term: &str) -> bool {
    let needle = normalize_token(term);
    if needle.is_empty() {
        return false;
    }
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .any(|part| normalize_token(part) == needle)
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
             CREATE TABLE recordings (id TEXT PRIMARY KEY);
             INSERT INTO recordings(id) VALUES ('r1');
             CREATE TABLE tier2_edit_policy_rules (
                user_id TEXT NOT NULL,
                variant_form TEXT NOT NULL,
                variant_norm TEXT NOT NULL,
                correct_form TEXT NOT NULL,
                correct_form_norm TEXT NOT NULL,
                edit_type TEXT NOT NULL DEFAULT 'replace',
                positive_count INTEGER NOT NULL DEFAULT 0,
                negative_count INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'candidate',
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                left_context_json TEXT,
                right_context_json TEXT,
                source_recording_id TEXT,
                PRIMARY KEY (user_id, variant_norm, correct_form_norm, edit_type)
             );
             CREATE TABLE tier2_decision_events (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                recording_id TEXT,
                timestamp_ms INTEGER NOT NULL,
                token_text TEXT NOT NULL,
                token_norm TEXT NOT NULL,
                chosen_form TEXT,
                chosen_form_norm TEXT,
                second_form TEXT,
                scorer_kind TEXT NOT NULL,
                score REAL NOT NULL DEFAULT 0.0,
                second_score REAL,
                margin REAL NOT NULL DEFAULT 0.0,
                deterministic_score REAL,
                onnx_score REAL,
                policy_score REAL,
                policy_boost REAL NOT NULL DEFAULT 0.0,
                applied INTEGER NOT NULL DEFAULT 0,
                reward_outcome TEXT,
                feedback_at INTEGER
             );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn second_explicit_confirmation_activates_rule() {
        let pool = mem_pool();
        assert!(record_explicit_edit(
            &pool,
            "u1",
            "macops",
            "Macobs",
            "replace",
            &[],
            &["data".to_string()],
            Some("r1"),
        ));
        assert_eq!(status(&pool, "u1").candidate_rule_count, 1);
        assert!(record_explicit_edit(
            &pool,
            "u1",
            "macops",
            "Macobs",
            "replace",
            &[],
            &["report".to_string()],
            Some("r1"),
        ));
        let status = status(&pool, "u1");
        assert_eq!(status.candidate_rule_count, 0);
        assert_eq!(status.active_rule_count, 1);
        assert_eq!(status.explicit_positive_count, 2);
        assert_eq!(load_active_replace_rules(&pool, "u1").len(), 1);
    }

    #[test]
    fn common_word_source_is_rejected() {
        let pool = mem_pool();
        assert!(!record_explicit_edit(
            &pool,
            "u1",
            "main",
            "Macobs",
            "replace",
            &[],
            &[],
            Some("r1"),
        ));
        assert_eq!(status(&pool, "u1").candidate_rule_count, 0);
    }

    #[test]
    fn repeated_penalty_blocks_rule() {
        let pool = mem_pool();
        record_explicit_edit(
            &pool,
            "u1",
            "macops",
            "Macobs",
            "replace",
            &[],
            &[],
            Some("r1"),
        );
        record_explicit_edit(
            &pool,
            "u1",
            "macops",
            "Macobs",
            "replace",
            &[],
            &[],
            Some("r1"),
        );
        assert!(penalize_rule(&pool, "u1", "macops", "Macobs"));
        assert!(penalize_rule(&pool, "u1", "macops", "Macobs"));
        let status = status(&pool, "u1");
        assert_eq!(status.active_rule_count, 0);
        assert_eq!(status.blocked_rule_count, 1);
    }

    /// mem_pool with the stt_replacements table so mark_removed_feedback
    /// can demote aliases via the `stt_alias` scorer_kind path.
    fn mem_pool_with_stt() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE recordings (id TEXT PRIMARY KEY);
             INSERT INTO recordings(id) VALUES ('r1');
             CREATE TABLE tier2_edit_policy_rules (
                user_id TEXT NOT NULL,
                variant_form TEXT NOT NULL,
                variant_norm TEXT NOT NULL,
                correct_form TEXT NOT NULL,
                correct_form_norm TEXT NOT NULL,
                edit_type TEXT NOT NULL DEFAULT 'replace',
                positive_count INTEGER NOT NULL DEFAULT 0,
                negative_count INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'candidate',
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                left_context_json TEXT,
                right_context_json TEXT,
                source_recording_id TEXT,
                PRIMARY KEY (user_id, variant_norm, correct_form_norm, edit_type)
             );
             CREATE TABLE tier2_decision_events (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                recording_id TEXT,
                timestamp_ms INTEGER NOT NULL,
                token_text TEXT NOT NULL,
                token_norm TEXT NOT NULL,
                chosen_form TEXT,
                chosen_form_norm TEXT,
                second_form TEXT,
                scorer_kind TEXT NOT NULL,
                score REAL NOT NULL DEFAULT 0.0,
                second_score REAL,
                margin REAL NOT NULL DEFAULT 0.0,
                deterministic_score REAL,
                onnx_score REAL,
                policy_score REAL,
                policy_boost REAL NOT NULL DEFAULT 0.0,
                applied INTEGER NOT NULL DEFAULT 0,
                reward_outcome TEXT,
                feedback_at INTEGER
             );
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
    fn mark_removed_feedback_penalizes_stt_alias() {
        let pool = mem_pool_with_stt();
        // Insert an STT alias: mac -> EMIAC
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stt_replacements
                 (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used)
             VALUES ('u1', 'mac', 'EMIAC', 'MK', 1.5, 2, 0)",
            [],
        )
        .unwrap();
        // Insert a decision event as if the tier2 engine applied this alias
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt1', 'u1', 'r1', 0, 'mac', 'mac', 'EMIAC', 'emiac',
                     'stt_alias', 1.0, 1.0, 1.0, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        // User corrected back to "Mac" -- EMIAC is NOT in kept text
        let feedback = mark_removed_feedback(
            &pool,
            "u1",
            "r1",
            "EMIAC ka kaam ho gaya",
            "Mac ka kaam ho gaya",
        );
        assert_eq!(feedback.penalized, 1, "stt_alias event should be penalized");
        assert_eq!(feedback.marked_kept, 0);

        // Verify the alias was demoted — demote() now blocks instead of
        // deleting, so the row survives with review_status = 'blocked'.
        let conn = pool.get().unwrap();
        let (weight, status): (f64, String) = conn
            .query_row(
                "SELECT weight, review_status FROM stt_replacements
                  WHERE user_id = 'u1' AND transcript_form = 'mac' AND correct_form = 'EMIAC'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(
            weight <= 0.0,
            "weight should be <= 0 after demotion, got {weight}"
        );
        assert_eq!(
            status, "blocked",
            "alias should be permanently blocked, not deleted"
        );
    }

    #[test]
    fn mark_removed_feedback_keeps_stt_alias_when_term_present() {
        let pool = mem_pool_with_stt();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stt_replacements
                 (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used)
             VALUES ('u1', 'meac', 'EMIAC', 'MK', 2.0, 3, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt2', 'u1', 'r1', 0, 'meac', 'meac', 'EMIAC', 'emiac',
                     'stt_alias', 1.0, 1.0, 1.0, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        // User kept EMIAC in their text — should NOT penalize
        let feedback = mark_removed_feedback(
            &pool,
            "u1",
            "r1",
            "EMIAC ka kaam ho gaya",
            "EMIAC ka kaam ho gaya",
        );
        assert_eq!(feedback.penalized, 0);
        assert_eq!(feedback.marked_kept, 1);

        // Verify the alias weight is unchanged
        let conn = pool.get().unwrap();
        let weight: f64 = conn
            .query_row(
                "SELECT weight FROM stt_replacements
                  WHERE user_id = 'u1' AND transcript_form = 'meac' AND correct_form = 'EMIAC'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            (weight - 2.0).abs() < 0.01,
            "alias weight should be unchanged"
        );
    }

    #[test]
    fn cluster_fuzzy_revert_inserts_blocked_rule() {
        let pool = mem_pool_with_stt();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt_cf', 'u1', 'r1', 0, 'nahi', 'nahi', 'n8n', 'n8n',
                     'tier2_cluster_fuzzy', 0.80, 0.80, 0.80, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let feedback =
            mark_removed_feedback(&pool, "u1", "r1", "n8n bol raha hun", "nahi bol raha hun");

        assert_eq!(
            feedback.penalized, 1,
            "cluster-fuzzy revert should penalize"
        );
        assert_eq!(
            feedback.penalized_pairs.len(),
            1,
            "should surface the penalized pair"
        );
        assert_eq!(feedback.penalized_pairs[0].0, "nahi");
        assert_eq!(feedback.penalized_pairs[0].1, "n8n");

        let conn = pool.get().unwrap();
        let (neg_count, rule_status): (i64, String) = conn
            .query_row(
                "SELECT negative_count, status FROM tier2_edit_policy_rules
                  WHERE user_id = 'u1' AND variant_norm = 'nahi' AND correct_form_norm = 'n8n'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rule_status, "blocked", "new rule should be blocked");
        assert!(neg_count >= 2, "negative count should be >= 2");
    }

    #[test]
    fn invisible_decision_is_not_penalized() {
        let pool = mem_pool_with_stt();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt_hidden', 'u1', 'r1', 0, 'करके', 'karake', 'Groq', 'groq',
                     'tier2_cluster_fuzzy', 0.82, 0.82, 0.82, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let feedback = mark_removed_feedback(
            &pool,
            "u1",
            "r1",
            "n8n aur Kafka ka use karke automation seekhni hai",
            "n8n aur Kafka ka use karke automation seekhni hai",
        );

        assert_eq!(feedback.penalized, 0);
        assert_eq!(feedback.marked_kept, 0);
        assert!(feedback.penalized_pairs.is_empty());

        let conn = pool.get().unwrap();
        let outcome: String = conn
            .query_row(
                "SELECT reward_outcome FROM tier2_decision_events WHERE id = 'evt_hidden'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outcome, "negative_unverified");
    }

    #[test]
    fn onnx_revert_inserts_blocked_rule() {
        let pool = mem_pool_with_stt();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt_onnx', 'u1', 'r1', 0, 'agent', 'agent', 'AirNote', 'airnote',
                     'tier2_cluster_fuzzy', 0.82, 0.82, 0.82, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let feedback = mark_removed_feedback(
            &pool,
            "u1",
            "r1",
            "AirNote is working on the deployment",
            "agent is working on the deployment",
        );

        assert_eq!(feedback.penalized, 1);
        assert_eq!(feedback.penalized_pairs[0].0, "agent");
        assert_eq!(feedback.penalized_pairs[0].1, "AirNote");

        let conn = pool.get().unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM tier2_edit_policy_rules
                  WHERE user_id = 'u1' AND correct_form_norm = 'airnote'
                    AND variant_norm = 'agent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "blocked");
    }

    #[test]
    fn multiple_reverts_in_one_recording() {
        let pool = mem_pool_with_stt();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt_a', 'u1', 'r1', 0, 'nahi', 'nahi', 'n8n', 'n8n',
                     'tier2_cluster_fuzzy', 0.80, 0.80, 0.80, 0.0, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt_b', 'u1', 'r1', 0, 'aajkal', 'aajkal', 'Kafka', 'kafka',
                     'tier2_cluster_fuzzy', 0.82, 0.82, 0.82, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let feedback = mark_removed_feedback(
            &pool,
            "u1",
            "r1",
            "n8n Kafka yeh sab chal raha hai",
            "nahi aajkal yeh sab chal raha hai",
        );

        assert_eq!(
            feedback.penalized, 2,
            "both wrong corrections should be penalized"
        );
        assert_eq!(feedback.penalized_pairs.len(), 2);

        let conn = pool.get().unwrap();
        let blocked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tier2_edit_policy_rules
                  WHERE user_id = 'u1' AND status = 'blocked'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(blocked, 2, "both mappings should be blocked");
    }

    #[test]
    fn kept_correction_is_not_penalized() {
        let pool = mem_pool_with_stt();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tier2_decision_events
                 (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                  chosen_form, chosen_form_norm, scorer_kind, score, margin,
                  deterministic_score, policy_boost, applied)
             VALUES ('evt_ok', 'u1', 'r1', 0, 'mecobs', 'mecobs', 'Macobs', 'macobs',
                     'tier2_cluster_fuzzy', 0.90, 0.90, 0.90, 0.0, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let feedback = mark_removed_feedback(
            &pool,
            "u1",
            "r1",
            "Macobs ka kaam ho gaya hai",
            "Macobs ka kaam ho gaya hai",
        );

        assert_eq!(
            feedback.penalized, 0,
            "correct correction should not be penalized"
        );
        assert_eq!(feedback.marked_kept, 1, "should be marked as kept");
        assert!(feedback.penalized_pairs.is_empty());
    }

    #[test]
    fn penalize_existing_active_rule_blocks_it() {
        let pool = mem_pool();
        record_explicit_edit(
            &pool,
            "u1",
            "mecorbs",
            "Macobs",
            "replace",
            &[],
            &["ka".to_string()],
            Some("r1"),
        );
        record_explicit_edit(
            &pool,
            "u1",
            "mecorbs",
            "Macobs",
            "replace",
            &[],
            &["ki".to_string()],
            Some("r1"),
        );
        let s = status(&pool, "u1");
        assert_eq!(s.active_rule_count, 1, "rule should be active");

        assert!(penalize_rule(&pool, "u1", "mecorbs", "Macobs"));
        assert!(penalize_rule(&pool, "u1", "mecorbs", "Macobs"));
        let s = status(&pool, "u1");
        assert_eq!(s.active_rule_count, 0);
        assert_eq!(
            s.blocked_rule_count, 1,
            "rule should be blocked after 2 penalties"
        );
    }
}
