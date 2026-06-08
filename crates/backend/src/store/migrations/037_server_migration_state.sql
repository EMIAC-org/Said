-- Tracks the state of the first-launch migration from local SQLite to server.
-- One row per (user, server_account, migration_version).

CREATE TABLE IF NOT EXISTS server_migration_state (
    user_id                    TEXT NOT NULL,
    server_account_id          TEXT NOT NULL,
    migration_version          INTEGER NOT NULL DEFAULT 1,
    status                     TEXT NOT NULL DEFAULT 'not_started',
    started_at_ms              INTEGER,
    completed_at_ms            INTEGER,
    last_attempt_at_ms         INTEGER,
    last_error                 TEXT,
    uploaded_history_count     INTEGER NOT NULL DEFAULT 0,
    uploaded_vocab_count       INTEGER NOT NULL DEFAULT 0,
    uploaded_alias_count       INTEGER NOT NULL DEFAULT 0,
    uploaded_email_count       INTEGER NOT NULL DEFAULT 0,
    uploaded_credentials_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, server_account_id, migration_version),
    CHECK (status IN ('not_started', 'running', 'partial', 'completed', 'failed'))
);
