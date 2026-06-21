-- DeepSeek is not a voice-polish model choice; migrate legacy rows to Groq fast.

UPDATE runtime_user_settings
   SET selected_model = 'fast'
 WHERE selected_model = 'deepseek';

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN ('fast', 'smart'));
