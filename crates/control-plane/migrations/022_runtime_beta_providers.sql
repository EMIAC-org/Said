-- Allow beta polish providers in the runtime credential vault.
ALTER TABLE runtime_provider_credentials
    DROP CONSTRAINT IF EXISTS runtime_provider_credentials_provider_check;

ALTER TABLE runtime_provider_credentials
    ADD CONSTRAINT runtime_provider_credentials_provider_check
    CHECK (provider IN (
        'deepgram', 'groq', 'openai', 'gemini', 'gateway', 'cerebras', 'deepinfra'
    ));
