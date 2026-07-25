-- Keep Gemma as the default while allowing users to compare DeepSeek V4 Flash.

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN (
            'deepinfra-gemma-4-26b-a4b',
            'deepseek-v4-flash'
        ));

ALTER TABLE runtime_provider_credentials
    DROP CONSTRAINT IF EXISTS runtime_provider_credentials_provider_check;

ALTER TABLE runtime_provider_credentials
    ADD CONSTRAINT runtime_provider_credentials_provider_check
    CHECK (provider IN (
        'groq', 'openai', 'gemini', 'gateway', 'deepgram', 'deepinfra', 'deepseek'
    ));
