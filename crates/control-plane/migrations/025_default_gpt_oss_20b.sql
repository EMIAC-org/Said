-- GPT OSS 20B is the production default. Allow full catalog keys in runtime settings.

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

UPDATE runtime_user_settings
   SET selected_model = CASE
        WHEN selected_model IN ('fast', 'smart', 'cerebras-gpt-oss', 'groq-gpt-oss-20b', 'groq-70b', 'phi4') THEN selected_model
        WHEN selected_model IN ('deepseek', 'llama-3.1-8b-instant') THEN 'fast'
        WHEN selected_model IN ('scout', 'groq-scout')
             OR selected_model ILIKE '%scout%' THEN 'groq-scout'
        WHEN selected_model IN ('gpt-oss-20b', 'openai/gpt-oss-20b') THEN 'groq-gpt-oss-20b'
        WHEN selected_model IN ('cerebras', 'maverick', 'groq-maverick', 'gpt-oss', 'gpt-oss-120b', 'openai/gpt-oss-120b')
             OR selected_model ILIKE '%maverick%'
             OR selected_model ILIKE '%gpt-oss%'
             OR selected_model ILIKE '%gpt_oss%' THEN 'cerebras-gpt-oss'
        ELSE 'groq-gpt-oss-20b'
       END;

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN (
            'fast', 'smart',
            'cerebras-gpt-oss',
            'groq-gpt-oss-20b',
            'groq-scout',
            'groq-70b',
            'phi4'
        ));

UPDATE runtime_user_settings
   SET selected_model = 'groq-gpt-oss-20b'
 WHERE selected_model = 'smart';

ALTER TABLE runtime_user_settings
    ALTER COLUMN selected_model SET DEFAULT 'groq-gpt-oss-20b';
