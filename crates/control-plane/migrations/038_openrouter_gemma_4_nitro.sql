-- Restore the single production polish route to OpenRouter Nitro. Migration
-- 037 may already be recorded on deployed databases, so this is deliberately
-- a new forward-only migration rather than a rewritten history entry.

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

UPDATE runtime_user_settings
   SET selected_model = 'openrouter-gemma-4-nitro'
 WHERE selected_model IS DISTINCT FROM 'openrouter-gemma-4-nitro';

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN ('openrouter-gemma-4-nitro'));

ALTER TABLE runtime_user_settings
    ALTER COLUMN selected_model SET DEFAULT 'openrouter-gemma-4-nitro';

-- Together credentials were only used by the retired control-plane polish
-- route. Cloud STT keeps its Together key locally and never reads this vault.
DELETE FROM runtime_provider_credentials
 WHERE provider IN ('together', 'cerebras');

ALTER TABLE runtime_provider_credentials
    DROP CONSTRAINT IF EXISTS runtime_provider_credentials_provider_check;

ALTER TABLE runtime_provider_credentials
    ADD CONSTRAINT runtime_provider_credentials_provider_check
    CHECK (provider IN (
        'groq', 'openai', 'gemini', 'gateway', 'deepgram', 'openrouter', 'deepinfra'
    ));
