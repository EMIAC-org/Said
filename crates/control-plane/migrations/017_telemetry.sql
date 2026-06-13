-- Runtime telemetry analytics (separate from user-facing history).

CREATE TABLE IF NOT EXISTS runtime_telemetry_runs (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id              UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id                  UUID REFERENCES orgs(id) ON DELETE SET NULL,
    run_id                  TEXT NOT NULL,
    recording_id            TEXT,
    device_id               TEXT,
    mode                    TEXT NOT NULL DEFAULT 'normal_voice',
    target_app              TEXT,
    platform                TEXT,
    app_version             TEXT,
    machine_class           TEXT,
    audio_seconds           DOUBLE PRECISION,
    word_count              INTEGER,
    char_count              INTEGER,
    transcribe_ms           INTEGER,
    embed_ms                INTEGER,
    polish_ms               INTEGER,
    total_ms                INTEGER,
    paste_ms                INTEGER,
    success                 BOOLEAN NOT NULL DEFAULT FALSE,
    error_code              TEXT,
    used_clipboard_fallback BOOLEAN NOT NULL DEFAULT FALSE,
    used_ws_pretranscript   BOOLEAN NOT NULL DEFAULT FALSE,
    used_http_stt_fallback  BOOLEAN NOT NULL DEFAULT FALSE,
    edit_detected           BOOLEAN NOT NULL DEFAULT FALSE,
    edit_bucket             TEXT NOT NULL DEFAULT 'none',
    edit_distance_chars     INTEGER,
    edit_distance_words     INTEGER,
    accepted_as_is          BOOLEAN NOT NULL DEFAULT FALSE,
    deleted_entire_output   BOOLEAN NOT NULL DEFAULT FALSE,
    re_recorded_quickly     BOOLEAN NOT NULL DEFAULT FALSE,
    learning_candidate      BOOLEAN NOT NULL DEFAULT FALSE,
    learning_modal_shown    BOOLEAN NOT NULL DEFAULT FALSE,
    learning_confirmed      BOOLEAN NOT NULL DEFAULT FALSE,
    learning_dismissed      BOOLEAN NOT NULL DEFAULT FALSE,
    server_learning_saved   BOOLEAN NOT NULL DEFAULT FALSE,
    server_learning_blocked BOOLEAN NOT NULL DEFAULT FALSE,
    has_numbers             BOOLEAN NOT NULL DEFAULT FALSE,
    has_currency            BOOLEAN NOT NULL DEFAULT FALSE,
    has_percent             BOOLEAN NOT NULL DEFAULT FALSE,
    has_email               BOOLEAN NOT NULL DEFAULT FALSE,
    has_url                 BOOLEAN NOT NULL DEFAULT FALSE,
    has_code_like_terms     BOOLEAN NOT NULL DEFAULT FALSE,
    mixed_language          BOOLEAN NOT NULL DEFAULT FALSE,
    protected_term_hit      BOOLEAN NOT NULL DEFAULT FALSE,
    client_version          TEXT,
    received_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    event_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, run_id)
);
CREATE INDEX IF NOT EXISTS idx_runtime_telemetry_runs_org_date
    ON runtime_telemetry_runs (org_id, event_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_telemetry_runs_account_date
    ON runtime_telemetry_runs (account_id, event_at DESC);

CREATE TABLE IF NOT EXISTS runtime_telemetry_daily (
    org_id              UUID REFERENCES orgs(id) ON DELETE CASCADE,
    account_id          UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    event_date          DATE NOT NULL,
    mode                TEXT NOT NULL DEFAULT 'all',
    run_count           INTEGER NOT NULL DEFAULT 0,
    audio_seconds       DOUBLE PRECISION NOT NULL DEFAULT 0,
    accepted_count      INTEGER NOT NULL DEFAULT 0,
    edit_count          INTEGER NOT NULL DEFAULT 0,
    heavy_edit_count    INTEGER NOT NULL DEFAULT 0,
    learning_modal_shown INTEGER NOT NULL DEFAULT 0,
    learning_confirmed  INTEGER NOT NULL DEFAULT 0,
    failure_count       INTEGER NOT NULL DEFAULT 0,
    fallback_count      INTEGER NOT NULL DEFAULT 0,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, event_date, mode)
);
CREATE INDEX IF NOT EXISTS idx_runtime_telemetry_daily_org
    ON runtime_telemetry_daily (org_id, event_date DESC);

CREATE TABLE IF NOT EXISTS runtime_telemetry_uploads (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id          UUID REFERENCES orgs(id) ON DELETE SET NULL,
    device_id       TEXT,
    client_version  TEXT,
    run_count       INTEGER NOT NULL DEFAULT 0,
    rollup_count    INTEGER NOT NULL DEFAULT 0,
    accepted_count  INTEGER NOT NULL DEFAULT 0,
    rejected_count  INTEGER NOT NULL DEFAULT 0,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
