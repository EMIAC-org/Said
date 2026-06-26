CREATE TABLE IF NOT EXISTS local_profile_summary (
    user_id            TEXT PRIMARY KEY REFERENCES local_user(id) ON DELETE CASCADE,
    profile_markdown   TEXT NOT NULL DEFAULT '',
    source_hash        TEXT NOT NULL DEFAULT '',
    source_counts_json TEXT NOT NULL DEFAULT '{}',
    version            INTEGER NOT NULL DEFAULT 0,
    updated_at         INTEGER NOT NULL
);

