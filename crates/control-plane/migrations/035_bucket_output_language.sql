-- Per-bucket output-language override.
--
-- Lets a user force the polish OUTPUT LANGUAGE per app-context bucket, independent
-- of the account/request default. The motivating case: speak Hindi/Hinglish freely
-- while dictating a prompt into an AI/coding app but get clean ENGLISH out, while
-- messaging apps preserve the spoken Hinglish tone.
--
-- This is a USER CHOICE, not a learned style knob. It therefore lives in its own
-- column, NOT inside `profile_json` (which the batch profiling worker overwrites on
-- every rebuild via upsert_bucket_profile). NULL = inherit the request's language
-- (today's behavior, no change). Allowed values mirror the runtime language enum.
ALTER TABLE runtime_user_bucket_profiles
    ADD COLUMN IF NOT EXISTS output_language_override TEXT;

ALTER TABLE runtime_user_bucket_profiles
    DROP CONSTRAINT IF EXISTS chk_bucket_output_language_override;

ALTER TABLE runtime_user_bucket_profiles
    ADD CONSTRAINT chk_bucket_output_language_override CHECK (
        output_language_override IS NULL
        OR output_language_override IN ('english', 'hinglish', 'hindi')
    );
