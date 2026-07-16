-- Replace the retired OpenRouter production polish route with DeepInfra's
-- direct, priority-tier Gemma 4 26B A4B route. This is forward-only: prior
-- migrations remain recorded on deployed databases.

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

UPDATE runtime_user_settings
   SET selected_model = 'deepinfra-gemma-4-26b-a4b'
 WHERE selected_model IS DISTINCT FROM 'deepinfra-gemma-4-26b-a4b';

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN ('deepinfra-gemma-4-26b-a4b'));

ALTER TABLE runtime_user_settings
    ALTER COLUMN selected_model SET DEFAULT 'deepinfra-gemma-4-26b-a4b';

-- OpenRouter no longer has a runtime route, so no stored credential can be
-- selected accidentally after the binary changes.
DELETE FROM runtime_provider_credentials
 WHERE provider = 'openrouter';

ALTER TABLE runtime_provider_credentials
    DROP CONSTRAINT IF EXISTS runtime_provider_credentials_provider_check;

ALTER TABLE runtime_provider_credentials
    ADD CONSTRAINT runtime_provider_credentials_provider_check
    CHECK (provider IN (
        'groq', 'openai', 'gemini', 'gateway', 'deepgram', 'deepinfra'
    ));
