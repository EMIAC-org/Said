use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use uuid::Uuid;

use super::{DbPool, now_ms};
use crate::store::stt_replacements::{ApplyResult, MatchKind, ScoringTrace};

pub type PolicyKey = (String, String);

#[derive(Debug, Clone, PartialEq)]
pub struct Tier2PolicyWeight {
    pub token_norm: String,
    pub correct_form: String,
    pub correct_form_norm: String,
    pub positive_count: i64,
    pub negative_count: i64,
    pub learned_weight: f64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub last_reward_source: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedbackMarkResult {
    pub rewarded: usize,
    pub penalized: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Tier2PolicyStatus {
    pub policy_pair_count: i64,
    pub positive_reward_count: i64,
    pub negative_reward_count: i64,
    pub last_policy_update_at: Option<i64>,
    pub rewards_since_onnx: i64,
}

pub fn normalize_token(text: &str) -> String {
    crate::llm::alias_safety::compact_norm(text)
}

pub fn reward_mapping(
    pool: &DbPool,
    user_id: &str,
    token: &str,
    correct_form: &str,
    amount: f64,
    source: &str,
) -> bool {
    update_mapping(
        pool,
        user_id,
        token,
        correct_form,
        amount.abs(),
        source,
        true,
    )
}

pub fn penalize_mapping(
    pool: &DbPool,
    user_id: &str,
    token: &str,
    correct_form: &str,
    amount: f64,
    source: &str,
) -> bool {
    update_mapping(
        pool,
        user_id,
        token,
        correct_form,
        amount.abs(),
        source,
        false,
    )
}

fn update_mapping(
    pool: &DbPool,
    user_id: &str,
    token: &str,
    correct_form: &str,
    amount: f64,
    source: &str,
    positive: bool,
) -> bool {
    let token_norm = normalize_token(token);
    let correct_form = correct_form.trim();
    let correct_form_norm = normalize_token(correct_form);
    if token_norm.is_empty() || correct_form_norm.is_empty() || token_norm == correct_form_norm {
        return false;
    }
    if crate::llm::alias_safety::is_common_alias_source(token)
        || crate::llm::alias_safety::is_common_alias_source(&token_norm)
        || crate::llm::promotion_gate::is_common_word(token)
        || crate::llm::promotion_gate::is_common_word(&token_norm)
    {
        return false;
    }

    let Ok(conn) = pool.get() else {
        return false;
    };
    let now = now_ms();
    let positive_delta = if positive { 1 } else { 0 };
    let negative_delta = if positive { 0 } else { 1 };
    let weight_delta = if positive { amount } else { -amount * 1.5 };

    conn.execute(
        "INSERT INTO tier2_policy_weights
             (user_id, token_norm, correct_form, correct_form_norm,
              positive_count, negative_count, learned_weight, first_seen,
              last_seen, last_reward_source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)
         ON CONFLICT(user_id, token_norm, correct_form_norm) DO UPDATE SET
              correct_form = excluded.correct_form,
              positive_count = positive_count + ?5,
              negative_count = negative_count + ?6,
              learned_weight = MAX(-4.0, MIN(8.0, learned_weight + ?7)),
              last_seen = excluded.last_seen,
              last_reward_source = excluded.last_reward_source",
        params![
            user_id,
            token_norm,
            correct_form,
            correct_form_norm,
            positive_delta,
            negative_delta,
            weight_delta,
            now,
            source.trim(),
        ],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

pub fn load_weights(pool: &DbPool, user_id: &str) -> HashMap<PolicyKey, Tier2PolicyWeight> {
    let Ok(conn) = pool.get() else {
        return HashMap::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT token_norm, correct_form, correct_form_norm,
                positive_count, negative_count, learned_weight, first_seen,
                last_seen, last_reward_source
           FROM tier2_policy_weights
          WHERE user_id = ?1",
    ) else {
        return HashMap::new();
    };
    let Ok(rows) = stmt.query_map(params![user_id], |row| {
        Ok(Tier2PolicyWeight {
            token_norm: row.get(0)?,
            correct_form: row.get(1)?,
            correct_form_norm: row.get(2)?,
            positive_count: row.get(3)?,
            negative_count: row.get(4)?,
            learned_weight: row.get(5)?,
            first_seen: row.get(6)?,
            last_seen: row.get(7)?,
            last_reward_source: row.get(8)?,
        })
    }) else {
        return HashMap::new();
    };

    rows.filter_map(|row| row.ok())
        .map(|weight| {
            (
                (weight.token_norm.clone(), weight.correct_form_norm.clone()),
                weight,
            )
        })
        .collect()
}

pub fn record_decisions(
    pool: &DbPool,
    user_id: &str,
    recording_id: &str,
    result: &ApplyResult,
) -> usize {
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let now = now_ms();
    let mut inserted = 0usize;
    for trace in &result.traces {
        if insert_trace(&conn, user_id, recording_id, now, trace) {
            inserted += 1;
        }
    }
    inserted
}

/// Record decision events for applied matches (exact STT alias replacements).
///
/// Unlike `record_decisions` which logs from scoring traces, this logs the
/// applied matches themselves so that `mark_removed_feedback` can penalise
/// the originating alias when the user reverts a correction.
pub fn record_applied_matches(
    pool: &DbPool,
    user_id: &str,
    recording_id: &str,
    result: &ApplyResult,
) -> usize {
    let Ok(conn) = pool.get() else {
        return 0;
    };
    let now = now_ms();
    let mut inserted = 0usize;
    for m in &result.matches {
        if m.kind != MatchKind::Exact {
            continue;
        }
        let id = Uuid::new_v4().to_string();
        let ok = conn
            .execute(
                "INSERT INTO tier2_decision_events
                     (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
                      chosen_form, chosen_form_norm, second_form, scorer_kind, score,
                      second_score, margin, deterministic_score, onnx_score,
                      policy_score, policy_boost, applied)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 'stt_alias', 1.0,
                         0.0, 1.0, 1.0, NULL, NULL, 0.0, 1)",
                params![
                    id,
                    user_id,
                    recording_id,
                    now,
                    m.transcript_form,
                    normalize_token(&m.transcript_form),
                    m.correct_form,
                    normalize_token(&m.correct_form),
                ],
            )
            .map(|rows| rows > 0)
            .unwrap_or(false);
        if ok {
            inserted += 1;
        }
    }
    inserted
}

fn insert_trace(
    conn: &rusqlite::Connection,
    user_id: &str,
    recording_id: &str,
    now: i64,
    trace: &ScoringTrace,
) -> bool {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO tier2_decision_events
             (id, user_id, recording_id, timestamp_ms, token_text, token_norm,
              chosen_form, chosen_form_norm, second_form, scorer_kind, score,
              second_score, margin, deterministic_score, onnx_score,
              policy_score, policy_boost, applied)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18)",
        params![
            id,
            user_id,
            recording_id,
            now,
            trace.token,
            normalize_token(&trace.token),
            trace.candidate,
            normalize_token(&trace.candidate),
            trace.second_candidate,
            trace.kind.as_str(),
            trace.score,
            trace.second_score,
            trace.margin,
            trace.deterministic_score,
            trace.onnx_score,
            trace.policy_score,
            trace.policy_boost,
            i64::from(trace.applied),
        ],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

pub fn mark_feedback(
    pool: &DbPool,
    user_id: &str,
    recording_id: &str,
    user_kept: &str,
) -> FeedbackMarkResult {
    let events: Vec<(String, String, String)> = {
        let Ok(conn) = pool.get() else {
            return FeedbackMarkResult::default();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, token_text, chosen_form
               FROM tier2_decision_events
              WHERE user_id = ?1
                AND recording_id = ?2
                AND applied = 1
                AND reward_outcome IS NULL
                AND chosen_form IS NOT NULL",
        ) else {
            return FeedbackMarkResult::default();
        };
        let Ok(rows) = stmt.query_map(params![user_id, recording_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) else {
            return FeedbackMarkResult::default();
        };
        rows.flatten().collect()
    };

    let mut result = FeedbackMarkResult::default();
    for row in events {
        let (event_id, token_text, chosen_form) = row;
        let kept = contains_term(user_kept, &chosen_form);
        let updated = if kept {
            reward_mapping(
                pool,
                user_id,
                &token_text,
                &chosen_form,
                0.35,
                "feedback_survived",
            )
        } else {
            penalize_mapping(
                pool,
                user_id,
                &token_text,
                &chosen_form,
                1.0,
                "feedback_removed",
            )
        };
        if updated {
            if kept {
                result.rewarded += 1;
            } else {
                result.penalized += 1;
            }
            let outcome = if kept { "positive" } else { "negative" };
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
    }
    result
}

fn contains_term(text: &str, term: &str) -> bool {
    let needle = normalize_token(term);
    if needle.is_empty() {
        return false;
    }
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .any(|part| normalize_token(part) == needle)
}

pub fn status(
    pool: &DbPool,
    user_id: &str,
    last_onnx_trained_at: Option<i64>,
) -> Tier2PolicyStatus {
    let Ok(conn) = pool.get() else {
        return Tier2PolicyStatus::default();
    };
    let mut status = conn
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(positive_count), 0),
                    COALESCE(SUM(negative_count), 0),
                    MAX(last_seen)
               FROM tier2_policy_weights
              WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok(Tier2PolicyStatus {
                    policy_pair_count: row.get(0)?,
                    positive_reward_count: row.get(1)?,
                    negative_reward_count: row.get(2)?,
                    last_policy_update_at: row.get(3)?,
                    rewards_since_onnx: 0,
                })
            },
        )
        .unwrap_or_default();

    if let Some(trained_at) = last_onnx_trained_at {
        status.rewards_since_onnx = conn
            .query_row(
                "SELECT COALESCE(SUM(positive_count), 0)
                   FROM tier2_policy_weights
                  WHERE user_id = ?1 AND last_seen > ?2",
                params![user_id, trained_at],
                |row| row.get(0),
            )
            .unwrap_or(0);
    } else {
        status.rewards_since_onnx = status.positive_reward_count;
    }

    status
}

pub fn latest_decision_for_recording(
    pool: &DbPool,
    user_id: &str,
    recording_id: &str,
) -> Option<(String, String)> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT token_text, chosen_form
           FROM tier2_decision_events
          WHERE user_id = ?1
            AND recording_id = ?2
            AND applied = 1
            AND chosen_form IS NOT NULL
          ORDER BY timestamp_ms DESC
          LIMIT 1",
        params![user_id, recording_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn policy_score(weight: &Tier2PolicyWeight) -> Option<f64> {
    if weight.positive_count <= 0 {
        return None;
    }
    let net = weight.positive_count as f64 - (weight.negative_count as f64 * 1.5);
    if net <= 0.0 || weight.learned_weight <= 0.0 {
        return Some(0.0);
    }
    Some((0.90 + net.ln_1p() * 0.035 + weight.learned_weight.max(0.0) * 0.02).min(0.99))
}

#[cfg(test)]
mod tests {
    use r2d2_sqlite::SqliteConnectionManager;

    use super::*;
    use crate::store::stt_replacements::MatchKind;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE recordings (id TEXT PRIMARY KEY);
             INSERT INTO recordings(id) VALUES ('r1');
             CREATE TABLE tier2_policy_weights (
                user_id TEXT NOT NULL,
                token_norm TEXT NOT NULL,
                correct_form TEXT NOT NULL,
                correct_form_norm TEXT NOT NULL,
                positive_count INTEGER NOT NULL DEFAULT 0,
                negative_count INTEGER NOT NULL DEFAULT 0,
                learned_weight REAL NOT NULL DEFAULT 0.0,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                last_reward_source TEXT,
                PRIMARY KEY (user_id, token_norm, correct_form_norm)
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
    fn reward_and_penalty_are_scoped_and_idempotent() {
        let pool = mem_pool();
        assert!(reward_mapping(
            &pool, "u1", "bimmicop", "Macobs", 1.0, "test"
        ));
        assert!(reward_mapping(
            &pool, "u1", "bimmicop", "Macobs", 1.0, "test"
        ));
        assert!(penalize_mapping(
            &pool, "u1", "bimmicop", "Macobs", 1.0, "test"
        ));

        let weights = load_weights(&pool, "u1");
        let weight = weights
            .get(&("bimmicop".to_string(), "macobs".to_string()))
            .unwrap();
        assert_eq!(weight.positive_count, 2);
        assert_eq!(weight.negative_count, 1);
        assert!(weight.learned_weight > 0.0);
    }

    #[test]
    fn status_does_not_expose_raw_pairs() {
        let pool = mem_pool();
        reward_mapping(&pool, "u1", "bimmicop", "Macobs", 1.0, "test");
        let status = status(&pool, "u1", None);
        assert_eq!(status.policy_pair_count, 1);
        assert_eq!(status.positive_reward_count, 1);
        assert_eq!(status.negative_reward_count, 0);
    }

    #[test]
    fn common_word_sources_are_not_rewarded() {
        let pool = mem_pool();
        assert!(!reward_mapping(&pool, "u1", "main", "Macobs", 1.0, "test"));
        assert!(load_weights(&pool, "u1").is_empty());
    }

    #[test]
    fn decision_feedback_penalizes_removed_mapping() {
        let pool = mem_pool();
        let result = ApplyResult {
            text: "Macobs ka data bhejo".to_string(),
            matches: vec![],
            traces: vec![ScoringTrace {
                token: "bimmicop".to_string(),
                candidate: "Macobs".to_string(),
                second_candidate: None,
                kind: MatchKind::Tier2Policy,
                score: 0.92,
                second_score: 0.0,
                margin: 0.92,
                deterministic_score: 0.2,
                onnx_score: None,
                policy_score: Some(0.92),
                policy_boost: 0.72,
                applied: true,
            }],
        };
        assert_eq!(record_decisions(&pool, "u1", "r1", &result), 1);
        let feedback = mark_feedback(&pool, "u1", "r1", "dusra text");
        assert_eq!(feedback.penalized, 1);
        assert_eq!(feedback.rewarded, 0);
    }

    #[test]
    fn record_applied_matches_inserts_stt_alias_events() {
        use crate::store::stt_replacements::AppliedMatch;

        let pool = mem_pool();
        let result = ApplyResult {
            text: "EMIAC mein kaam hai".to_string(),
            matches: vec![AppliedMatch {
                transcript_form: "mac".to_string(),
                correct_form: "EMIAC".to_string(),
                kind: MatchKind::Exact,
            }],
            traces: vec![],
        };
        assert_eq!(record_applied_matches(&pool, "u1", "r1", &result), 1);

        // Verify the event was inserted with scorer_kind = 'stt_alias'
        let conn = pool.get().unwrap();
        let scorer: String = conn
            .query_row(
                "SELECT scorer_kind FROM tier2_decision_events
                  WHERE user_id = 'u1' AND recording_id = 'r1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scorer, "stt_alias");
    }

    #[test]
    fn record_applied_matches_skips_non_exact() {
        use crate::store::stt_replacements::AppliedMatch;

        let pool = mem_pool();
        let result = ApplyResult {
            text: "Macobs ka data".to_string(),
            matches: vec![AppliedMatch {
                transcript_form: "macops".to_string(),
                correct_form: "Macobs".to_string(),
                kind: MatchKind::Tier2EditPolicy,
            }],
            traces: vec![],
        };
        assert_eq!(
            record_applied_matches(&pool, "u1", "r1", &result),
            0,
            "non-exact matches should not be recorded as stt_alias events"
        );
    }
}
