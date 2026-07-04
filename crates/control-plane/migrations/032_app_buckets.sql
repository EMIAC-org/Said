-- App-context buckets: the foundation for per-bucket ("conditional block") profiles.
--
-- Two additive tables. Nothing here touches `runtime_user_profiles` — that stays
-- the account-GLOBAL profile (the person: background, domains, vocab). Per-bucket
-- STYLE overlays (the conditional blocks) live in `runtime_user_bucket_profiles`,
-- keyed additionally by `bucket_key`. This keeps the existing learn->store->inject
-- pipeline working unchanged while we accumulate context per app-bucket.
--
-- Bucket keys are a fixed, small enum owned in Rust (profile::bucket::Bucket):
--   coding | messaging | work_tracker | formal_writing | default
-- The DB CHECKs mirror that enum so a bad key can never land.

-- 1) app_key -> bucket_key mapping. GLOBAL (not per-account): "VS Code is Coding"
--    holds for everyone. Known apps resolve from a compiled-in static table in Rust;
--    this table persists ONLY the AI-agent classifications of previously-unknown apps
--    (and any admin overrides), so the agent classifies each new app at most once.
CREATE TABLE IF NOT EXISTS app_bucket_map (
    app_key      TEXT PRIMARY KEY,
    bucket_key   TEXT NOT NULL,
    source       TEXT NOT NULL DEFAULT 'agent',
    confidence   DOUBLE PRECISION NOT NULL DEFAULT 1,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_app_bucket_map_bucket CHECK (
        bucket_key IN ('coding', 'messaging', 'work_tracker', 'formal_writing', 'default')
    ),
    CONSTRAINT chk_app_bucket_map_source CHECK (
        source IN ('static', 'agent', 'admin')
    )
);

-- 2) Per-account/org/bucket STYLE overlay profiles (the "conditional blocks").
--    Mirrors runtime_user_profiles but with bucket_key in the primary key. A brand-new
--    bucket simply has no row -> callers fall back to the global profile, so there is
--    no cold-start cliff.
CREATE TABLE IF NOT EXISTS runtime_user_bucket_profiles (
    account_id         UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_scope          UUID NOT NULL,
    bucket_key         TEXT NOT NULL,
    profile_json       JSONB NOT NULL DEFAULT '{}'::jsonb,
    profile_markdown   TEXT NOT NULL DEFAULT '',
    version            BIGINT NOT NULL DEFAULT 1,
    schema_version     INTEGER NOT NULL DEFAULT 1,
    status             TEXT NOT NULL DEFAULT 'ready',
    dirty_at           TIMESTAMPTZ,
    last_rebuilt_at    TIMESTAMPTZ,
    last_error         TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, org_scope, bucket_key),
    CONSTRAINT chk_bucket_profile_bucket CHECK (
        bucket_key IN ('coding', 'messaging', 'work_tracker', 'formal_writing', 'default')
    ),
    CONSTRAINT chk_bucket_profile_status CHECK (
        status IN ('ready', 'dirty', 'rebuilding', 'error')
    )
);

CREATE INDEX IF NOT EXISTS idx_runtime_bucket_profiles_account_updated
    ON runtime_user_bucket_profiles (account_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_runtime_bucket_profiles_dirty
    ON runtime_user_bucket_profiles (status, dirty_at)
    WHERE status = 'dirty';
