-- Migration 058 collided with an older dev-branch migration number. Recreate
-- the review queue at a fresh version so databases already marked 58 recover.
CREATE TABLE IF NOT EXISTS edit_review_sessions (
    id                     TEXT PRIMARY KEY,
    user_id                TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    recording_id           TEXT NOT NULL,
    ai_output              TEXT NOT NULL,
    user_kept              TEXT NOT NULL,
    review_candidates_json TEXT NOT NULL,
    detected_changes_json  TEXT NOT NULL,
    created_at_ms          INTEGER NOT NULL,
    resolved_at_ms         INTEGER,
    status                 INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_edit_review_recording_pending
    ON edit_review_sessions (user_id, recording_id)
    WHERE status = 0;

CREATE INDEX IF NOT EXISTS idx_edit_review_fifo
    ON edit_review_sessions (user_id, status, created_at_ms ASC);
