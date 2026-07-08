-- Migration 019: background-only learning trust + edit-event quality metadata
--
-- Goal:
--   • Keep the hot path deterministic.
--   • Persist a compact alias trust lifecycle for exported speech hints.
--   • Add lightweight edit-event metadata so vector learning can dedupe and
--     skip low-information corrections without changing runtime retrieval.

ALTER TABLE stt_replacements
    ADD COLUMN export_tier TEXT NOT NULL DEFAULT 'local_only';
ALTER TABLE stt_replacements
    ADD COLUMN contradiction_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stt_replacements
    ADD COLUMN last_contradicted_at INTEGER;
ALTER TABLE stt_replacements
    ADD COLUMN review_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE stt_replacements
    ADD COLUMN review_reason TEXT;
ALTER TABLE stt_replacements
    ADD COLUMN last_reviewed_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_stt_repl_export_tier
    ON stt_replacements (user_id, export_tier, contradiction_count);

ALTER TABLE edit_events
    ADD COLUMN learning_kind TEXT;
ALTER TABLE edit_events
    ADD COLUMN text_fingerprint TEXT;
ALTER TABLE edit_events
    ADD COLUMN vector_quality TEXT NOT NULL DEFAULT 'normal';

CREATE INDEX IF NOT EXISTS idx_edit_user_fingerprint
    ON edit_events (user_id, text_fingerprint, timestamp_ms DESC);
