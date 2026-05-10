-- recording_seconds was incorrectly storing pipeline latency (STT+LLM
-- processing time) instead of actual audio duration.  Recalculate it from
-- word_count at an estimated 130 WPM for all rows where the implied WPM
-- exceeds 300 (physically impossible for human speech).
UPDATE recordings
SET recording_seconds = CAST(word_count AS REAL) * 60.0 / 130.0
WHERE word_count > 0
  AND recording_seconds > 0
  AND (CAST(word_count AS REAL) / (recording_seconds / 60.0)) > 300;
