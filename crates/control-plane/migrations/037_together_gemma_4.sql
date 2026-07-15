-- Replace the production polish route with Together AI's paid Gemma 4 31B.
ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

UPDATE runtime_user_settings
   SET selected_model = 'together-gemma-4-31b'
 WHERE selected_model IS DISTINCT FROM 'together-gemma-4-31b';

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN ('together-gemma-4-31b'));

ALTER TABLE runtime_user_settings
    ALTER COLUMN selected_model SET DEFAULT 'together-gemma-4-31b';

DELETE FROM runtime_provider_credentials
 WHERE provider IN ('openrouter', 'cerebras');

ALTER TABLE runtime_provider_credentials
    DROP CONSTRAINT IF EXISTS runtime_provider_credentials_provider_check;

ALTER TABLE runtime_provider_credentials
    ADD CONSTRAINT runtime_provider_credentials_provider_check
    CHECK (provider IN (
        'groq', 'openai', 'gemini', 'gateway', 'together', 'deepinfra'
    ));
