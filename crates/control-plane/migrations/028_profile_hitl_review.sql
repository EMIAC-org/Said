-- Profile learning HITL review queue.
--
-- DeepSeek/validator jobs now produce review proposals first. The profile row
-- is mutated only after the user approves the proposal.

ALTER TABLE runtime_profile_learn_jobs
    DROP CONSTRAINT IF EXISTS chk_profile_learn_job_status;

ALTER TABLE runtime_profile_learn_jobs
    ADD CONSTRAINT chk_profile_learn_job_status CHECK (
        status IN (
            'queued', 'processing',
            'pending_review', 'approved', 'dismissed',
            'shadow', 'applied', 'rejected', 'failed'
        )
    );

CREATE INDEX IF NOT EXISTS idx_profile_learn_jobs_pending_review
    ON runtime_profile_learn_jobs (account_id, org_scope, updated_at DESC)
    WHERE status = 'pending_review';

ALTER TABLE runtime_profile_audit_log
    DROP CONSTRAINT IF EXISTS runtime_profile_audit_log_action_check;

ALTER TABLE runtime_profile_audit_log
    ADD CONSTRAINT runtime_profile_audit_log_action_check
    CHECK (action IN (
        'patch', 'rebuild', 'reset',
        'learn_queued', 'learn_shadow', 'learn_applied', 'learn_rejected', 'learn_failed',
        'learn_proposed', 'learn_approved', 'learn_dismissed'
    ));
