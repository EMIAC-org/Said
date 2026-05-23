-- Tier 2 local policy learning.
--
-- `tier2_policy_weights` is the immediate local memory used by the Tier 2
-- scorer. It is updated after confirmed user edits and read on the next
-- dictation, so learning does not wait for ONNX retraining.
--
-- `tier2_decision_events` stores local traces for applied/rejected Tier 2
-- decisions so feedback can reward or penalize a specific scorer action.

CREATE TABLE IF NOT EXISTS tier2_policy_weights (
    user_id            TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    token_norm         TEXT NOT NULL,
    correct_form       TEXT NOT NULL,
    correct_form_norm  TEXT NOT NULL,
    positive_count     INTEGER NOT NULL DEFAULT 0,
    negative_count     INTEGER NOT NULL DEFAULT 0,
    learned_weight     REAL NOT NULL DEFAULT 0.0,
    first_seen         INTEGER NOT NULL,
    last_seen          INTEGER NOT NULL,
    last_reward_source TEXT,
    PRIMARY KEY (user_id, token_norm, correct_form_norm)
);

CREATE INDEX IF NOT EXISTS idx_tier2_policy_user_token
    ON tier2_policy_weights (user_id, token_norm);

CREATE INDEX IF NOT EXISTS idx_tier2_policy_user_seen
    ON tier2_policy_weights (user_id, last_seen DESC);

CREATE TABLE IF NOT EXISTS tier2_decision_events (
    id                  TEXT PRIMARY KEY,
    user_id             TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    recording_id        TEXT REFERENCES recordings(id) ON DELETE SET NULL,
    timestamp_ms        INTEGER NOT NULL,
    token_text          TEXT NOT NULL,
    token_norm          TEXT NOT NULL,
    chosen_form         TEXT,
    chosen_form_norm    TEXT,
    second_form         TEXT,
    scorer_kind         TEXT NOT NULL,
    score               REAL NOT NULL DEFAULT 0.0,
    second_score        REAL,
    margin              REAL NOT NULL DEFAULT 0.0,
    deterministic_score REAL,
    onnx_score          REAL,
    policy_score        REAL,
    policy_boost        REAL NOT NULL DEFAULT 0.0,
    applied             INTEGER NOT NULL DEFAULT 0,
    reward_outcome      TEXT,
    feedback_at         INTEGER
);

CREATE INDEX IF NOT EXISTS idx_tier2_decision_recording
    ON tier2_decision_events (recording_id);

CREATE INDEX IF NOT EXISTS idx_tier2_decision_user_token
    ON tier2_decision_events (user_id, token_norm, timestamp_ms DESC);
