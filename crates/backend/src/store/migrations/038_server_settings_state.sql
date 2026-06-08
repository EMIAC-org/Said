CREATE TABLE IF NOT EXISTS server_settings_state (
    user_id              TEXT NOT NULL,
    server_account_id    TEXT NOT NULL,
    settings_json        TEXT NOT NULL DEFAULT '{}',
    server_version       INTEGER NOT NULL DEFAULT 0,
    last_synced_at_ms    INTEGER,
    last_error           TEXT,
    PRIMARY KEY (user_id, server_account_id)
);
