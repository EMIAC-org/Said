-- Allow DeepSeek as a server polish model choice (Groq fast/smart remain default paths).

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN ('fast', 'smart', 'deepseek'));
