-- Unified AirNote server runtime gateway.
-- Stores provider credential metadata, runtime traces, and per-user learning
-- memory. Secrets are encrypted by the application before insertion.

CREATE TABLE IF NOT EXISTS runtime_provider_credentials (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID REFERENCES orgs(id) ON DELETE CASCADE,
    account_id         UUID REFERENCES accounts(id) ON DELETE CASCADE,
    scope              TEXT NOT NULL DEFAULT 'user',
    provider           TEXT NOT NULL,
    display_name       TEXT NOT NULL DEFAULT '',
    secret_ciphertext  TEXT NOT NULL,
    secret_nonce       TEXT NOT NULL,
    secret_key_version TEXT NOT NULL DEFAULT 'v1',
    secret_last4       TEXT NOT NULL DEFAULT '',
    status             TEXT NOT NULL DEFAULT 'active',
    validated_at       TIMESTAMPTZ,
    last_used_at       TIMESTAMPTZ,
    last_error         TEXT,
    created_by         UUID REFERENCES accounts(id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope IN ('user', 'org', 'airnote_managed')),
    CHECK (status IN ('active', 'invalid', 'revoked', 'rate_limited')),
    CHECK (provider IN ('groq', 'openai', 'gemini', 'gateway'))
);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_credentials_account
    ON runtime_provider_credentials (account_id, provider, status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_credentials_org
    ON runtime_provider_credentials (org_id, provider, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS runtime_sessions (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id         UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id             UUID REFERENCES orgs(id) ON DELETE SET NULL,
    device_id          TEXT,
    client_run_id      TEXT,
    mode               TEXT NOT NULL DEFAULT 'normal_voice',
    source             TEXT NOT NULL DEFAULT 'desktop_voice',
    platform           TEXT,
    app_version        TEXT,
    status             TEXT NOT NULL DEFAULT 'created',
    error_kind         TEXT,
    input_hash         TEXT,
    output_hash        TEXT,
    provider_summary   JSONB NOT NULL DEFAULT '{}'::jsonb,
    latency_json       JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata_json      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_sessions_account_created
    ON runtime_sessions (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_sessions_org_created
    ON runtime_sessions (org_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_sessions_client_run
    ON runtime_sessions (client_run_id);

CREATE TABLE IF NOT EXISTS runtime_stage_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id      UUID NOT NULL REFERENCES runtime_sessions(id) ON DELETE CASCADE,
    stage       TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'ok',
    latency_ms  BIGINT,
    error_kind  TEXT,
    metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_stage_events_run
    ON runtime_stage_events (run_id, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_runtime_stage_events_stage_created
    ON runtime_stage_events (stage, created_at DESC);

CREATE TABLE IF NOT EXISTS runtime_provider_usage (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    credential_id     UUID REFERENCES runtime_provider_credentials(id) ON DELETE SET NULL,
    run_id            UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    attempt_index     INTEGER NOT NULL DEFAULT 0,
    credential_scope  TEXT NOT NULL DEFAULT 'unknown',
    provider          TEXT NOT NULL,
    model             TEXT,
    input_tokens      INTEGER,
    output_tokens     INTEGER,
    estimated_cost_usd DOUBLE PRECISION,
    request_ms        BIGINT,
    ttft_ms           BIGINT,
    stream_ms         BIGINT,
    total_ms          BIGINT,
    timeout_ms        BIGINT,
    status            TEXT NOT NULL DEFAULT 'ok',
    error_kind        TEXT,
    fallback_reason   TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_usage_credential
    ON runtime_provider_usage (credential_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_usage_run
    ON runtime_provider_usage (run_id, attempt_index ASC);

CREATE TABLE IF NOT EXISTS runtime_learning_events (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id       UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id           UUID REFERENCES orgs(id) ON DELETE SET NULL,
    device_id        TEXT,
    run_id           UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    recording_id     TEXT,
    event_type       TEXT NOT NULL,
    classification   TEXT,
    input_hash       TEXT,
    output_hash      TEXT,
    corrected_hash   TEXT,
    payload_json     JSONB NOT NULL DEFAULT '{}'::jsonb,
    server_judgment  JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_learning_events_account
    ON runtime_learning_events (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_learning_events_run
    ON runtime_learning_events (run_id, created_at DESC);

CREATE TABLE IF NOT EXISTS personal_vocab_terms (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id     UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id         UUID REFERENCES orgs(id) ON DELETE SET NULL,
    term           TEXT NOT NULL,
    term_norm      TEXT NOT NULL,
    term_type      TEXT NOT NULL DEFAULT 'other',
    source         TEXT NOT NULL DEFAULT 'server_runtime',
    weight         DOUBLE PRECISION NOT NULL DEFAULT 1,
    positive_count INTEGER NOT NULL DEFAULT 0,
    negative_count INTEGER NOT NULL DEFAULT 0,
    status         TEXT NOT NULL DEFAULT 'active',
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, term_norm)
);
CREATE INDEX IF NOT EXISTS idx_personal_vocab_terms_account
    ON personal_vocab_terms (account_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS personal_stt_replacements (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id          UUID REFERENCES orgs(id) ON DELETE SET NULL,
    transcript_form TEXT NOT NULL,
    transcript_norm TEXT NOT NULL,
    correct_form    TEXT NOT NULL,
    correct_norm    TEXT NOT NULL,
    positive_count  INTEGER NOT NULL DEFAULT 0,
    negative_count  INTEGER NOT NULL DEFAULT 0,
    weight          DOUBLE PRECISION NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'active',
    safety_status   TEXT NOT NULL DEFAULT 'unknown',
    first_seen_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, transcript_norm, correct_norm)
);
CREATE INDEX IF NOT EXISTS idx_personal_stt_replacements_account
    ON personal_stt_replacements (account_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS personal_edit_policy_rules (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id         UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id             UUID REFERENCES orgs(id) ON DELETE SET NULL,
    variant_form       TEXT NOT NULL,
    variant_norm       TEXT NOT NULL,
    correct_form       TEXT NOT NULL,
    correct_norm       TEXT NOT NULL,
    edit_type          TEXT NOT NULL DEFAULT 'replace',
    positive_count     INTEGER NOT NULL DEFAULT 0,
    negative_count     INTEGER NOT NULL DEFAULT 0,
    status             TEXT NOT NULL DEFAULT 'candidate',
    left_context_json  JSONB NOT NULL DEFAULT '[]'::jsonb,
    right_context_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    source_run_id      UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    first_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, variant_norm, correct_norm, edit_type)
);
CREATE INDEX IF NOT EXISTS idx_personal_edit_policy_rules_account
    ON personal_edit_policy_rules (account_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS personal_policy_weights (
    account_id         UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    token_norm         TEXT NOT NULL,
    correct_form       TEXT NOT NULL,
    correct_form_norm  TEXT NOT NULL,
    positive_count     INTEGER NOT NULL DEFAULT 0,
    negative_count     INTEGER NOT NULL DEFAULT 0,
    learned_weight     DOUBLE PRECISION NOT NULL DEFAULT 0,
    last_reward_source TEXT,
    first_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, token_norm, correct_form_norm)
);
CREATE INDEX IF NOT EXISTS idx_personal_policy_weights_account
    ON personal_policy_weights (account_id, last_seen_at DESC);

CREATE TABLE IF NOT EXISTS personal_blocked_aliases (
    account_id        UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    source_norm       TEXT NOT NULL,
    target_norm       TEXT NOT NULL,
    reason            TEXT NOT NULL DEFAULT 'user_blocked',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, source_norm, target_norm)
);

CREATE TABLE IF NOT EXISTS runtime_debug_payloads (
    run_id      UUID PRIMARY KEY REFERENCES runtime_sessions(id) ON DELETE CASCADE,
    raw_input   TEXT,
    raw_output  TEXT,
    raw_audio_ref TEXT,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_debug_payloads_expires
    ON runtime_debug_payloads (expires_at);
