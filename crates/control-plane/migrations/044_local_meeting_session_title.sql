ALTER TABLE local_meeting_sessions
    ADD COLUMN IF NOT EXISTS title TEXT;

UPDATE local_meeting_sessions
   SET title = 'Untitled meeting'
 WHERE title IS NULL OR length(btrim(title)) = 0;

ALTER TABLE local_meeting_sessions
    ALTER COLUMN title SET NOT NULL;

ALTER TABLE local_meeting_sessions
    DROP CONSTRAINT IF EXISTS ck_local_meeting_session_title;

ALTER TABLE local_meeting_sessions
    ADD CONSTRAINT ck_local_meeting_session_title
    CHECK (char_length(btrim(title)) BETWEEN 1 AND 200);
