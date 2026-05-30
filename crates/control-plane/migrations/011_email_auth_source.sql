-- Track whether an org member joined through Lark OAuth or email/password.
-- Nullable for backward compatibility. Read paths compute a safe fallback from
-- lark_user_id when older rows do not have an explicit source.
ALTER TABLE org_members
    ADD COLUMN IF NOT EXISTS auth_source TEXT;

CREATE INDEX IF NOT EXISTS idx_org_members_auth_source
    ON org_members (org_id, auth_source);
