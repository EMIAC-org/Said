-- Polish always routes through the control-plane server runtime.
UPDATE preferences
   SET server_runtime_enabled = 1,
       updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
 WHERE server_runtime_enabled = 0;
