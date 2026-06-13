-- Memory hygiene worker state + audit log.

CREATE TABLE IF NOT EXISTS personal_memory_hygiene_state (
    account_id       UUID PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    memory_dirty_at  TIMESTAMPTZ,
    last_hygiene_at  TIMESTAMPTZ,
    hygiene_version  INTEGER NOT NULL DEFAULT 1,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_memory_hygiene_dirty
    ON personal_memory_hygiene_state (memory_dirty_at)
    WHERE memory_dirty_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS alias_safety_audit (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id   UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id       UUID REFERENCES orgs(id) ON DELETE SET NULL,
    action       TEXT NOT NULL,
    target_type  TEXT NOT NULL DEFAULT 'alias',
    heard        TEXT,
    correct      TEXT,
    verdict      TEXT NOT NULL,
    reason       TEXT NOT NULL DEFAULT '',
    model        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_alias_safety_audit_account
    ON alias_safety_audit (account_id, created_at DESC);

ALTER TABLE personal_stt_replacements
    ADD COLUMN IF NOT EXISTS learned_stt_provider TEXT;
