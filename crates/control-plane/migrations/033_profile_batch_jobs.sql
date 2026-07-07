-- Batched, per-user profiling + KB runs (DeepSeek-V4-Flash), one run per ~10 dictations.
--
-- Replaces the per-edit learn cadence with a COALESCED per-user window job:
--   * at most ONE active (queued|processing) job per (account, org_scope) -> load
--     scales with active users, not dictation volume;
--   * claimed concurrently by N workers via FOR UPDATE SKIP LOCKED (no global lock);
--   * records EVERY triggered run including skips (skip_reason) so we can answer
--     "how many times did DeepSeek run for this user, and why".

CREATE TABLE IF NOT EXISTS runtime_profile_batch_jobs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id    UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_scope     UUID NOT NULL,
    status        TEXT NOT NULL DEFAULT 'queued',
    skip_reason   TEXT,                 -- set when status='skipped' (e.g. 'no_signal')
    run_count     INTEGER NOT NULL DEFAULT 0,   -- dictations in the analyzed window
    window_from   TIMESTAMPTZ,
    window_to     TIMESTAMPTZ,
    attempts      INTEGER NOT NULL DEFAULT 0,
    latency_ms    BIGINT,
    token_usage   BIGINT,
    error         TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at  TIMESTAMPTZ,
    CONSTRAINT chk_batch_job_status CHECK (
        status IN ('queued', 'processing', 'applied', 'shadow', 'rejected', 'skipped', 'failed')
    )
);

-- Coalescing guard: a user can have at most one in-flight job. The enqueue path also
-- checks NOT EXISTS, but this partial unique index makes it race-proof.
CREATE UNIQUE INDEX IF NOT EXISTS idx_batch_jobs_active_per_user
    ON runtime_profile_batch_jobs (account_id, org_scope)
    WHERE status IN ('queued', 'processing');

-- Claim ordering (queued) + stuck-job reaper scan (processing).
CREATE INDEX IF NOT EXISTS idx_batch_jobs_queued
    ON runtime_profile_batch_jobs (created_at)
    WHERE status = 'queued';

CREATE INDEX IF NOT EXISTS idx_batch_jobs_processing
    ON runtime_profile_batch_jobs (updated_at)
    WHERE status = 'processing';

-- Per-user run rollup for fast reads ("ran 12, skipped 8, last run 20m ago").
-- Additive columns; existing store.rs SELECTs are explicit and ignore them.
ALTER TABLE runtime_user_profiles
    ADD COLUMN IF NOT EXISTS profile_run_count  INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS skipped_run_count  INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS last_run_at        TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_run_outcome   TEXT;
