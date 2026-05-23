-- Migration 027: Tier 2 local correction model metadata.
--
-- Stores only local artifact bookkeeping for the optional ONNX scorer. Raw
-- corrections and vocabulary examples remain in their existing local tables.

CREATE TABLE IF NOT EXISTS tier2_model_metadata (
    user_id           TEXT PRIMARY KEY REFERENCES local_user(id) ON DELETE CASCADE,
    artifact_path     TEXT NOT NULL,
    vocab_index_path  TEXT NOT NULL,
    trained_at        INTEGER NOT NULL,
    alias_count       INTEGER NOT NULL DEFAULT 0,
    vocab_count       INTEGER NOT NULL DEFAULT 0,
    data_fingerprint  TEXT,
    metrics_json      TEXT,
    last_error        TEXT,
    updated_at        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tier2_model_trained_at
    ON tier2_model_metadata (user_id, trained_at DESC);
