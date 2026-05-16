-- Track when a pending edit notification was shown to the user.
-- Once notified, it should not ping again unless explicitly un-dismissed.
ALTER TABLE pending_edits ADD COLUMN notified_at INTEGER;
