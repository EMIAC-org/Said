-- Wave 1 (memory slice) + Wave 5/6 support: server-side personal memory.
--
-- Per-user, never cross-user. These hold learned corrections (the user's own
-- protected terms / aliases) — allowed by the retention contract ("learning
-- memory: yes, but scoped and protected"). No raw transcripts are stored.
-- Idempotent DDL so the embedded migration is safe to re-run.

-- Learned spoken→canonical replacements, e.g. "mac ops" -> "Macobs".
CREATE TABLE IF NOT EXISTS personal_stt_replacements (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    spoken      TEXT NOT NULL,                 -- lowercased misheard form
    canonical   TEXT NOT NULL,                 -- corrected form the user kept
    source      TEXT NOT NULL DEFAULT 'edit',  -- edit | explicit
    hit_count   INTEGER NOT NULL DEFAULT 1,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, spoken)
);
CREATE INDEX IF NOT EXISTS idx_personal_stt_replacements_account
    ON personal_stt_replacements (account_id);

-- User-specific unsafe mappings that must never be auto-applied/learned.
CREATE TABLE IF NOT EXISTS personal_blocked_aliases (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    spoken      TEXT NOT NULL,                 -- lowercased form that is blocked
    reason      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, spoken)
);
CREATE INDEX IF NOT EXISTS idx_personal_blocked_aliases_account
    ON personal_blocked_aliases (account_id);

-- Privacy-safe learning ledger (no raw text — only the decision + run link).
CREATE TABLE IF NOT EXISTS learning_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    run_id      UUID,
    kind        TEXT NOT NULL,   -- learned_replacement | blocked_unsafe | ignored_formatting | explicit_term
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_learning_events_account
    ON learning_events (account_id, created_at DESC);
