-- Allow user-initiated app->bucket overrides (from the Buckets kanban) alongside
-- the existing static / agent / admin sources. A 'user' mapping wins over static +
-- agent in resolve_bucket, so re-filing an app in the UI always sticks.
--
-- Idempotent: this runner re-executes every migration on startup, so we drop the
-- old constraint (if present) and re-add it with the widened value set.
ALTER TABLE app_bucket_map DROP CONSTRAINT IF EXISTS chk_app_bucket_map_source;

ALTER TABLE app_bucket_map ADD CONSTRAINT chk_app_bucket_map_source
    CHECK (source IN ('static', 'agent', 'admin', 'user'));
