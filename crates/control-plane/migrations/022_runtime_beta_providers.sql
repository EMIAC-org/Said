-- Allow beta polish providers in the runtime credential vault.
ALTER TABLE runtime_provider_credentials
    DROP CONSTRAINT IF EXISTS runtime_provider_credentials_provider_check;

DELETE FROM runtime_provider_credentials
WHERE provider NOT IN ('groq', 'openai', 'gemini', 'gateway', 'cerebras', 'deepinfra');

ALTER TABLE runtime_provider_credentials
    ADD CONSTRAINT runtime_provider_credentials_provider_check
    CHECK (provider IN (
        'groq', 'openai', 'gemini', 'gateway', 'cerebras', 'deepinfra'
    ));
