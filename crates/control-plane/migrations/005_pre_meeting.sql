-- Pre-meeting notification support: scheduled start time + notification tracking

ALTER TABLE meetings ADD COLUMN IF NOT EXISTS scheduled_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS meeting_notifications (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id        UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    account_id        UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    notification_type TEXT NOT NULL,
    lark_message_id   TEXT,
    sent_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (meeting_id, account_id, notification_type)
);

CREATE INDEX IF NOT EXISTS idx_meeting_notifications_meeting
    ON meeting_notifications (meeting_id);
