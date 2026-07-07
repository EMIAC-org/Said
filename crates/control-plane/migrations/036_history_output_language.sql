-- Per-dictation output language (monitoring).
--
-- Denormalized column so the admin per-user dictation list/detail can scan, per
-- dictation, which OUTPUT LANGUAGE was actually used in which app (target_app is
-- already stored). Written server-authoritatively from the runtime polish path
-- (write_history_from_runtime) so it reflects the EFFECTIVE language after any
-- per-bucket override, and also accepted from the client /history/sync for
-- local/offline dictations. NULL for older rows / unknown.
ALTER TABLE runtime_history_items
    ADD COLUMN IF NOT EXISTS output_language TEXT;
