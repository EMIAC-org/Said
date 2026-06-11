-- Desktop telemetry outbox: per-run summaries + daily rollups (metadata only, no raw text).

CREATE TABLE IF NOT EXISTS telemetry_run_summaries (
    run_id                  TEXT PRIMARY KEY,
    recording_id            TEXT,
    user_id                 TEXT NOT NULL,
    device_id               TEXT,
    mode                    TEXT NOT NULL DEFAULT 'normal_voice',
    target_app              TEXT,
    platform                TEXT,
    app_version             TEXT,
    machine_class           TEXT,
    audio_seconds           REAL,
    word_count              INTEGER,
    char_count              INTEGER,
    transcribe_ms           INTEGER,
    embed_ms                INTEGER,
    polish_ms               INTEGER,
    total_ms                INTEGER,
    paste_ms                INTEGER,
    success                 INTEGER NOT NULL DEFAULT 0,
    error_code              TEXT,
    used_clipboard_fallback INTEGER NOT NULL DEFAULT 0,
    used_ws_pretranscript   INTEGER NOT NULL DEFAULT 0,
    used_http_stt_fallback  INTEGER NOT NULL DEFAULT 0,
    edit_detected           INTEGER NOT NULL DEFAULT 0,
    edit_bucket             TEXT NOT NULL DEFAULT 'none',
    edit_distance_chars     INTEGER,
    edit_distance_words     INTEGER,
    accepted_as_is          INTEGER NOT NULL DEFAULT 0,
    deleted_entire_output   INTEGER NOT NULL DEFAULT 0,
    re_recorded_quickly     INTEGER NOT NULL DEFAULT 0,
    learning_candidate      INTEGER NOT NULL DEFAULT 0,
    learning_modal_shown    INTEGER NOT NULL DEFAULT 0,
    learning_confirmed      INTEGER NOT NULL DEFAULT 0,
    learning_dismissed      INTEGER NOT NULL DEFAULT 0,
    server_learning_saved   INTEGER NOT NULL DEFAULT 0,
    server_learning_blocked INTEGER NOT NULL DEFAULT 0,
    has_numbers             INTEGER NOT NULL DEFAULT 0,
    has_currency            INTEGER NOT NULL DEFAULT 0,
    has_percent             INTEGER NOT NULL DEFAULT 0,
    has_email               INTEGER NOT NULL DEFAULT 0,
    has_url                 INTEGER NOT NULL DEFAULT 0,
    has_code_like_terms     INTEGER NOT NULL DEFAULT 0,
    mixed_language          INTEGER NOT NULL DEFAULT 0,
    protected_term_hit      INTEGER NOT NULL DEFAULT 0,
    status                  TEXT NOT NULL DEFAULT 'pending',
    created_at_ms           INTEGER NOT NULL,
    updated_at_ms           INTEGER NOT NULL,
    ready_at_ms             INTEGER,
    uploaded_at_ms          INTEGER
);
CREATE INDEX IF NOT EXISTS idx_telemetry_runs_status ON telemetry_run_summaries (status, updated_at_ms);
CREATE INDEX IF NOT EXISTS idx_telemetry_runs_user ON telemetry_run_summaries (user_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS telemetry_daily_rollups (
    user_id         TEXT NOT NULL,
    event_date      TEXT NOT NULL,
    mode            TEXT NOT NULL DEFAULT 'all',
    run_count       INTEGER NOT NULL DEFAULT 0,
    audio_seconds   REAL NOT NULL DEFAULT 0,
    accepted_count  INTEGER NOT NULL DEFAULT 0,
    edit_count      INTEGER NOT NULL DEFAULT 0,
    heavy_edit_count INTEGER NOT NULL DEFAULT 0,
    learning_modal_shown INTEGER NOT NULL DEFAULT 0,
    learning_confirmed INTEGER NOT NULL DEFAULT 0,
    failure_count   INTEGER NOT NULL DEFAULT 0,
    fallback_count  INTEGER NOT NULL DEFAULT 0,
    updated_at_ms   INTEGER NOT NULL,
    PRIMARY KEY (user_id, event_date, mode)
);

CREATE TABLE IF NOT EXISTS telemetry_upload_state (
    user_id             TEXT PRIMARY KEY,
    last_upload_at_ms   INTEGER,
    last_error          TEXT,
    pending_run_count   INTEGER NOT NULL DEFAULT 0,
    completed_since_upload INTEGER NOT NULL DEFAULT 0
);
