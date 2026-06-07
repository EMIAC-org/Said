-- AirNote Mobile Gateway — initial schema.
--
-- Self-contained: this service owns its own accounts/auth and never references
-- the control-plane (desktop/enterprise) database. Privacy stance: no raw audio
-- and no raw transcript/polished text is stored — only character counts and
-- redacted metadata. All DDL is idempotent (IF NOT EXISTS) so the embedded
-- migration is safe to re-run on every startup.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ── Accounts + bearer auth ────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS accounts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email           TEXT UNIQUE NOT NULL,
    password_hash   TEXT NOT NULL,
    license_tier    TEXT NOT NULL DEFAULT 'free',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Opaque bearer tokens (access + refresh). No JWT: simple, revocable rows.
CREATE TABLE IF NOT EXISTS auth_sessions (
    token       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL DEFAULT 'access' CHECK (kind IN ('access', 'refresh')),
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_account ON auth_sessions (account_id);

-- ── Devices ───────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS mobile_devices (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id           UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id            TEXT NOT NULL,
    platform             TEXT NOT NULL DEFAULT 'ios',
    app_version          TEXT,
    build_number         TEXT,
    build_channel        TEXT,
    os_version           TEXT,
    model                TEXT,
    locale               TEXT,
    permission_snapshot  JSONB NOT NULL DEFAULT '{}'::jsonb,
    revoked_at           TIMESTAMPTZ,
    last_seen_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, device_id)
);
CREATE INDEX IF NOT EXISTS idx_mobile_devices_account ON mobile_devices (account_id);

-- ── Voice sessions ────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS voice_sessions (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_token           UUID UNIQUE NOT NULL DEFAULT gen_random_uuid(),
    account_id              UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id               TEXT NOT NULL,
    client_request_id       TEXT NOT NULL,
    platform                TEXT NOT NULL DEFAULT 'ios',
    surface                 TEXT NOT NULL DEFAULT 'ios_keyboard',
    language_hint           TEXT NOT NULL DEFAULT 'auto',
    style                   TEXT NOT NULL DEFAULT 'work',
    status                  TEXT NOT NULL DEFAULT 'created',
    context_json            JSONB NOT NULL DEFAULT '{}'::jsonb,
    vocab_snapshot_hash     TEXT,
    current_vocab_hash      TEXT NOT NULL DEFAULT 'personal-v0',
    streaming_enabled       BOOLEAN NOT NULL DEFAULT true,
    max_recording_seconds   INTEGER NOT NULL DEFAULT 60,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at              TIMESTAMPTZ NOT NULL,
    completed_at            TIMESTAMPTZ,
    UNIQUE (account_id, device_id, client_request_id)
);
CREATE INDEX IF NOT EXISTS idx_voice_sessions_account_created
    ON voice_sessions (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_voice_sessions_token
    ON voice_sessions (session_token);

-- ── Voice runs (one per stream/batch dictation attempt) ───────────────────────

CREATE TABLE IF NOT EXISTS voice_runs (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id              UUID REFERENCES voice_sessions(id) ON DELETE SET NULL,
    account_id              UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id               TEXT NOT NULL,
    client_request_id       TEXT,
    mode                    TEXT NOT NULL DEFAULT 'stream',
    status                  TEXT NOT NULL DEFAULT 'created',
    language                TEXT,
    style                   TEXT,
    transcript_char_count   INTEGER NOT NULL DEFAULT 0,
    polished_char_count     INTEGER NOT NULL DEFAULT 0,
    audio_frame_count       INTEGER NOT NULL DEFAULT 0,
    audio_byte_count        INTEGER NOT NULL DEFAULT 0,
    stt_ms                  INTEGER,
    polish_ms               INTEGER,
    latency_ms              INTEGER,
    error_code              TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at            TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_voice_runs_account_created
    ON voice_runs (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_voice_runs_session
    ON voice_runs (session_id, created_at DESC);

-- ── Per-stage events (server-side pipeline trace, redacted) ───────────────────

CREATE TABLE IF NOT EXISTS voice_stage_events (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id       UUID REFERENCES voice_runs(id) ON DELETE CASCADE,
    session_id   UUID REFERENCES voice_sessions(id) ON DELETE SET NULL,
    account_id   UUID REFERENCES accounts(id) ON DELETE CASCADE,
    device_id    TEXT,
    stage        TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'ok',
    metadata     JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_voice_stage_events_run
    ON voice_stage_events (run_id, occurred_at ASC);

-- ── Client events (privacy-safe product/setup telemetry) ──────────────────────

CREATE TABLE IF NOT EXISTS voice_events (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id          UUID REFERENCES voice_sessions(id) ON DELETE SET NULL,
    account_id          UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
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
CREATE INDEX IF NOT EXISTS idx_voice_events_account_received
    ON voice_events (account_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_voice_events_type_received
    ON voice_events (event_type, received_at DESC);

-- ── Provider cost ledger (Deepgram / Groq usage off the hot path) ─────────────

CREATE TABLE IF NOT EXISTS provider_usage (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    run_id          UUID REFERENCES voice_runs(id) ON DELETE SET NULL,
    provider        TEXT NOT NULL,
    operation       TEXT NOT NULL,
    request_count   INTEGER NOT NULL DEFAULT 1,
    input_units     INTEGER NOT NULL DEFAULT 0,
    output_units    INTEGER NOT NULL DEFAULT 0,
    cost_micros     BIGINT NOT NULL DEFAULT 0,
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_provider_usage_account
    ON provider_usage (account_id, occurred_at DESC);

-- ── Personal vocabulary (explicit-learning, v1 personal scope only) ───────────

CREATE TABLE IF NOT EXISTS vocab_terms (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    term            TEXT NOT NULL,
    spoken_aliases  JSONB NOT NULL DEFAULT '[]'::jsonb,
    term_type       TEXT,
    priority        REAL NOT NULL DEFAULT 0.5,
    source          TEXT NOT NULL DEFAULT 'user',
    archived_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, term)
);
CREATE INDEX IF NOT EXISTS idx_vocab_terms_account
    ON vocab_terms (account_id) WHERE archived_at IS NULL;
