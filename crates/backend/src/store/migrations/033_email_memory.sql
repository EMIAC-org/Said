CREATE TABLE IF NOT EXISTS email_memories (
    user_id        TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    email          TEXT NOT NULL,
    email_norm     TEXT NOT NULL,
    source_hint    TEXT,
    source_norm    TEXT,
    positive_count INTEGER NOT NULL DEFAULT 1,
    first_seen     INTEGER NOT NULL,
    last_seen      INTEGER NOT NULL,
    PRIMARY KEY (user_id, email_norm)
);

CREATE INDEX IF NOT EXISTS idx_email_memories_user_seen
    ON email_memories (user_id, last_seen DESC);

CREATE INDEX IF NOT EXISTS idx_email_memories_user_source
    ON email_memories (user_id, source_norm);
