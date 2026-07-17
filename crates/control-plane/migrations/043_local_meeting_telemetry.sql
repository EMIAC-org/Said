CREATE TABLE IF NOT EXISTS local_meeting_sessions (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id               UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    client_session_id        TEXT NOT NULL,
    status                   TEXT NOT NULL,
    started_at               TIMESTAMPTZ NOT NULL,
    ended_at                 TIMESTAMPTZ NOT NULL,
    duration_seconds         DOUBLE PRECISION NOT NULL,
    transcript_word_count    INTEGER NOT NULL,
    transcription_provider   TEXT,
    transcription_model      TEXT,
    transcription_latency_ms BIGINT,
    device_id                TEXT,
    platform                 TEXT,
    app_version              TEXT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_local_meeting_session_account_client
        UNIQUE (account_id, client_session_id),
    CONSTRAINT uq_local_meeting_session_scope
        UNIQUE (id, org_id, account_id),
    CONSTRAINT ck_local_meeting_session_client_id
        CHECK (length(btrim(client_session_id)) > 0),
    CONSTRAINT ck_local_meeting_session_status
        CHECK (status IN ('completed', 'failed', 'cancelled')),
    CONSTRAINT ck_local_meeting_session_time
        CHECK (ended_at >= started_at),
    CONSTRAINT ck_local_meeting_session_duration
        CHECK (duration_seconds >= 0),
    CONSTRAINT ck_local_meeting_session_words
        CHECK (transcript_word_count >= 0),
    CONSTRAINT ck_local_meeting_session_transcription_latency
        CHECK (transcription_latency_ms IS NULL OR transcription_latency_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_local_meeting_sessions_org_started
    ON local_meeting_sessions (org_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_local_meeting_sessions_account_started
    ON local_meeting_sessions (account_id, started_at DESC);

CREATE TABLE IF NOT EXISTS local_meeting_provider_usage (
    id                         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    local_meeting_session_id   UUID NOT NULL,
    org_id                     UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id                 UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    idempotency_key            TEXT NOT NULL,
    credential_scope           TEXT NOT NULL,
    provider                   TEXT NOT NULL,
    model                      TEXT NOT NULL,
    feature_stage              TEXT NOT NULL,
    prompt_tokens              INTEGER NOT NULL,
    cache_hit_tokens           INTEGER NOT NULL,
    cache_miss_tokens          INTEGER NOT NULL,
    completion_tokens          INTEGER NOT NULL,
    reasoning_tokens           INTEGER,
    latency_ms                 BIGINT NOT NULL,
    result_status              TEXT NOT NULL,
    occurred_at                TIMESTAMPTZ NOT NULL,
    estimated_cost_usd         DOUBLE PRECISION NOT NULL,
    cost_source                TEXT NOT NULL,
    rate_card_version          TEXT NOT NULL,
    rate_card_snapshot         JSONB NOT NULL,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_local_meeting_usage_account_key
        UNIQUE (account_id, idempotency_key),
    CONSTRAINT fk_local_meeting_usage_session_scope
        FOREIGN KEY (local_meeting_session_id, org_id, account_id)
        REFERENCES local_meeting_sessions(id, org_id, account_id) ON DELETE CASCADE,
    CONSTRAINT ck_local_meeting_usage_idempotency_key
        CHECK (length(btrim(idempotency_key)) > 0),
    CONSTRAINT ck_local_meeting_usage_credential_scope
        CHECK (credential_scope = 'airnote_bundled'),
    CONSTRAINT ck_local_meeting_usage_provider
        CHECK (provider = 'deepseek'),
    CONSTRAINT ck_local_meeting_usage_model
        CHECK (model = 'deepseek-v4-pro'),
    CONSTRAINT ck_local_meeting_usage_tokens
        CHECK (
            prompt_tokens >= 0
            AND cache_hit_tokens >= 0
            AND cache_miss_tokens >= 0
            AND completion_tokens >= 0
            AND (reasoning_tokens IS NULL OR reasoning_tokens >= 0)
            AND prompt_tokens = cache_hit_tokens + cache_miss_tokens
            AND (reasoning_tokens IS NULL OR reasoning_tokens <= completion_tokens)
        ),
    CONSTRAINT ck_local_meeting_usage_latency
        CHECK (latency_ms >= 0),
    CONSTRAINT ck_local_meeting_usage_result_status
        CHECK (result_status IN ('success', 'error', 'cancelled', 'timeout')),
    CONSTRAINT ck_local_meeting_usage_cost
        CHECK (estimated_cost_usd >= 0)
);

CREATE INDEX IF NOT EXISTS idx_local_meeting_provider_usage_session
    ON local_meeting_provider_usage (local_meeting_session_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_local_meeting_provider_usage_org_occurred
    ON local_meeting_provider_usage (org_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_local_meeting_provider_usage_account_occurred
    ON local_meeting_provider_usage (account_id, occurred_at DESC);
