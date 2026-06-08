-- Cross-device runtime settings per AirNote user.
-- Secrets stay in runtime_provider_credentials; this table stores only behaviour prefs.

CREATE TABLE IF NOT EXISTS runtime_user_settings (
    account_id                   UUID PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    selected_model               TEXT NOT NULL DEFAULT 'fast',
    output_language              TEXT NOT NULL DEFAULT 'hinglish',
    tone_preset                  TEXT NOT NULL DEFAULT 'professional',
    custom_prompt                TEXT,
    auto_paste                   BOOLEAN NOT NULL DEFAULT TRUE,
    edit_capture                 BOOLEAN NOT NULL DEFAULT TRUE,
    learning_enabled             BOOLEAN NOT NULL DEFAULT TRUE,
    server_runtime_enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    server_audio_runtime_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    message_polish_mode          BOOLEAN NOT NULL DEFAULT FALSE,
    notification_prefs_json      JSONB NOT NULL DEFAULT '{}'::jsonb,
    privacy_prefs_json           JSONB NOT NULL DEFAULT '{}'::jsonb,
    version                      BIGINT NOT NULL DEFAULT 1,
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_runtime_settings_model    CHECK (selected_model   IN ('fast', 'smart')),
    CONSTRAINT chk_runtime_settings_language CHECK (output_language  IN ('hinglish', 'english'))
);

CREATE TABLE IF NOT EXISTS runtime_settings_audit_log (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id          UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    changed_by          UUID REFERENCES accounts(id) ON DELETE SET NULL,
    changed_fields_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    source              TEXT NOT NULL DEFAULT 'desktop',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (source IN ('desktop', 'mobile', 'admin', 'migration'))
);
CREATE INDEX IF NOT EXISTS idx_runtime_settings_audit_account
    ON runtime_settings_audit_log (account_id, created_at DESC);
