-- Server-side history for voice dictation sessions.
-- Stores transcript/output/edit text for signed-in users.
-- Raw audio and screen context are never stored here.

CREATE TABLE IF NOT EXISTS runtime_history_items (
    id                         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id                 UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id                     UUID REFERENCES orgs(id) ON DELETE SET NULL,
    run_id                     UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    client_run_id              TEXT,
    recording_id               TEXT,
    device_id                  TEXT,
    platform                   TEXT,
    app_version                TEXT,
    source                     TEXT NOT NULL DEFAULT 'desktop_voice',
    raw_transcript             TEXT,
    transcript                 TEXT,
    local_corrected_transcript TEXT,
    polished_output            TEXT,
    final_text                 TEXT,
    model_used                 TEXT,
    word_count                 INTEGER,
    recording_seconds          DOUBLE PRECISION,
    transcribe_ms              BIGINT,
    embed_ms                   BIGINT,
    polish_ms                  BIGINT,
    target_app                 TEXT,
    formatter_trace_json       JSONB NOT NULL DEFAULT '{}',
    resolver_trace_json        JSONB NOT NULL DEFAULT '{}',
    edit_feedback_json         JSONB NOT NULL DEFAULT '{}',
    privacy_json               JSONB NOT NULL DEFAULT '{}',
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at                 TIMESTAMPTZ
);

-- Idempotency: one row per (account, client_run_id or recording_id or id)
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_history_account_dedup
    ON runtime_history_items (account_id, COALESCE(client_run_id, recording_id, id::text));

CREATE INDEX IF NOT EXISTS idx_runtime_history_account_created
    ON runtime_history_items (account_id, created_at DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_runtime_history_run
    ON runtime_history_items (run_id)
    WHERE run_id IS NOT NULL;
