-- Bug reports ---------------------------------------------------------------
-- Local-first desktop users open a signed public report URL from the app.
-- Screenshots are stored inline for the first pass. Cloudinary can replace
-- screenshot_data_url with screenshot_url later without changing admin UI.
CREATE TABLE IF NOT EXISTS bug_reports (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID REFERENCES orgs(id) ON DELETE SET NULL,
    account_id          UUID REFERENCES accounts(id) ON DELETE SET NULL,
    reporter_email      TEXT,
    reporter_name       TEXT,
    title               TEXT NOT NULL,
    description         TEXT NOT NULL,
    severity            TEXT NOT NULL DEFAULT 'normal',
    status              TEXT NOT NULL DEFAULT 'open',
    app_version         TEXT,
    platform            TEXT,
    device_id           TEXT,
    screenshot_url      TEXT,
    screenshot_data_url TEXT,
    screenshot_name     TEXT,
    screenshot_mime     TEXT,
    user_agent          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_bug_reports_org_created
    ON bug_reports (org_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_bug_reports_account_created
    ON bug_reports (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_bug_reports_status
    ON bug_reports (status, created_at DESC);
