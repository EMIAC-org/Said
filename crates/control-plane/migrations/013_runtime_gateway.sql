-- Unified runtime gateway foundation.
-- Stores session/event metadata for mobile/server runtime without raw audio or raw text.

CREATE TABLE IF NOT EXISTS runtime_provider_credentials (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_scope         TEXT NOT NULL CHECK (owner_scope IN ('account', 'org')),
    account_id          UUID REFERENCES accounts(id) ON DELETE CASCADE,
    org_id              UUID REFERENCES orgs(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL CHECK (provider IN ('deepgram', 'groq', 'openai', 'gemini')),
    label               TEXT,
    encrypted_secret    TEXT,
    secret_digest       TEXT,
    status              TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'active', 'failed', 'revoked')),
    validation_status   TEXT NOT NULL DEFAULT 'untested' CHECK (validation_status IN ('untested', 'valid', 'invalid')),
    last_validated_at   TIMESTAMPTZ,
    revoked_at          TIMESTAMPTZ,
    created_by          UUID REFERENCES accounts(id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (owner_scope = 'account' AND account_id IS NOT NULL AND org_id IS NULL)
        OR
        (owner_scope = 'org' AND org_id IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_credentials_account
    ON runtime_provider_credentials (account_id, provider, status);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_credentials_org
    ON runtime_provider_credentials (org_id, provider, status);

CREATE TABLE IF NOT EXISTS runtime_sessions (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_token           UUID UNIQUE NOT NULL DEFAULT gen_random_uuid(),
    account_id              UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id                  UUID REFERENCES orgs(id) ON DELETE SET NULL,
    device_id               TEXT NOT NULL,
    client_request_id       TEXT NOT NULL,
    platform                TEXT NOT NULL DEFAULT 'ios',
    surface                 TEXT NOT NULL DEFAULT 'ios_keyboard',
    language_hint           TEXT NOT NULL DEFAULT 'auto',
    style                   TEXT NOT NULL DEFAULT 'work',
    status                  TEXT NOT NULL DEFAULT 'created',
    context_json            JSONB NOT NULL DEFAULT '{}'::jsonb,
    vocab_snapshot_hash     TEXT,
    current_vocab_hash      TEXT NOT NULL DEFAULT 'global-v0',
    streaming_enabled       BOOLEAN NOT NULL DEFAULT true,
    max_recording_seconds   INTEGER NOT NULL DEFAULT 60,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at              TIMESTAMPTZ NOT NULL,
    completed_at            TIMESTAMPTZ,
    UNIQUE (account_id, device_id, client_request_id)
);
CREATE INDEX IF NOT EXISTS idx_runtime_sessions_account_created
    ON runtime_sessions (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_sessions_device_created
    ON runtime_sessions (device_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_sessions_token
    ON runtime_sessions (session_token);

CREATE TABLE IF NOT EXISTS runtime_runs (
    id                         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id                 UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    account_id                 UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id                     UUID REFERENCES orgs(id) ON DELETE SET NULL,
    device_id                  TEXT NOT NULL,
    client_request_id          TEXT,
    mode                       TEXT NOT NULL DEFAULT 'mobile_server',
    status                     TEXT NOT NULL DEFAULT 'created',
    provider_credential_id     UUID REFERENCES runtime_provider_credentials(id) ON DELETE SET NULL,
    transcript_char_count      INTEGER NOT NULL DEFAULT 0,
    polished_char_count        INTEGER NOT NULL DEFAULT 0,
    audio_frame_count          INTEGER NOT NULL DEFAULT 0,
    audio_byte_count           INTEGER NOT NULL DEFAULT 0,
    latency_ms                 INTEGER,
    error_code                 TEXT,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at               TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_runtime_runs_account_created
    ON runtime_runs (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_runs_session
    ON runtime_runs (session_id, created_at DESC);

CREATE TABLE IF NOT EXISTS runtime_stage_events (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id          UUID REFERENCES runtime_runs(id) ON DELETE CASCADE,
    session_id      UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    account_id      UUID REFERENCES accounts(id) ON DELETE CASCADE,
    device_id       TEXT,
    stage           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'ok',
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_stage_events_run
    ON runtime_stage_events (run_id, occurred_at ASC);
CREATE INDEX IF NOT EXISTS idx_runtime_stage_events_account
    ON runtime_stage_events (account_id, occurred_at DESC);

CREATE TABLE IF NOT EXISTS runtime_events (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id          UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    account_id          UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id              UUID REFERENCES orgs(id) ON DELETE SET NULL,
    device_id           TEXT NOT NULL,
    client_event_id     TEXT,
    client_request_id   TEXT,
    build               TEXT,
    platform            TEXT NOT NULL DEFAULT 'ios',
    surface             TEXT NOT NULL DEFAULT 'ios_keyboard',
    event_type          TEXT NOT NULL,
    redacted_context    JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurred_at         TIMESTAMPTZ,
    received_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, client_event_id)
);
CREATE INDEX IF NOT EXISTS idx_runtime_events_account_received
    ON runtime_events (account_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_events_session_received
    ON runtime_events (session_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_events_type_received
    ON runtime_events (event_type, received_at DESC);

CREATE TABLE IF NOT EXISTS runtime_provider_usage (
    id                         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_credential_id     UUID REFERENCES runtime_provider_credentials(id) ON DELETE SET NULL,
    account_id                 UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id                     UUID REFERENCES orgs(id) ON DELETE SET NULL,
    run_id                     UUID REFERENCES runtime_runs(id) ON DELETE SET NULL,
    provider                   TEXT NOT NULL,
    operation                  TEXT NOT NULL,
    request_count              INTEGER NOT NULL DEFAULT 1,
    input_units                INTEGER NOT NULL DEFAULT 0,
    output_units               INTEGER NOT NULL DEFAULT 0,
    cost_micros                BIGINT NOT NULL DEFAULT 0,
    occurred_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_usage_account
    ON runtime_provider_usage (account_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_usage_credential
    ON runtime_provider_usage (provider_credential_id, occurred_at DESC);
