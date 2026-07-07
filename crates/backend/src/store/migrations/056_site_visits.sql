-- ── Browser site context (on-device only) ────────────────────────────────────
-- Records the site (host/domain) a dictation was pasted into when the target is
-- a browser and the opt-in `browser_context_enabled` pref is set. Domain only —
-- never the full URL (scheme/path/query are stripped at capture). Local table,
-- never synced to the cloud runtime: browsing context stays on this device.
CREATE TABLE IF NOT EXISTS site_visits (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      TEXT    NOT NULL,
    target_app   TEXT    NOT NULL,   -- browser bundle-id (e.g. com.google.Chrome)
    host         TEXT    NOT NULL,   -- domain only (e.g. mail.google.com)
    timestamp_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_site_visits_user_host
    ON site_visits (user_id, host);
