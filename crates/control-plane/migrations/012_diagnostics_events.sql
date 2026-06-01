CREATE TABLE IF NOT EXISTS diagnostics_events (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id    TEXT NOT NULL,
    account_id   UUID,
    org_id       UUID,
    event_type   TEXT NOT NULL,
    severity     TEXT NOT NULL,
    app_version  TEXT,
    os           TEXT,
    arch         TEXT,
    channel      TEXT,
    phase        TEXT,
    context      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_diagnostics_severity_created
    ON diagnostics_events (severity, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_diagnostics_device_created
    ON diagnostics_events (device_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_diagnostics_event_type_created
    ON diagnostics_events (event_type, created_at DESC);
