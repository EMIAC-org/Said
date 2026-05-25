-- Desktop client registry (install acknowledgment + heartbeat)
CREATE TABLE IF NOT EXISTS desktop_clients (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL,
    platform        TEXT NOT NULL,
    app_version     TEXT NOT NULL,
    hostname        TEXT,
    first_seen_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, device_id)
);
CREATE INDEX IF NOT EXISTS idx_desktop_clients_org ON desktop_clients (org_id, last_seen_at DESC);
CREATE INDEX IF NOT EXISTS idx_desktop_clients_account ON desktop_clients (account_id);
