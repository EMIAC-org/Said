-- Lark calendar event sync for scheduled meetings.

ALTER TABLE meetings ADD COLUMN IF NOT EXISTS duration_minutes INTEGER NOT NULL DEFAULT 30;
ALTER TABLE meetings ADD COLUMN IF NOT EXISTS lark_calendar_id TEXT;
ALTER TABLE meetings ADD COLUMN IF NOT EXISTS lark_event_id TEXT;
ALTER TABLE meetings ADD COLUMN IF NOT EXISTS lark_event_status TEXT NOT NULL DEFAULT 'pending';
