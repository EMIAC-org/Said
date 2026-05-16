-- Wave 4: Single OpenAI account per org for Codex AI pipeline -----------------

ALTER TABLE orgs ADD COLUMN IF NOT EXISTS openai_access_token TEXT;
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS openai_refresh_token TEXT;
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS openai_token_expires_at TIMESTAMPTZ;
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS openai_plan_type TEXT;         -- 'plus' | 'pro'
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS openai_connected_at TIMESTAMPTZ;
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS openai_label TEXT;             -- optional display name
