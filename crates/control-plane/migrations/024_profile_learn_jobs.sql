-- Profile learn-from-edit job queue + extended audit log actions.

CREATE TABLE IF NOT EXISTS runtime_profile_learn_jobs (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id     UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_scope      UUID NOT NULL,
    edit_event_id  TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'queued',
    request_json   JSONB NOT NULL DEFAULT '{}'::jsonb,
    response_json  JSONB,
    from_version   BIGINT NOT NULL DEFAULT 0,
    to_version     BIGINT,
    error          TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at   TIMESTAMPTZ,
    CONSTRAINT chk_profile_learn_job_status CHECK (
        status IN ('queued', 'processing', 'shadow', 'applied', 'rejected', 'failed')
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_profile_learn_jobs_idempotency
    ON runtime_profile_learn_jobs (account_id, org_scope, edit_event_id);

CREATE INDEX IF NOT EXISTS idx_profile_learn_jobs_status_created
    ON runtime_profile_learn_jobs (status, created_at)
    WHERE status = 'queued';

-- Widen audit log action/source constraints for profile learning.
ALTER TABLE runtime_profile_audit_log
    DROP CONSTRAINT IF EXISTS runtime_profile_audit_log_action_check;

ALTER TABLE runtime_profile_audit_log
    ADD CONSTRAINT runtime_profile_audit_log_action_check
    CHECK (action IN (
        'patch', 'rebuild', 'reset',
        'learn_queued', 'learn_shadow', 'learn_applied', 'learn_rejected', 'learn_failed',
        'learn_proposed', 'learn_approved', 'learn_dismissed'
    ));

ALTER TABLE runtime_profile_audit_log
    DROP CONSTRAINT IF EXISTS runtime_profile_audit_log_source_check;

-- NOTE: keep this source list a SUPERSET that also covers every value any
-- LATER migration allows. All migrations re-run (idempotently) on every boot in
-- order, and this ADD runs BEFORE migration 037's widening — so if this list is
-- narrower than what the running app already wrote (e.g. 'batch'), this ADD
-- fails with "check constraint ... is violated by some row" and the process
-- crash-loops on startup. 'batch' added here to match migration 037.
ALTER TABLE runtime_profile_audit_log
    ADD CONSTRAINT runtime_profile_audit_log_source_check
    CHECK (source IN (
        'api', 'rebuild_worker', 'migration', 'admin',
        'deepseek_edit', 'validator', 'batch'
    ));
