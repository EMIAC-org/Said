-- Latest per-account prompt profile snapshot (what was injected into voice polish).

CREATE TABLE IF NOT EXISTS runtime_prompt_profile_latest (
    account_id               UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_scope                UUID NOT NULL,
    profile_source           TEXT NOT NULL DEFAULT 'none',
    profile_markdown         TEXT NOT NULL DEFAULT '',
    profile_chars            INTEGER NOT NULL DEFAULT 0,
    profile_hash             TEXT NOT NULL DEFAULT '',
    client_profile_version   BIGINT,
    last_run_id              UUID REFERENCES runtime_sessions(id) ON DELETE SET NULL,
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, org_scope),
    CONSTRAINT chk_runtime_prompt_profile_source CHECK (
        profile_source IN ('client_local', 'server_db', 'none')
    )
);

CREATE INDEX IF NOT EXISTS idx_runtime_prompt_profile_latest_updated
    ON runtime_prompt_profile_latest (updated_at DESC);
