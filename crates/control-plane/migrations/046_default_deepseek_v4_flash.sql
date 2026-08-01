-- New accounts default to the fast non-reasoning polish route. Existing
-- selections remain untouched.

ALTER TABLE runtime_user_settings
    ALTER COLUMN selected_model SET DEFAULT 'deepseek-v4-flash';
