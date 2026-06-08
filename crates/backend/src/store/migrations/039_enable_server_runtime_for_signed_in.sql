-- Signed-in desktop users should route polish through the control-plane runtime
-- by default. Migration 035 added server_runtime_enabled with DEFAULT 0, so
-- existing enterprise users were still polishing locally and never appeared in
-- the admin Runtime ledger.

UPDATE preferences
   SET server_runtime_enabled = 1,
       updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
 WHERE server_runtime_enabled = 0
   AND user_id IN (
         SELECT id
           FROM local_user
          WHERE cloud_token IS NOT NULL
            AND TRIM(cloud_token) != ''
       );
