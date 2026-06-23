-- Server-owned per-user runtime profiles (JSON + prompt markdown).
-- org_scope uses 00000000-0000-0000-0000-000000000000 for account-global profiles
-- when no active org is in scope (avoids NULL in composite PK).

CREATE TABLE IF NOT EXISTS runtime_user_profiles (
    account_id         UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_scope          UUID NOT NULL,
    profile_json       JSONB NOT NULL DEFAULT '{}'::jsonb,
    profile_markdown   TEXT NOT NULL DEFAULT '',
    version            BIGINT NOT NULL DEFAULT 1,
    schema_version     INTEGER NOT NULL DEFAULT 1,
    status             TEXT NOT NULL DEFAULT 'ready',
    source_hash        TEXT NOT NULL DEFAULT '',
    dirty_at           TIMESTAMPTZ,
    last_rebuilt_at    TIMESTAMPTZ,
    last_error         TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, org_scope),
    CONSTRAINT chk_runtime_profile_status CHECK (
        status IN ('ready', 'dirty', 'rebuilding', 'error')
    )
);

CREATE INDEX IF NOT EXISTS idx_runtime_user_profiles_account_updated
    ON runtime_user_profiles (account_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_runtime_user_profiles_dirty
    ON runtime_user_profiles (status, dirty_at)
    WHERE status = 'dirty';

CREATE TABLE IF NOT EXISTS runtime_profile_audit_log (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id     UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_scope      UUID NOT NULL,
    from_version   BIGINT NOT NULL,
    to_version     BIGINT NOT NULL,
    action         TEXT NOT NULL,
    patch_json     JSONB NOT NULL DEFAULT '{}'::jsonb,
    source         TEXT NOT NULL DEFAULT 'api',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (action IN ('patch', 'rebuild', 'reset')),
    CHECK (source IN ('api', 'rebuild_worker', 'migration', 'admin'))
);

CREATE INDEX IF NOT EXISTS idx_runtime_profile_audit_account
    ON runtime_profile_audit_log (account_id, created_at DESC);
