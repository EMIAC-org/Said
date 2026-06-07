-- Wave 1 (memory) + Wave 5/6 support: per-user server-side personal memory for
-- the unified runtime. Strictly per-account; never cross-user. Holds learned
-- corrections (the user's own protected terms/aliases) — allowed by the
-- retention contract ("learning memory: yes, but scoped and protected"). No raw
-- transcripts stored. Idempotent DDL (IF NOT EXISTS) — safe to re-run.

CREATE TABLE IF NOT EXISTS personal_vocab_terms (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    term        TEXT NOT NULL,
    term_type   TEXT,
    priority    REAL NOT NULL DEFAULT 0.5,
    source      TEXT NOT NULL DEFAULT 'user',
    archived_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, term)
);
CREATE INDEX IF NOT EXISTS idx_personal_vocab_terms_account
    ON personal_vocab_terms (account_id) WHERE archived_at IS NULL;

CREATE TABLE IF NOT EXISTS personal_stt_replacements (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    spoken      TEXT NOT NULL,
    canonical   TEXT NOT NULL,
    source      TEXT NOT NULL DEFAULT 'edit',
    hit_count   INTEGER NOT NULL DEFAULT 1,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, spoken)
);
CREATE INDEX IF NOT EXISTS idx_personal_stt_replacements_account
    ON personal_stt_replacements (account_id);

CREATE TABLE IF NOT EXISTS personal_blocked_aliases (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    spoken      TEXT NOT NULL,
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, spoken)
);
CREATE INDEX IF NOT EXISTS idx_personal_blocked_aliases_account
    ON personal_blocked_aliases (account_id);

CREATE TABLE IF NOT EXISTS personal_edit_policy_rules (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    rule        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_personal_edit_policy_rules_account
    ON personal_edit_policy_rules (account_id);

CREATE TABLE IF NOT EXISTS personal_policy_weights (
    account_id  UUID PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    weights     JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS learning_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    run_id      UUID,
    kind        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_learning_events_account
    ON learning_events (account_id, created_at DESC);

-- Explicit-only raw debug capture, TTL-bound (off by default).
CREATE TABLE IF NOT EXISTS runtime_debug_payloads (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id   UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    run_id       UUID,
    kind         TEXT NOT NULL,
    ciphertext   TEXT,
    expires_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_runtime_debug_payloads_expiry
    ON runtime_debug_payloads (expires_at);
