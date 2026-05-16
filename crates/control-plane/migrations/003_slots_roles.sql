-- Wave 3: Slots + Roles + Participant lifecycle ---------------------------------

-- Meeting slots (atomic AI processing units) ----------------------------------
CREATE TABLE IF NOT EXISTS meeting_slots (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id          UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    slot_index          INTEGER NOT NULL,
    start_ms            BIGINT NOT NULL,
    end_ms              BIGINT,                        -- NULL while slot is open
    chunk_count         INTEGER NOT NULL DEFAULT 0,
    word_count          INTEGER NOT NULL DEFAULT 0,
    speaker_ids         UUID[] NOT NULL DEFAULT '{}',
    ai_status           TEXT NOT NULL DEFAULT 'pending', -- pending | processing | done | failed
    summary_delta       TEXT,
    tasks_detected      INTEGER NOT NULL DEFAULT 0,
    decisions_detected  INTEGER NOT NULL DEFAULT 0,
    gap_noted           BOOLEAN NOT NULL DEFAULT false,  -- true if chunks were missing (disconnect)
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_meeting_slots_meeting ON meeting_slots (meeting_id, slot_index);

-- Per-org configurable meeting creator roles ----------------------------------
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS meeting_creator_roles JSONB NOT NULL DEFAULT '["COMPANY_ADMIN", "MANAGER"]';

-- Add 'disconnected' to participant status vocabulary (no enum — string field).
-- Add disconnect tracking columns.
ALTER TABLE meeting_participants ADD COLUMN IF NOT EXISTS disconnect_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE meeting_participants ADD COLUMN IF NOT EXISTS last_chunk_index INTEGER;

-- Auto-end silence config per org (seconds, default 7200 = 2 hours) -----------
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS auto_end_silence_secs INTEGER NOT NULL DEFAULT 7200;

-- Track last activity per meeting for auto-end --------------------------------
ALTER TABLE meetings ADD COLUMN IF NOT EXISTS last_activity_at TIMESTAMPTZ;
