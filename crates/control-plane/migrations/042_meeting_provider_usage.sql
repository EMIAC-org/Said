-- Per-slot meeting AI provider usage + estimated cost. The meeting summary
-- pipeline runs over the Codex backend but is billed against the DeepSeek V4
-- Flash rate card (see crates/control-plane/src/ai_worker.rs), so token counts
-- and an estimated cost are recorded here for org/user cost rollups.
CREATE TABLE IF NOT EXISTS meeting_provider_usage (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id          UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    org_id              UUID NOT NULL,
    slot_index          INTEGER,
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    input_tokens        INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens       INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd  DOUBLE PRECISION,
    cost_source         TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_meeting_provider_usage_meeting
    ON meeting_provider_usage (meeting_id);
CREATE INDEX IF NOT EXISTS idx_meeting_provider_usage_org_created
    ON meeting_provider_usage (org_id, created_at);
