CREATE TABLE IF NOT EXISTS prompt_templates (
    user_id      TEXT NOT NULL,
    kind         TEXT NOT NULL,
    title        TEXT NOT NULL,
    base_version TEXT NOT NULL,
    active_body  TEXT NOT NULL,
    draft_body   TEXT,
    updated_at   INTEGER NOT NULL,
    applied_at   INTEGER,
    PRIMARY KEY (user_id, kind),
    FOREIGN KEY (user_id) REFERENCES local_user(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS prompt_template_events (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    kind          TEXT NOT NULL,
    event_type    TEXT NOT NULL,
    body_snapshot TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES local_user(id) ON DELETE CASCADE
);
