-- Append-only alias learn audit log for org-admin observability.
-- Does not replace frozen personal_stt_replacements writers.

CREATE TABLE IF NOT EXISTS runtime_alias_learn_events (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id   UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id       UUID REFERENCES orgs(id) ON DELETE SET NULL,
    recording_id TEXT,
    heard        TEXT NOT NULL,
    correct      TEXT NOT NULL,
    source       TEXT NOT NULL DEFAULT 'classify',
    safety       TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_alias_learn_org_created
    ON runtime_alias_learn_events (org_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_alias_learn_account_created
    ON runtime_alias_learn_events (account_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_alias_learn_recording
    ON runtime_alias_learn_events (recording_id)
    WHERE recording_id IS NOT NULL;
