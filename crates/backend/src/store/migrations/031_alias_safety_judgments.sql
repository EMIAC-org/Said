CREATE TABLE IF NOT EXISTS alias_safety_judgments (
    user_id      TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    source_norm  TEXT NOT NULL,
    verdict      TEXT NOT NULL,
    confidence   REAL NOT NULL DEFAULT 0.0,
    provider     TEXT NOT NULL DEFAULT 'local',
    model        TEXT NOT NULL DEFAULT '',
    reason       TEXT NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, source_norm)
);

CREATE INDEX IF NOT EXISTS idx_alias_safety_user_verdict
    ON alias_safety_judgments (user_id, verdict);
