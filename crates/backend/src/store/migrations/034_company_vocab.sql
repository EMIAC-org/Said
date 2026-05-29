CREATE TABLE IF NOT EXISTS company_bucket_state (
    user_id        TEXT PRIMARY KEY,
    org_id         TEXT,
    version        INTEGER NOT NULL DEFAULT 0,
    bucket_hash    TEXT,
    last_checked_at INTEGER,
    last_synced_at  INTEGER,
    last_error      TEXT
);

CREATE TABLE IF NOT EXISTS company_vocabulary (
    user_id     TEXT NOT NULL,
    org_id      TEXT NOT NULL,
    term        TEXT NOT NULL,
    term_norm   TEXT NOT NULL,
    term_type   TEXT,
    language    TEXT,
    weight      REAL NOT NULL DEFAULT 1.0,
    priority    INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'approved',
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (user_id, org_id, term_norm)
);
CREATE INDEX IF NOT EXISTS idx_company_vocabulary_user_priority
    ON company_vocabulary(user_id, priority DESC, weight DESC, term);

CREATE TABLE IF NOT EXISTS company_stt_replacements (
    user_id          TEXT NOT NULL,
    org_id           TEXT NOT NULL,
    transcript_form  TEXT NOT NULL,
    transcript_norm  TEXT NOT NULL,
    correct_form     TEXT NOT NULL,
    correct_norm     TEXT NOT NULL,
    language         TEXT,
    weight           REAL NOT NULL DEFAULT 1.0,
    status           TEXT NOT NULL DEFAULT 'approved',
    safety_status    TEXT NOT NULL DEFAULT 'unknown',
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (user_id, org_id, transcript_norm, correct_norm)
);
CREATE INDEX IF NOT EXISTS idx_company_stt_replacements_user
    ON company_stt_replacements(user_id, correct_norm, transcript_norm);

CREATE TABLE IF NOT EXISTS company_vocab_tombstones (
    user_id        TEXT NOT NULL,
    org_id         TEXT NOT NULL,
    entity_kind    TEXT NOT NULL,
    entity_norm    TEXT NOT NULL,
    bucket_version INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (user_id, org_id, entity_kind, entity_norm)
);

CREATE TABLE IF NOT EXISTS company_vocab_upload_state (
    user_id          TEXT PRIMARY KEY,
    last_uploaded_at INTEGER,
    last_error       TEXT
);
