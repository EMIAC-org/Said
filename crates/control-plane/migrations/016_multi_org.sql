-- Multi-org SaaS: active workspace context, org billing, org policy overlay.

ALTER TABLE accounts ADD COLUMN IF NOT EXISTS active_org_id UUID REFERENCES orgs(id) ON DELETE SET NULL;

ALTER TABLE sessions ADD COLUMN IF NOT EXISTS active_org_id UUID REFERENCES orgs(id) ON DELETE SET NULL;

CREATE TABLE IF NOT EXISTS org_subscriptions (
    org_id              UUID PRIMARY KEY REFERENCES orgs(id) ON DELETE CASCADE,
    tier                TEXT NOT NULL DEFAULT 'team',
    seat_limit          INTEGER,
    expires_at          TIMESTAMPTZ,
    stripe_customer_id  TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS org_usage_daily (
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    event_date      DATE NOT NULL DEFAULT CURRENT_DATE,
    polish_count    INTEGER NOT NULL DEFAULT 0,
    word_count      INTEGER NOT NULL DEFAULT 0,
    model_used      TEXT NOT NULL DEFAULT 'fast',
    PRIMARY KEY (org_id, account_id, event_date, model_used)
);
CREATE INDEX IF NOT EXISTS idx_org_usage_daily_org_date
    ON org_usage_daily (org_id, event_date DESC);

CREATE TABLE IF NOT EXISTS org_runtime_settings (
    org_id                      UUID PRIMARY KEY REFERENCES orgs(id) ON DELETE CASCADE,
    stt_provider                TEXT,
    enforce_stt_provider        BOOLEAN NOT NULL DEFAULT FALSE,
    server_runtime_enabled      BOOLEAN,
    enforce_server_runtime      BOOLEAN NOT NULL DEFAULT FALSE,
    allowed_models_json         JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Backfill active_org_id for accounts with exactly one org membership.
UPDATE accounts a
   SET active_org_id = om.org_id
  FROM org_members om
  JOIN (
        SELECT account_id
          FROM org_members
         GROUP BY account_id
        HAVING COUNT(*) = 1
       ) single_org_accounts
    ON single_org_accounts.account_id = om.account_id
 WHERE a.id = om.account_id
   AND a.active_org_id IS NULL;
