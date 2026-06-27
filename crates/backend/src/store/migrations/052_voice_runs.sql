CREATE TABLE IF NOT EXISTS voice_runs (
    run_id                 TEXT PRIMARY KEY,
    user_id                TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    audio_id               TEXT,
    mode                   TEXT NOT NULL DEFAULT 'normal',
    target_app             TEXT,
    status                 TEXT NOT NULL,
    wav_bytes              INTEGER NOT NULL DEFAULT 0,
    duration_ms            INTEGER NOT NULL DEFAULT 0,
    pre_transcript         TEXT,
    recording_id           TEXT REFERENCES recordings(id) ON DELETE SET NULL,
    error_code             TEXT,
    error_message          TEXT,
    retryable              INTEGER NOT NULL DEFAULT 0,
    owned_by_airnote       INTEGER NOT NULL DEFAULT 0,
    attempt_count          INTEGER NOT NULL DEFAULT 1,
    completed_successfully INTEGER NOT NULL DEFAULT 0,
    paste_success          INTEGER,
    diagnostic_json        TEXT,
    created_at_ms          INTEGER NOT NULL,
    updated_at_ms          INTEGER NOT NULL,
    failed_at_ms           INTEGER,
    completed_at_ms        INTEGER
);

CREATE INDEX IF NOT EXISTS idx_voice_runs_user_status_time
    ON voice_runs(user_id, status, updated_at_ms DESC);

CREATE INDEX IF NOT EXISTS idx_voice_runs_audio
    ON voice_runs(audio_id);
