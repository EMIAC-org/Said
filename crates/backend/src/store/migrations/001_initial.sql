-- Voice Polish local SQLite schema (migration 001)
-- All user data lives here — never leaves the Mac.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- ── Local user (v1: single auto-created account) ─────────────────────────────
CREATE TABLE IF NOT EXISTS local_user (
    id            TEXT PRIMARY KEY,
    email         TEXT NOT NULL,
    cloud_token   TEXT,
    license_tier  TEXT NOT NULL DEFAULT 'free',
    created_at    INTEGER NOT NULL
);

-- ── Per-user preferences (1:1) ───────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS preferences (
    user_id              TEXT PRIMARY KEY REFERENCES local_user(id),
    selected_model       TEXT NOT NULL DEFAULT 'smart',
    tone_preset          TEXT NOT NULL DEFAULT 'neutral',
    custom_prompt        TEXT,
    language             TEXT NOT NULL DEFAULT 'auto',
    auto_paste           INTEGER NOT NULL DEFAULT 1,
    edit_capture         INTEGER NOT NULL DEFAULT 1,
    polish_text_hotkey   TEXT NOT NULL DEFAULT 'cmd+shift+p',
    updated_at           INTEGER NOT NULL
);

-- ── Rolling recording history ────────────────────────────────────────────────
-- Best-effort 1-day retention via the background sweep in main.rs (see
-- cleanup_old_recordings). The sweep rarely fires in short sessions, so rows
-- usually outlive a day — this table is NOT a durable store, don't treat it as one.
CREATE TABLE IF NOT EXISTS recordings (
    id                TEXT PRIMARY KEY,
    user_id           TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    timestamp_ms      INTEGER NOT NULL,
    transcript        TEXT NOT NULL,
    polished          TEXT NOT NULL,
    final_text        TEXT,
    word_count        INTEGER NOT NULL,
    recording_seconds REAL NOT NULL,
    model_used        TEXT NOT NULL,
    confidence        REAL,
    transcribe_ms     INTEGER,
    embed_ms          INTEGER,
    polish_ms         INTEGER,
    target_app        TEXT,
    edit_count        INTEGER NOT NULL DEFAULT 0,
    source            TEXT NOT NULL DEFAULT 'voice'
);
CREATE INDEX IF NOT EXISTS idx_rec_user_time ON recordings (user_id, timestamp_ms DESC);

-- ── Edit events (PERMANENT — learning corpus, never cleaned) ─────────────────
CREATE TABLE IF NOT EXISTS edit_events (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    recording_id  TEXT REFERENCES recordings(id) ON DELETE SET NULL,
    timestamp_ms  INTEGER NOT NULL,
    transcript    TEXT NOT NULL,
    ai_output     TEXT NOT NULL,
    user_kept     TEXT NOT NULL,
    target_app    TEXT,
    embedding_id  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_edit_user_time ON edit_events (user_id, timestamp_ms DESC);

-- ── Persistent embedding cache ────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS embedding_cache (
    text_hash   TEXT PRIMARY KEY,
    embedding   BLOB NOT NULL,
    created_at  INTEGER NOT NULL
);
