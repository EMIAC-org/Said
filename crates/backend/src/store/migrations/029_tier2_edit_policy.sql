-- Tier 2 local edit-policy rules.
--
-- These rows are learned only from explicit user edits. They are intentionally
-- narrower than fuzzy scorers: an active replace rule applies an observed STT
-- variant to one protected correction target.

CREATE TABLE IF NOT EXISTS tier2_edit_policy_rules (
    user_id             TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    variant_form        TEXT NOT NULL,
    variant_norm        TEXT NOT NULL,
    correct_form        TEXT NOT NULL,
    correct_form_norm   TEXT NOT NULL,
    edit_type           TEXT NOT NULL DEFAULT 'replace',
    positive_count      INTEGER NOT NULL DEFAULT 0,
    negative_count      INTEGER NOT NULL DEFAULT 0,
    status              TEXT NOT NULL DEFAULT 'candidate',
    first_seen          INTEGER NOT NULL,
    last_seen           INTEGER NOT NULL,
    left_context_json   TEXT,
    right_context_json  TEXT,
    source_recording_id TEXT REFERENCES recordings(id) ON DELETE SET NULL,
    PRIMARY KEY (user_id, variant_norm, correct_form_norm, edit_type)
);

CREATE INDEX IF NOT EXISTS idx_tier2_edit_policy_user_status
    ON tier2_edit_policy_rules (user_id, status, last_seen DESC);

CREATE INDEX IF NOT EXISTS idx_tier2_edit_policy_user_variant
    ON tier2_edit_policy_rules (user_id, variant_norm);

ALTER TABLE recordings ADD COLUMN raw_transcript TEXT;
ALTER TABLE recordings ADD COLUMN local_corrected_transcript TEXT;
ALTER TABLE recordings ADD COLUMN polished_output TEXT;
