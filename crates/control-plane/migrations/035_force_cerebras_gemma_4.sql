-- Force runtime polish settings to Cerebras Gemma 4.

ALTER TABLE runtime_user_settings
    DROP CONSTRAINT IF EXISTS chk_runtime_settings_model;

UPDATE runtime_user_settings
   SET selected_model = 'cerebras-gemma-4'
 WHERE selected_model IS NOT NULL
   AND selected_model != 'cerebras-gemma-4';

ALTER TABLE runtime_user_settings
    ADD CONSTRAINT chk_runtime_settings_model
        CHECK (selected_model IN ('cerebras-gemma-4'));

ALTER TABLE runtime_user_settings
    ALTER COLUMN selected_model SET DEFAULT 'cerebras-gemma-4';
