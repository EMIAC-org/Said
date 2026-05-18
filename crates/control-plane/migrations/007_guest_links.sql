-- Guest browser links --------------------------------------------------------

ALTER TABLE accounts ADD COLUMN IF NOT EXISTS is_guest BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS guest_invites (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id  UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    created_by  UUID NOT NULL REFERENCES accounts(id),
    expires_at  TIMESTAMPTZ,
    max_uses    INTEGER,
    use_count   INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_guest_invites_token ON guest_invites (token);
CREATE INDEX IF NOT EXISTS idx_guest_invites_meeting ON guest_invites (meeting_id);
