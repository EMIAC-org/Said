-- Background observability outbox: dictation plaintext + edit feedback for control plane.

CREATE TABLE IF NOT EXISTS observability_outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         TEXT NOT NULL,
    op              TEXT NOT NULL,
    recording_id    TEXT,
    payload_json    TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    attempts        INTEGER NOT NULL DEFAULT 0,
    created_at_ms   INTEGER NOT NULL,
    last_attempt_ms INTEGER,
    last_error      TEXT
);

CREATE INDEX IF NOT EXISTS idx_observability_outbox_status
    ON observability_outbox (status, created_at_ms);

CREATE INDEX IF NOT EXISTS idx_observability_outbox_user
    ON observability_outbox (user_id, created_at_ms DESC);
